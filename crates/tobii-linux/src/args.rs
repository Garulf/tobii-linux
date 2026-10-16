//! The subset of Python's argparse the command line uses, with its parsing rules and messages.
//!
//! Options may be abbreviated to a unique prefix and given as `--option=value`; negative
//! numbers are values, not options; the last occurrence wins; `-h` prints the help text
//! (argparse's own output at 80 columns) and exits with 0; errors print the usage and exit
//! with 2. Unrecognized arguments are reported by the top-level parser, after the
//! subcommand's own checks, as argparse does.

use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arity {
    Flag,
    One,
    Exactly(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Text,
    Int,
    Float,
}

#[derive(Debug, Clone, Copy)]
pub struct OptionSpec {
    pub flag: &'static str,
    pub arity: Arity,
    pub kind: Kind,
    pub choices: &'static [&'static str],
    pub required: bool,
}

impl OptionSpec {
    pub const fn flag(flag: &'static str) -> Self {
        Self { flag, arity: Arity::Flag, kind: Kind::Text, choices: &[], required: false }
    }

    pub const fn text(flag: &'static str) -> Self {
        Self { flag, arity: Arity::One, kind: Kind::Text, choices: &[], required: false }
    }

    pub const fn int(flag: &'static str) -> Self {
        Self { flag, arity: Arity::One, kind: Kind::Int, choices: &[], required: false }
    }

    pub const fn float(flag: &'static str) -> Self {
        Self { flag, arity: Arity::One, kind: Kind::Float, choices: &[], required: false }
    }

    pub const fn floats(flag: &'static str, count: usize) -> Self {
        Self { flag, arity: Arity::Exactly(count), kind: Kind::Float, choices: &[], required: false }
    }

    pub const fn choice(flag: &'static str, choices: &'static [&'static str]) -> Self {
        Self { flag, arity: Arity::One, kind: Kind::Text, choices, required: false }
    }

    pub const fn required(self) -> Self {
        Self { required: true, ..self }
    }
}

/// One parser: its program name (as argparse prints it), help text and options.
#[derive(Debug, Clone)]
pub struct Parser {
    pub prog: String,
    pub help: String,
    pub options: Vec<OptionSpec>,
}

/// How parsing ended without values: help printed (exit 0) or an error (exit 2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Exit {
    Help(String),
    Error(String),
}

impl Exit {
    pub fn code(&self) -> u8 {
        match self {
            Exit::Help(_) => 0,
            Exit::Error(_) => 2,
        }
    }

    /// Writes the help text to stdout or the error to stderr.
    pub fn report(&self) {
        match self {
            Exit::Help(text) => print!("{text}"),
            Exit::Error(text) => eprint!("{text}"),
        }
    }
}

/// Values given on the command line, by option flag; absent options take their defaults.
#[derive(Debug, Clone, Default)]
pub struct Values {
    given: HashMap<&'static str, Vec<String>>,
}

impl Values {
    pub fn flag(&self, flag: &str) -> bool {
        self.given.contains_key(flag)
    }

    pub fn text(&self, flag: &str) -> Option<&str> {
        self.given.get(flag).map(|values| values[0].as_str())
    }

    pub fn int(&self, flag: &str) -> Option<i64> {
        self.text(flag).map(|text| python_int(text).expect("checked while parsing"))
    }

    pub fn float(&self, flag: &str) -> Option<f64> {
        self.text(flag).map(|text| python_float(text).expect("checked while parsing"))
    }

    pub fn floats(&self, flag: &str) -> Option<Vec<f64>> {
        self.given
            .get(flag)
            .map(|values| values.iter().map(|text| python_float(text).expect("checked while parsing")).collect())
    }
}

fn without_underscores(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let separated = bytes.iter().enumerate().all(|(index, byte)| {
        *byte != b'_'
            || (index > 0
                && index + 1 < bytes.len()
                && bytes[index - 1].is_ascii_alphanumeric()
                && bytes[index + 1].is_ascii_alphanumeric())
    });
    separated.then(|| text.replace('_', ""))
}

/// Python's `int(text)` for decimal text.
pub fn python_int(text: &str) -> Option<i64> {
    let cleaned = without_underscores(text.trim())?;
    let digits = cleaned.strip_prefix(['+', '-']).unwrap_or(&cleaned);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    cleaned.parse().ok()
}

