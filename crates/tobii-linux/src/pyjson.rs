//! JSON text exactly as Python's `json.dumps` writes it, so output matches the Python command.
//!
//! Floats use Python's `repr` (shortest round trip, exponent below 1e-4 and from 1e16),
//! strings are ASCII with `\uXXXX` escapes, and objects keep their insertion order.

use std::fmt::Write;

#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    List(Vec<Json>),
    Object(Vec<(String, Json)>),
}

impl From<bool> for Json {
    fn from(value: bool) -> Self {
        Json::Bool(value)
    }
}

macro_rules! from_int {
    ($($t:ty),*) => {$(
        impl From<$t> for Json {
            fn from(value: $t) -> Self {
                Json::Int(i64::from(value))
            }
        }
    )*};
}
from_int!(i8, i16, i32, i64, u8, u16, u32);

impl From<usize> for Json {
    fn from(value: usize) -> Self {
        Json::Int(value as i64)
    }
}

impl From<u64> for Json {
    fn from(value: u64) -> Self {
        Json::Int(value as i64)
    }
}

impl From<f64> for Json {
    fn from(value: f64) -> Self {
        Json::Float(value)
    }
}

impl From<f32> for Json {
    fn from(value: f32) -> Self {
        Json::Float(f64::from(value))
    }
}

impl From<&str> for Json {
    fn from(value: &str) -> Self {
        Json::Str(value.to_owned())
    }
}

impl From<String> for Json {
    fn from(value: String) -> Self {
        Json::Str(value)
    }
}

impl<T: Into<Json>> From<Option<T>> for Json {
    fn from(value: Option<T>) -> Self {
        value.map_or(Json::Null, Into::into)
    }
}

impl<T: Into<Json>> From<Vec<T>> for Json {
    fn from(values: Vec<T>) -> Self {
        Json::List(values.into_iter().map(Into::into).collect())
    }
}

impl<T: Into<Json> + Clone, const N: usize> From<[T; N]> for Json {
    fn from(values: [T; N]) -> Self {
        Json::List(values.into_iter().map(Into::into).collect())
    }
}

/// An object from `(key, value)` pairs, in order.
pub fn object<const N: usize>(entries: [(&str, Json); N]) -> Json {
    Json::Object(entries.into_iter().map(|(key, value)| (key.to_owned(), value)).collect())
}

/// Python's `repr(float)`.
/// The shortest digits that read back as `value`, nearest to it, ties to even as Python's
/// repr: Rust's shortest form can round a tie up.
fn nearest_shortest(value: f64) -> String {
    let shortest = format!("{value:e}");
    let digits = shortest.split('e').next().unwrap_or_default().bytes().filter(u8::is_ascii_digit).count();
    let nearest = format!("{value:.precision$e}", precision = digits.saturating_sub(1));
    if nearest.parse::<f64>() == Ok(value) { nearest } else { shortest }
}

pub fn float_repr(value: f64) -> String {
    if value.is_nan() {
        return "nan".into();
    }
    if value.is_infinite() {
        return if value > 0.0 { "inf".into() } else { "-inf".into() };
    }
    let scientific = nearest_shortest(value);
    let (mantissa, exponent) = scientific.split_once('e').expect("Rust's {:e} always has an exponent");
    let exponent: i32 = exponent.parse().expect("integer exponent");
    let (sign, mantissa) = mantissa.strip_prefix('-').map_or(("", mantissa), |rest| ("-", rest));
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    if !(-4..16).contains(&exponent) {
        let (first, rest) = digits.split_at(1);
        let fraction = if rest.is_empty() { String::new() } else { format!(".{rest}") };
        let exponent_sign = if exponent < 0 { '-' } else { '+' };
        return format!("{sign}{first}{fraction}e{exponent_sign}{:02}", exponent.abs());
    }
    let point = exponent + 1;
    let text = if point <= 0 {
        format!("0.{}{digits}", "0".repeat((-point) as usize))
    } else if point as usize >= digits.len() {
        format!("{digits}{}.0", "0".repeat(point as usize - digits.len()))
    } else {
        let (whole, fraction) = digits.split_at(point as usize);
        format!("{whole}.{fraction}")
    };
    format!("{sign}{text}")
}

fn json_float(value: f64) -> String {
    if value.is_nan() {
        "NaN".into()
    } else if value.is_infinite() {
        if value > 0.0 { "Infinity".into() } else { "-Infinity".into() }
    } else {
        float_repr(value)
    }
}

