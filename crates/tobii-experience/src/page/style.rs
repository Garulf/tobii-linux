//! Tobii's page styles (docs/experience-app.md section 5) as GTK CSS, and the page's icons
//! from their WPF path data (section 5.8).

/// Tobii's `TobiiFontFamily` is Segoe UI Light; Linux takes Selawik Light, Microsoft's open
/// substitute, then the setup app's fallbacks.
pub const FONT_FAMILY: &str =
    r#""Selawik Light", "Segoe UI Light", "Segoe UI", "Selawik", "Open Sans", "Noto Sans", "DejaVu Sans", sans-serif"#;

/// The stylesheet for windows with the `tobii-page` class.
///
/// GTK's min sizes are content-box sizes, so Tobii's border-box minimums lose the padding
/// and border (the `GazeEnabledButton`'s 150 x 33 becomes 80 x 17 inside its 35/8 padding).
/// GTK has no `Viewbox`, so a few values are adapted: the `GazeEnabledButton` pill is the
/// whole button drawn at Tobii's scale, with its text enlarged by 1/0.95 to keep 12 pt; the
/// toggle knob's margins are taken inside the 1 px track border.
pub fn css() -> String {
    format!(
        r#"
window.tobii-page {{ background-color: #151617; }}
window.tobii-page label {{ font-family: {FONT_FAMILY}; font-weight: 300; font-size: 12pt; color: #BFD6E0; }}
window.tobii-page label.h3 {{ font-size: 16pt; color: #FFFFFF; }}
window.tobii-page label.body {{ font-size: 12pt; color: #D5D5D5; }}
window.tobii-page label.warning {{ color: #FFB631; }}

window.tobii-page button.tobii-toggle {{ padding: 0; margin: 0; min-width: 0; min-height: 0; border: none;
  border-radius: 0; background: none; box-shadow: none; outline: none; }}
window.tobii-page .tobii-toggle .track {{ min-width: 40px; min-height: 17px; border: 1px solid #D9F4FF;
  border-radius: 10px; background-color: transparent; transition: all 100ms; }}
window.tobii-page .tobii-toggle:checked .track {{ background-color: #008DBF; border-color: transparent; }}
window.tobii-page .tobii-toggle .knob {{ min-width: 10px; min-height: 10px; border-radius: 5px;
  background-color: #D9F4FF; margin-left: 3px; transition: all 100ms; }}
window.tobii-page .tobii-toggle:checked .knob {{ margin-left: 24px; }}
window.tobii-page .tobii-toggle label.state {{ font-size: 10pt; color: #AFC4CD; margin-left: 10px;
  min-width: 65px; transition: color 100ms; }}
window.tobii-page .tobii-toggle:hover .knob {{ background-color: #FFFFFF; transform: scale(1.07); }}
window.tobii-page .tobii-toggle:hover .track {{ border-color: #FFFFFF; background-color: #637279; }}
window.tobii-page .tobii-toggle:hover:checked .track {{ border-color: transparent; background-color: #00ACE9; }}
window.tobii-page .tobii-toggle:hover label.state {{ color: #FFFFFF; }}
window.tobii-page .tobii-toggle:active .knob {{ background-color: #FFFFFF; transform: scale(0.93); }}
window.tobii-page .tobii-toggle:active .track {{ border-color: #FFFFFF; background-color: #AFC4CD; }}
window.tobii-page .tobii-toggle:active:checked .track {{ border-color: transparent; background-color: #00BEFF; }}
window.tobii-page .tobii-toggle:active label.state {{ color: #FFFFFF; }}
window.tobii-page .tobii-toggle:disabled .knob {{ background-color: #637279; transform: none; }}
window.tobii-page .tobii-toggle:disabled .track {{ border-color: #637279; background-color: transparent; }}
window.tobii-page .tobii-toggle:disabled:checked .track {{ border-color: transparent; background-color: #2B3940; }}
window.tobii-page .tobii-toggle:disabled label.state {{ color: #2B3940; }}

window.tobii-page button.tobii-gaze-button {{ min-width: 80px; min-height: 17px; padding: 8px 35px;
  margin: 10px 0 15px -5px; border: none; border-radius: 9999px; box-shadow: none; outline: none;
  background-image: none; background-color: #004B66; color: #D9F4FF; transform: scale(0.95);
  transition: background-color 100ms, color 100ms, transform 100ms; }}
window.tobii-page button.tobii-gaze-button label {{ font-size: 12.63pt; color: inherit; }}
window.tobii-page button.tobii-gaze-button:hover {{ background-color: #018EC1; color: #FFFFFF;
  transform: scale(0.975); }}
window.tobii-page button.tobii-gaze-button:active {{ background-color: #00BEFF; color: #FFFFFF;
  transform: scale(0.925); transition: color 100ms, transform 100ms; }}
window.tobii-page button.tobii-gaze-button:disabled {{ background-color: #2B3940; color: #637279;
  transform: scale(0.95); }}

window.tobii-page .tobii-banner {{ background-color: #000000; }}
window.tobii-page button.tobii-action-button {{ min-width: 70px; min-height: 33px; padding: 0 25px; border: none;
  border-radius: 9999px; box-shadow: none; outline: none; background-image: none; background-color: #008DBF;
  color: #D9F4FF; transition: background-color 100ms, color 100ms; }}
window.tobii-page button.tobii-action-button label {{ font-size: 10pt; color: inherit; }}
window.tobii-page button.tobii-action-button:hover {{ background-color: #00ACE9; color: #FFFFFF; }}
window.tobii-page button.tobii-action-button:active {{ background-color: #00BEFF; color: #FFFFFF; transition: none; }}

window.tobii-page button.tobii-accent {{ padding: 0; min-width: 0; min-height: 0; border: 1px solid transparent;
  border-radius: 0; background: none; box-shadow: none; outline: none; color: #008DBF; }}
window.tobii-page button.tobii-accent:hover:not(:active) {{ color: #00ACE9; }}
window.tobii-page button.tobii-accent:disabled {{ color: #6E6E6E; }}
window.tobii-page button.tobii-accent:active > * {{ transform: translateY(1px); }}
window.tobii-page button.tobii-accent label {{ color: inherit; }}
window.tobii-page button.tobii-undo {{ margin: 3px 0; padding: 15px; }}
window.tobii-page button.tobii-trash {{ padding: 15px; }}
window.tobii-page .tobii-profile-row {{ min-height: 60px; }}

window.tobii-page checkbutton.tobii-radio {{ min-height: 40px; padding: 0; }}
window.tobii-page checkbutton.tobii-radio radio {{ -gtk-icon-source: none; min-width: 10px; min-height: 10px;
  padding: 4px; margin: 2px 3px 2px 2px; border: 1px solid #D9F4FF; border-radius: 50%; box-shadow: none;
  background-color: transparent; background-clip: content-box, padding-box;
  background-image: linear-gradient(transparent, transparent), linear-gradient(transparent, transparent);
  transition: all 100ms; }}
window.tobii-page checkbutton.tobii-radio radio:checked {{
  background-image: linear-gradient(#008DBF, #008DBF), linear-gradient(transparent, transparent); }}
window.tobii-page checkbutton.tobii-radio label {{ font-size: 10pt; color: #AFC4CD; margin: 0 5px; }}
window.tobii-page checkbutton.tobii-radio:hover radio {{ border-color: #FFFFFF; transform: scale(1.05);
  background-image: linear-gradient(transparent, transparent), linear-gradient(#637279, #637279); }}
window.tobii-page checkbutton.tobii-radio:hover radio:checked {{
  background-image: linear-gradient(#00ACE9, #00ACE9), linear-gradient(#637279, #637279); }}
window.tobii-page checkbutton.tobii-radio:hover label {{ color: #FFFFFF; }}
window.tobii-page checkbutton.tobii-radio:active radio {{ border-color: #FFFFFF; transform: scale(0.95);
  background-image: linear-gradient(transparent, transparent), linear-gradient(#AFC4CD, #AFC4CD); }}
window.tobii-page checkbutton.tobii-radio:active radio:checked {{
  background-image: linear-gradient(#00BEFF, #00BEFF), linear-gradient(#AFC4CD, #AFC4CD); }}
window.tobii-page checkbutton.tobii-radio:active label {{ color: #FFFFFF; }}
window.tobii-page checkbutton.tobii-radio:disabled radio {{ border-color: #637279; transform: none;
  background-image: linear-gradient(transparent, transparent), linear-gradient(transparent, transparent); }}
window.tobii-page checkbutton.tobii-radio:disabled radio:checked {{
  background-image: linear-gradient(#2B3940, #2B3940), linear-gradient(transparent, transparent); }}
window.tobii-page checkbutton.tobii-radio:disabled label {{ color: #2B3940; }}

window.tobii-page scrollbar, window.tobii-page scrollbar range, window.tobii-page scrollbar trough {{
  background: none; border: none; box-shadow: none; padding: 0; margin: 0; }}
window.tobii-page scrollbar slider {{ min-width: 3px; min-height: 30px; margin: 0 0 0 12px; border: none;
  border-radius: 0; background-color: #004B66;
  transition: min-width 500ms linear 1s, margin 500ms linear 1s; }}
window.tobii-page scrollbar:hover slider {{ min-width: 15px; margin-left: 0;
  transition: min-width 100ms, margin 100ms; }}
"#
    )
}

/// How an icon path is painted with the foreground of the button around it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Paint {
    Fill,
    /// An unfilled stroke of this thickness with round joins.
    Stroke(f64),
}

/// One path of an icon, in its canvas coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IconPath {
    pub data: &'static str,
    pub paint: Paint,
}

/// A WPF `Viewbox` around a `Canvas`: the canvas is scaled uniformly to the drawn size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Icon {
    pub width: f64,
    pub height: f64,
    pub paths: &'static [IconPath],
}

const HEAD_AND_SHOULDERS: [IconPath; 2] = [
    IconPath {
        data: "F1 M0.76,29.9067 C0.76,29.9067 2.25467,19.7707 10.8947,19.7707 C19.536,19.7707 21.0307,29.9067 \
               21.0307,29.9067",
        paint: Paint::Stroke(1.52),
    },
    IconPath {
        data: "F1 M17.936,12.9427 C17.936,16.7133 14.8787,19.7707 11.108,19.7707 C7.33733,19.7707 4.28,16.7133 \
               4.28,12.9427 C4.28,9.172 7.33733,6.11466 11.108,6.11466 C14.8787,6.11466 17.936,9.172 17.936,12.9427",
        paint: Paint::Stroke(1.52),
    },
];

pub const DOWN_ARROW: Icon =
    Icon { width: 40.0, height: 20.0, paths: &[IconPath { data: "M0 0 L40 0 L20 20 L0 0", paint: Paint::Fill }] };
pub const RIGHT_ARROW: Icon =
    Icon { width: 20.0, height: 40.0, paths: &[IconPath { data: "M0 0 L20 20 L0 40 L0 0", paint: Paint::Fill }] };
/// The stroked paths keep their absolute coordinates: Tobii's `Canvas.Left`/`Top` plus half
/// the stroke thickness land within 0.07 px of them, so `Stretch=Fill` changes nothing.
pub const NEW_PROFILE: Icon = Icon {
    width: 28.0,
    height: 30.7,
    paths: &[
        HEAD_AND_SHOULDERS[0],
        IconPath { data: "F1 M27.7493,4.94266 L19.212,4.94266", paint: Paint::Stroke(1.28) },
        IconPath { data: "F1 M23.484,0.677332 L23.484,9.208", paint: Paint::Stroke(1.28) },
        HEAD_AND_SHOULDERS[1],
    ],
};
pub const PROFILE: Icon = Icon { width: 28.0, height: 30.7, paths: &HEAD_AND_SHOULDERS };
pub const TRASH: Icon = Icon {
    width: 48.0,
    height: 48.0,
    paths: &[IconPath {
        data: "M10 9v2h28V9H10z M30 5H18v2h12V5z M12 38c0 2.2 1.8 4 4 4h16c2.2 0 4-1.8 4-4V13H12V38z M14 15h20v23  \
               c0 1.1-0.9 2-2 2H16c-1.1 0-2-0.9-2-2V15z",
        paint: Paint::Fill,
    }],
};

/// An absolute path segment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Segment {
    Move(f64, f64),
    Line(f64, f64),
    Cubic([f64; 6]),
    Close,
}

enum Token {
    Command(char),
    Number(f64),
}

fn tokens(data: &str) -> Result<Vec<Token>, String> {
    let chars: Vec<char> = data.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() || c == ',' {
            i += 1;
        } else if c.is_ascii_alphabetic() {
            tokens.push(Token::Command(c));
            i += 1;
        } else if c.is_ascii_digit() || matches!(c, '.' | '-' | '+') {
            let start = i;
            let mut seen_dot = false;
            i += 1;
            if c == '.' {
                seen_dot = true;
            }
            while i < chars.len() && (chars[i].is_ascii_digit() || (chars[i] == '.' && !seen_dot)) {
                seen_dot |= chars[i] == '.';
                i += 1;
            }
            let text: String = chars[start..i].iter().collect();
            tokens.push(Token::Number(text.parse().map_err(|_| format!("bad number {text:?} in {data:?}"))?));
        } else {
            return Err(format!("unexpected {c:?} in {data:?}"));
        }
    }
    Ok(tokens)
}

/// Parses WPF path markup (`F` fill rule, `M L H V C Z`, absolute and relative) into
/// absolute segments.
pub fn parse_path(data: &str) -> Result<Vec<Segment>, String> {
    let tokens = tokens(data)?;
    let mut segments = Vec::new();
    let (mut x, mut y, mut start) = (0.0, 0.0, (0.0, 0.0));
    let mut command = None;
    let mut i = 0;
    let numbers = |i: &mut usize, count: usize| -> Result<Vec<f64>, String> {
        let mut values = Vec::with_capacity(count);
        for _ in 0..count {
            match tokens.get(*i) {
                Some(Token::Number(value)) => values.push(*value),
                _ => return Err(format!("missing number in {data:?}")),
            }
            *i += 1;
        }
        Ok(values)
    };
    while i < tokens.len() {
        if let Token::Command(c) = tokens[i] {
            command = Some(c);
            i += 1;
            if matches!(c, 'Z' | 'z') {
                segments.push(Segment::Close);
                (x, y) = start;
                continue;
            }
            if c == 'F' {
                numbers(&mut i, 1)?;
                command = None;
                continue;
            }
        }
        let Some(c) = command else { return Err(format!("a number without a command in {data:?}")) };
        let relative = c.is_ascii_lowercase();
        let (dx, dy) = if relative { (x, y) } else { (0.0, 0.0) };
        match c.to_ascii_uppercase() {
            'M' => {
                let v = numbers(&mut i, 2)?;
                (x, y) = (v[0] + dx, v[1] + dy);
                start = (x, y);
                segments.push(Segment::Move(x, y));
                command = Some(if relative { 'l' } else { 'L' });
            }
            'L' => {
                let v = numbers(&mut i, 2)?;
                (x, y) = (v[0] + dx, v[1] + dy);
                segments.push(Segment::Line(x, y));
            }
            'H' => {
                x = numbers(&mut i, 1)?[0] + dx;
                segments.push(Segment::Line(x, y));
            }
            'V' => {
                y = numbers(&mut i, 1)?[0] + dy;
                segments.push(Segment::Line(x, y));
            }
            'C' => {
                let v = numbers(&mut i, 6)?;
                let points = [v[0] + dx, v[1] + dy, v[2] + dx, v[3] + dy, v[4] + dx, v[5] + dy];
                (x, y) = (points[4], points[5]);
                segments.push(Segment::Cubic(points));
            }
            other => return Err(format!("unsupported command {other:?} in {data:?}")),
        }
    }
    Ok(segments)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn css_uses_tobiis_values() {
        let css = css();
        assert!(css.contains("window.tobii-page { background-color: #151617; }"));
        assert!(css.contains(r#"font-family: "Selawik Light", "Segoe UI Light""#));
        assert!(css.contains("label.h3 { font-size: 16pt; color: #FFFFFF; }"));
        assert!(css.contains("label.body { font-size: 12pt; color: #D5D5D5; }"));
        assert!(
            css.contains("button.tobii-gaze-button { min-width: 80px; min-height: 17px; padding: 8px 35px;"),
            "Tobii's 150 x 33 border box less the 35/8 padding, as GTK content-box minimums"
        );
    }

    #[test]
    fn every_icon_parses() {
        for icon in [DOWN_ARROW, RIGHT_ARROW, NEW_PROFILE, PROFILE, TRASH] {
            for path in icon.paths {
                assert!(!parse_path(path.data).unwrap().is_empty(), "{}", path.data);
            }
        }
    }

    #[test]
    fn paths_resolve_relative_commands() {
        use Segment::{Close, Cubic, Line, Move};
        assert_eq!(
            parse_path("M10 9v2h28V9H10z M14 15h20v23  c0 1.1-0.9 2-2 2H16").unwrap(),
            [
                Move(10.0, 9.0),
                Line(10.0, 11.0),
                Line(38.0, 11.0),
                Line(38.0, 9.0),
                Line(10.0, 9.0),
                Close,
                Move(14.0, 15.0),
                Line(34.0, 15.0),
                Line(34.0, 38.0),
                Cubic([34.0, 39.1, 33.1, 40.0, 32.0, 40.0]),
                Line(16.0, 40.0),
            ]
        );
        assert_eq!(
            parse_path("F1 M27.7493,4.94266 L19.212,4.94266").unwrap(),
            [Move(27.7493, 4.94266), Line(19.212, 4.94266)]
        );
        assert!(parse_path("M0 0 Q1 1 2 2").is_err());
        assert!(parse_path("M0").is_err());
    }
}