/// Python's `float(text)`.
pub fn python_float(text: &str) -> Option<f64> {
    let cleaned = without_underscores(text.trim())?;
    let body = cleaned.strip_prefix(['+', '-']).unwrap_or(&cleaned).to_ascii_lowercase();
    if !matches!(body.as_str(), "inf" | "infinity" | "nan")
        && !body.bytes().all(|b| b.is_ascii_digit() || matches!(b, b'.' | b'e' | b'+' | b'-'))
    {
        return None;
    }
    cleaned.parse().ok()
}

/// Python's `repr` of a str.
pub fn python_repr(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') { '"' } else { '\'' };
    let mut out = String::from(quote);
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => out.push_str(&format!("\\x{:02x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

fn looks_negative(text: &str) -> bool {
    let Some(rest) = text.strip_prefix('-') else { return false };
    let all_digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    match rest.split_once('.') {
        None => all_digits(rest),
        Some((whole, fraction)) => (whole.is_empty() || all_digits(whole)) && all_digits(fraction),
    }
}

/// An argument string as argparse classifies it before consuming anything.
#[derive(Debug, Clone)]
enum Token {
    Positional(String),
    Option { spec: Option<OptionSpec>, text: String, explicit: Option<String> },
    Help { explicit: Option<String> },
}

const HELP_FLAGS: [&str; 2] = ["-h", "--help"];

impl Parser {
    pub fn new(prog: impl Into<String>, help: impl Into<String>, options: Vec<OptionSpec>) -> Self {
        Self { prog: prog.into(), help: help.into(), options }
    }

    /// The usage block at the top of the help text.
    pub fn usage(&self) -> &str {
        self.help.split("\n\n").next().unwrap_or(&self.help)
    }

    pub fn error(&self, message: &str) -> Exit {
        Exit::Error(format!("{}\n{}: error: {message}\n", self.usage(), self.prog))
    }

    fn lookup(&self, flag: &str) -> Option<Token> {
        if HELP_FLAGS.contains(&flag) {
            return Some(Token::Help { explicit: None });
        }
        self.options.iter().find(|spec| spec.flag == flag).map(|spec| Token::Option {
            spec: Some(*spec),
            text: flag.into(),
            explicit: None,
        })
    }

    fn classify(&self, arg: &str) -> Result<Token, Exit> {
        if !arg.starts_with('-') || arg == "-" {
            return Ok(Token::Positional(arg.into()));
        }
        if let Some(token) = self.lookup(arg) {
            return Ok(token);
        }
        let (flag, explicit) = match arg.split_once('=') {
            Some((flag, value)) => (flag, Some(value.to_string())),
            None => (arg, None),
        };
        if explicit.is_some()
            && let Some(token) = self.lookup(flag)
        {
            return Ok(with_explicit(token, explicit));
        }
        if arg.starts_with("--") {
            let mut matches: Vec<&str> = HELP_FLAGS[1..]
                .iter()
                .copied()
                .chain(self.options.iter().map(|spec| spec.flag))
                .filter(|candidate| candidate.starts_with("--") && candidate.starts_with(flag))
                .collect();
            matches.sort_by_key(|candidate| self.position(candidate));
            match matches.as_slice() {
                [only] => {
                    let token = self.lookup(only).expect("a known flag");
                    return Ok(with_explicit(token, explicit));
                }
                [] => {}
                several => {
                    return Err(self.error(&format!("ambiguous option: {flag} could match {}", several.join(", "))));
                }
            }
        }
        if looks_negative(arg) || arg.contains(' ') {
            return Ok(Token::Positional(arg.into()));
        }
        Ok(Token::Option { spec: None, text: arg.into(), explicit: None })
    }

    fn position(&self, flag: &str) -> usize {
        if HELP_FLAGS.contains(&flag) {
            return 0;
        }
        1 + self.options.iter().position(|spec| spec.flag == flag).unwrap_or(usize::MAX - 1)
    }

    fn checked(&self, spec: &OptionSpec, value: &str) -> Result<(), Exit> {
        let (valid, kind) = match spec.kind {
            Kind::Text => (true, ""),
            Kind::Int => (python_int(value).is_some(), "int"),
            Kind::Float => (python_float(value).is_some(), "float"),
        };
        if !valid {
            return Err(self.error(&format!("argument {}: invalid {kind} value: {}", spec.flag, python_repr(value))));
        }
        if !spec.choices.is_empty() && !spec.choices.contains(&value) {
            let choices: Vec<String> = spec.choices.iter().map(|choice| python_repr(choice)).collect();
            return Err(self.error(&format!(
                "argument {}: invalid choice: {} (choose from {})",
                spec.flag,
                python_repr(value),
                choices.join(", ")
            )));
        }
        Ok(())
    }

    /// Parses `args`, returning the values and the arguments it did not recognize.
    pub fn parse_known(&self, args: &[String]) -> Result<(Values, Vec<String>), Exit> {
        let mut tokens = Vec::with_capacity(args.len());
        let mut rest = args.iter();
        for arg in rest.by_ref() {
            if arg == "--" {
                break;
            }
            tokens.push(self.classify(arg)?);
        }
        tokens.extend(rest.map(|arg| Token::Positional(arg.clone())));
        let mut values = Values::default();
        let mut extras = Vec::new();
        let mut index = 0;
        while index < tokens.len() {
            let token = tokens[index].clone();
            index += 1;
            match token {
                Token::Positional(text) => extras.push(text),
                Token::Help { explicit: None } => return Err(Exit::Help(self.help.clone())),
                Token::Help { explicit: Some(value) } => {
                    return Err(
                        self.error(&format!("argument -h/--help: ignored explicit argument {}", python_repr(&value)))
                    );
                }
                Token::Option { spec: None, text, .. } => extras.push(text),
                Token::Option { spec: Some(spec), explicit, .. } => {
                    let taken = match (spec.arity, explicit) {
                        (Arity::Flag, Some(value)) => {
                            return Err(self.error(&format!(
                                "argument {}: ignored explicit argument {}",
                                spec.flag,
                                python_repr(&value)
                            )));
                        }
                        (Arity::Flag, None) => Vec::new(),
                        (Arity::One, Some(value)) => vec![value],
                        (Arity::Exactly(_), Some(_)) => {
                            return Err(self.error(&format!("argument {}: {}", spec.flag, expected(spec.arity))));
                        }
                        (arity, None) => {
                            let count = if arity == Arity::One { 1 } else { expected_count(arity) };
                            let available = tokens[index..]
                                .iter()
                                .take_while(|token| matches!(token, Token::Positional(_)))
                                .count();
                            if available < count {
                                return Err(self.error(&format!("argument {}: {}", spec.flag, expected(arity))));
                            }
                            let taken = tokens[index..index + count]
                                .iter()
                                .map(|token| match token {
                                    Token::Positional(text) => text.clone(),
                                    _ => unreachable!("counted positionals"),
                                })
                                .collect();
                            index += count;
                            taken
                        }
                    };
                    for value in &taken {
                        self.checked(&spec, value)?;
                    }
                    values.given.insert(spec.flag, taken);
                }
            }
        }
        let missing: Vec<&str> =
            self.options.iter().filter(|spec| spec.required && !values.flag(spec.flag)).map(|spec| spec.flag).collect();
        if !missing.is_empty() {
            return Err(self.error(&format!("the following arguments are required: {}", missing.join(", "))));
        }
        Ok((values, extras))
    }

    /// Parses `args`; unrecognized arguments are an error of this parser.
    pub fn parse(&self, args: &[String]) -> Result<Values, Exit> {
        let (values, extras) = self.parse_known(args)?;
        if !extras.is_empty() {
            return Err(self.error(&format!("unrecognized arguments: {}", extras.join(" "))));
        }
        Ok(values)
    }
}

fn with_explicit(token: Token, explicit: Option<String>) -> Token {
    match token {
        Token::Option { spec, text, .. } => Token::Option { spec, text, explicit },
        Token::Help { .. } => Token::Help { explicit },
        positional => positional,
    }
}

fn expected_count(arity: Arity) -> usize {
    match arity {
        Arity::Flag => 0,
        Arity::One => 1,
        Arity::Exactly(count) => count,
    }
}

fn expected(arity: Arity) -> String {
    match arity {
        Arity::One => "expected one argument".into(),
        other => format!("expected {} arguments", expected_count(other)),
    }
}

/// Help text captured for the program name `tobii-linux`, renamed for another program name.
pub fn help_for(prog: &str, captured: &str) -> String {
    const CAPTURED_PROG: &str = "tobii-linux";
    if prog == CAPTURED_PROG {
        return captured.to_string();
    }
    let shift = prog.len() as isize - CAPTURED_PROG.len() as isize;
    let (usage, rest) = captured.split_once("\n\n").unwrap_or((captured, ""));
    let mut lines = usage.lines();
    let mut renamed = vec![lines.next().unwrap_or_default().replacen(CAPTURED_PROG, prog, 1)];
    for line in lines {
        let indent = line.len() - line.trim_start().len();
        let indent = (indent as isize + shift).max(1) as usize;
        renamed.push(format!("{}{}", " ".repeat(indent), line.trim_start()));
    }
    let rest = rest.replace(&format!("{CAPTURED_PROG} "), &format!("{prog} "));
    if rest.is_empty() { format!("{}\n", renamed.join("\n")) } else { format!("{}\n\n{rest}", renamed.join("\n")) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parser() -> Parser {
        Parser::new(
            "tobii-linux serve",
            "usage: tobii-linux serve [-h] --display ...\n\noptions:\n  -h, --help\n",
            vec![
                OptionSpec::floats("--display", 9).required(),
                OptionSpec::int("--port"),
                OptionSpec::text("--name"),
                OptionSpec::flag("--no-gaze"),
                OptionSpec::choice("--wine-clock", &["auto", "boottime", "raw"]),
            ],
        )
    }

    fn args(text: &str) -> Vec<String> {
        text.split_whitespace().map(String::from).collect()
    }

    fn error(text: &str) -> String {
        match parser().parse_known(&args(text)) {
            Err(Exit::Error(message)) => message.lines().last().unwrap().to_string(),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn values_negative_numbers_abbreviations_and_extras() {
        let (values, extras) = parser()
            .parse_known(&args("--disp -296.62 326 111.82 1 2 3 4 5 6 --po=7 x --no -y --name a --name b"))
            .unwrap();
        assert_eq!(values.floats("--display").unwrap()[0], -296.62);
        assert_eq!(values.int("--port"), Some(7));
        assert_eq!(values.text("--name"), Some("b"));
        assert!(values.flag("--no-gaze") && !values.flag("--wine-clock"));
        assert_eq!(extras, ["x", "-y"]);
    }

    #[test]
    fn errors_match_argparse() {
        assert_eq!(error(""), "tobii-linux serve: error: the following arguments are required: --display");
        assert_eq!(error("--display 1 2 3"), "tobii-linux serve: error: argument --display: expected 9 arguments");
        assert_eq!(error("--display=1"), "tobii-linux serve: error: argument --display: expected 9 arguments");
        assert_eq!(error("--port x"), "tobii-linux serve: error: argument --port: invalid int value: 'x'");
        assert_eq!(error("--name"), "tobii-linux serve: error: argument --name: expected one argument");
        assert_eq!(error("--n x"), "tobii-linux serve: error: ambiguous option: --n could match --name, --no-gaze");
        assert_eq!(error("--no-gaze=1"), "tobii-linux serve: error: argument --no-gaze: ignored explicit argument '1'");
        assert_eq!(
            error("--wine-clock x"),
            "tobii-linux serve: error: argument --wine-clock: invalid choice: 'x' (choose from 'auto', 'boottime', 'raw')"
        );
        assert!(matches!(parser().parse_known(&args("--port 3 --he")), Err(Exit::Help(_))));
    }

    #[test]
    fn python_number_syntax() {
        assert_eq!(python_int(" 1_000 "), Some(1000));
        assert_eq!(python_int("0x10"), None);
        assert_eq!(python_float("1e3"), Some(1000.0));
        assert!(python_float("-inf").unwrap().is_infinite());
        assert_eq!(python_float("1__0"), None);
        assert_eq!(python_repr("it's"), "\"it's\"");
    }

    #[test]
    fn help_is_renamed_for_another_program() {
        let help = help_for("tl", "usage: tobii-linux serve [-h] --a\n                         [--b]\n\nx\n");
        assert_eq!(help, "usage: tl serve [-h] --a\n                [--b]\n\nx\n");
    }
}