/// A JSON string literal with `ensure_ascii`.
pub fn string_literal(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            ' '..='~' => out.push(c),
            _ => {
                let mut units = [0u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    let _ = write!(out, "\\u{unit:04x}");
                }
            }
        }
    }
    out.push('"');
    out
}

#[derive(Debug, Clone, Copy)]
enum Style {
    /// `json.dumps(value)`: `", "` and `": "`.
    Default,
    /// `separators=(',', ':')`.
    Compact,
    /// `indent=2`.
    Indented,
}

fn write(out: &mut String, value: &Json, style: Style, depth: usize) {
    let (item, key) = match style {
        Style::Default => (", ", ": "),
        Style::Compact => (",", ":"),
        Style::Indented => (",", ": "),
    };
    let newline = |out: &mut String, depth: usize| {
        if matches!(style, Style::Indented) {
            out.push('\n');
            out.push_str(&"  ".repeat(depth));
        }
    };
    match value {
        Json::Null => out.push_str("null"),
        Json::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
        Json::Int(value) => {
            let _ = write!(out, "{value}");
        }
        Json::Float(value) => out.push_str(&json_float(*value)),
        Json::Str(text) => out.push_str(&string_literal(text)),
        Json::List(items) if items.is_empty() => out.push_str("[]"),
        Json::Object(entries) if entries.is_empty() => out.push_str("{}"),
        Json::List(items) => {
            out.push('[');
            for (index, element) in items.iter().enumerate() {
                if index > 0 {
                    out.push_str(item);
                }
                newline(out, depth + 1);
                write(out, element, style, depth + 1);
            }
            newline(out, depth);
            out.push(']');
        }
        Json::Object(entries) => {
            out.push('{');
            for (index, (name, element)) in entries.iter().enumerate() {
                if index > 0 {
                    out.push_str(item);
                }
                newline(out, depth + 1);
                out.push_str(&string_literal(name));
                out.push_str(key);
                write(out, element, style, depth + 1);
            }
            newline(out, depth);
            out.push('}');
        }
    }
}

/// `json.dumps(value)`.
pub fn dumps(value: &Json) -> String {
    let mut out = String::new();
    write(&mut out, value, Style::Default, 0);
    out
}

/// `json.dumps(value, separators=(',', ':'))`.
pub fn compact(value: &Json) -> String {
    let mut out = String::new();
    write(&mut out, value, Style::Compact, 0);
    out
}

/// `json.dumps(value, indent=2)`.
pub fn indented(value: &Json) -> String {
    let mut out = String::new();
    write(&mut out, value, Style::Indented, 0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floats_print_as_python_repr() {
        let cases = [
            (1.0, "1.0"),
            (-0.0, "-0.0"),
            (0.1, "0.1"),
            (600.0, "600.0"),
            (1e-5, "1e-05"),
            (0.0001, "0.0001"),
            (1.5e-7, "1.5e-07"),
            (1e16, "1e+16"),
            (1234567890123456.0, "1234567890123456.0"),
            (12345678901234567.0, "1.2345678901234568e+16"),
            (-296.62, "-296.62"),
            (0.30000000000000004, "0.30000000000000004"),
            (f64::from(0.35f32), "0.3499999940395355"),
            (1e300, "1e+300"),
            (5e-324, "5e-324"),
            (f64::from(-26.342_422_f32), "-26.342422485351562"),
        ];
        for (value, text) in cases {
            assert_eq!(float_repr(value), text, "{value:e}");
        }
    }

    #[test]
    fn strings_are_ascii_with_python_escapes() {
        assert_eq!(
            string_literal("a\"b\\c\n\u{1}\u{7f}é😀"),
            "\"a\\\"b\\\\c\\n\\u0001\\u007fé\\ud83d\\ude00\"".replace('é', "\\u00e9")
        );
    }

    #[test]
    fn layouts_match_json_dumps() {
        let value =
            object([("a", Json::Int(1)), ("b", vec![1.5, 2.0].into()), ("c", Json::List(vec![])), ("d", Json::Null)]);
        assert_eq!(dumps(&value), r#"{"a": 1, "b": [1.5, 2.0], "c": [], "d": null}"#);
        assert_eq!(compact(&value), r#"{"a":1,"b":[1.5,2.0],"c":[],"d":null}"#);
        assert_eq!(
            indented(&value),
            "{\n  \"a\": 1,\n  \"b\": [\n    1.5,\n    2.0\n  ],\n  \"c\": [],\n  \"d\": null\n}"
        );
        assert_eq!(dumps(&Json::Float(f64::NAN)), "NaN");
        assert_eq!(dumps(&Json::Float(f64::NEG_INFINITY)), "-Infinity");
    }
}
