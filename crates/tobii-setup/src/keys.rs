//! What keys do on each screen (setup.js's keydown handler and Tobii's WindowPresenter).

use crate::alignment::LineKey;
use crate::screen::{Action, ScreenKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Escape,
    Enter,
    Tab,
    Left,
    Right,
    Char(char),
    F11,
    Other,
}

#[derive(Debug, Clone, PartialEq)]
pub enum KeyResponse {
    /// The key means nothing here; let the focused widget have it.
    Pass,
    Send(Action),
    /// The display step's lines handle it.
    Lines(LineKey),
    /// Save the typed profile name.
    SaveProfile,
    /// Close the window after the last screen.
    Close,
    ToggleFullscreen,
    /// Any key on the positioning screen: continue, and ask for fullscreen.
    Continue,
}

pub fn respond(screen: &ScreenKind, key: Key) -> KeyResponse {
    match key {
        Key::F11 => return KeyResponse::ToggleFullscreen,
        Key::Escape if screen.is_end() => return KeyResponse::Close,
        Key::Escape => return KeyResponse::Send(Action::Cancel),
        _ => {}
    }
    match screen {
        ScreenKind::Display(_) => match key {
            Key::Enter => KeyResponse::Lines(LineKey::Enter),
            Key::Tab => KeyResponse::Lines(LineKey::Tab),
            Key::Left => KeyResponse::Lines(LineKey::Left),
            Key::Right => KeyResponse::Lines(LineKey::Right),
            Key::Char('l' | 'L') => KeyResponse::Lines(LineKey::L),
            Key::Char('r' | 'R') => KeyResponse::Lines(LineKey::R),
            _ => KeyResponse::Pass,
        },
        ScreenKind::Positioning { .. } => KeyResponse::Continue,
        _ if key != Key::Enter => KeyResponse::Pass,
        ScreenKind::CalibrationFailed => KeyResponse::Send(Action::Retry),
        ScreenKind::DisplaySave { .. } => KeyResponse::Send(Action::Save),
        ScreenKind::AspectWarning => KeyResponse::Send(Action::Yes),
        ScreenKind::Profile { .. } => KeyResponse::SaveProfile,
        kind if kind.is_end() => KeyResponse::Close,
        _ => KeyResponse::Pass,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen::CalibrationPhase;

    #[test]
    fn escape_cancels_until_the_end() {
        assert_eq!(respond(&ScreenKind::Connecting, Key::Escape), KeyResponse::Send(Action::Cancel));
        let calibrating = ScreenKind::Calibration(CalibrationPhase::Pop { index: 0 });
        assert_eq!(respond(&calibrating, Key::Escape), KeyResponse::Send(Action::Cancel));
        let finished = ScreenKind::Finished { title: String::new(), text: String::new() };
        assert_eq!(respond(&finished, Key::Escape), KeyResponse::Close);
        assert_eq!(respond(&finished, Key::Enter), KeyResponse::Close);
    }

    #[test]
    fn enter_is_the_default_button() {
        assert_eq!(respond(&ScreenKind::CalibrationFailed, Key::Enter), KeyResponse::Send(Action::Retry));
        let save = ScreenKind::DisplaySave { summary: String::new(), offset_mm: 0.0, first: true };
        assert_eq!(respond(&save, Key::Enter), KeyResponse::Send(Action::Save));
        assert_eq!(respond(&ScreenKind::AspectWarning, Key::Enter), KeyResponse::Send(Action::Yes));
        let profile = ScreenKind::Profile { suggested: String::new(), problem: String::new() };
        assert_eq!(respond(&profile, Key::Enter), KeyResponse::SaveProfile);
        assert_eq!(respond(&profile, Key::Char('a')), KeyResponse::Pass);
        assert_eq!(respond(&ScreenKind::CalibrationFailed, Key::Char('x')), KeyResponse::Pass);
    }

    #[test]
    fn positioning_takes_any_key_and_the_lines_take_theirs() {
        let positioning = ScreenKind::Positioning { sit_upright: false, leaving: false };
        assert_eq!(respond(&positioning, Key::Char('x')), KeyResponse::Continue);
        let display = ScreenKind::Display(crate::screen::DisplayLines {
            intro: false,
            left: 0.4,
            right: 0.6,
            aspect: 1.0,
            tracker_width_mm: 184.0,
            picture_proportion: 1.0,
            instructions: "",
            previous: false,
            next: false,
        });
        assert_eq!(respond(&display, Key::Char('r')), KeyResponse::Lines(LineKey::R));
        assert_eq!(respond(&display, Key::Tab), KeyResponse::Lines(LineKey::Tab));
        assert_eq!(respond(&display, Key::Char('x')), KeyResponse::Pass);
        assert_eq!(respond(&display, Key::F11), KeyResponse::ToggleFullscreen);
    }
}
