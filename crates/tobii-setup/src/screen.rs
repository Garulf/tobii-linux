//! What the flow shows and what the user does on it.

use tobii_gaze::positioning::EyesView;

/// The moving-lines step: where the lines start and how the tracker picture is drawn.
#[derive(Debug, Clone, PartialEq)]
pub struct DisplayLines {
    /// "Where is your eye tracker?" first, as for a tracker without a display setup.
    pub intro: bool,
    pub left: f64,
    pub right: f64,
    pub aspect: f64,
    pub tracker_width_mm: f64,
    pub picture_proportion: f64,
    pub instructions: &'static str,
    /// Whether there is a monitor to the left (previous) or right (next) to move to.
    pub previous: bool,
    pub next: bool,
}

/// One step of the calibration screen.
#[derive(Debug, Clone, PartialEq)]
pub enum CalibrationPhase {
    /// "Let's calibrate for your eyes..." (`welcome`), then "Look at each dot until it explodes!".
    Intro {
        welcome: bool,
        again: bool,
    },
    /// A stage's targets fly out from the centre: (index, x, y) in normalized display units.
    Targets {
        stage: usize,
        targets: Vec<(usize, f64, f64)>,
    },
    Focus {
        index: usize,
        focused: bool,
    },
    /// The collection was discarded; the target returns to rest.
    Missed {
        index: usize,
    },
    /// The target was collected and explodes.
    Pop {
        index: usize,
    },
    Computing {
        stage: usize,
    },
    Computed {
        stage: usize,
    },
    Restart {
        attempt: usize,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum ScreenKind {
    Connecting,
    Busy {
        title: String,
    },
    /// The setup moves to the monitor with key `to`.
    Moving {
        to: String,
    },
    Display(DisplayLines),
    DisplaySave {
        summary: String,
        offset_mm: f64,
        first: bool,
    },
    AspectWarning,
    Positioning {
        sit_upright: bool,
        leaving: bool,
    },
    Calibration(CalibrationPhase),
    CalibrationFailed,
    Profile {
        suggested: String,
        problem: String,
    },
    Finished {
        title: String,
        text: String,
    },
    Cancelled {
        title: String,
        text: String,
    },
    Error {
        title: String,
        text: String,
    },
}

impl ScreenKind {
    pub fn name(&self) -> &'static str {
        match self {
            ScreenKind::Connecting => "connecting",
            ScreenKind::Busy { .. } => "busy",
            ScreenKind::Moving { .. } => "moving",
            ScreenKind::Display(_) => "display",
            ScreenKind::DisplaySave { .. } => "display-save",
            ScreenKind::AspectWarning => "aspect-warning",
            ScreenKind::Positioning { .. } => "positioning",
            ScreenKind::Calibration(_) => "calibration",
            ScreenKind::CalibrationFailed => "calibration-failed",
            ScreenKind::Profile { .. } => "profile",
            ScreenKind::Finished { .. } => "finished",
            ScreenKind::Cancelled { .. } => "cancelled",
            ScreenKind::Error { .. } => "error",
        }
    }

    /// The last screens: Esc and the close button no longer cancel anything.
    pub fn is_end(&self) -> bool {
        matches!(self, ScreenKind::Finished { .. } | ScreenKind::Cancelled { .. } | ScreenKind::Error { .. })
    }

    /// Screens that only work when the window covers the whole monitor.
    pub fn needs_fullscreen(&self) -> bool {
        matches!(self, ScreenKind::Display(_) | ScreenKind::Positioning { .. } | ScreenKind::Calibration(_))
    }

    /// Whether the close button shows: not on the last screens and not over the calibration targets.
    pub fn shows_close(&self) -> bool {
        !self.is_end() && !matches!(self, ScreenKind::Calibration(_))
    }
}

/// A published screen: its sequence number, the monitor it is placed on and what it shows.
#[derive(Debug, Clone, PartialEq)]
pub struct Screen {
    pub seq: u64,
    pub monitor: Option<String>,
    pub kind: ScreenKind,
}

impl Screen {
    pub fn name(&self) -> &'static str {
        self.kind.name()
    }
}

/// What the user did on a screen.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Esc, the close button or Cancel; stands until the flow handles it.
    Cancel,
    /// A key or click on the positioning screen.
    Continue,
    /// Done on the lines, with where the lines are.
    Done {
        left: f64,
        right: f64,
    },
    Previous,
    Next,
    /// Save the new display setup.
    Save,
    /// Keep the old display setup.
    Skip,
    Yes,
    No,
    Retry,
    /// Save the calibration under this profile name.
    SaveProfile {
        name: String,
    },
}

impl Action {
    pub fn name(&self) -> &'static str {
        match self {
            Action::Cancel => "cancel",
            Action::Continue => "continue",
            Action::Done { .. } => "done",
            Action::Previous => "previous",
            Action::Next => "next",
            Action::Save | Action::SaveProfile { .. } => "save",
            Action::Skip => "skip",
            Action::Yes => "yes",
            Action::No => "no",
            Action::Retry => "retry",
        }
    }
}

/// The eye positions the positioning screen draws, updated many times a second.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EyesLive {
    pub eyes: EyesView,
    pub hint: &'static str,
    pub good: bool,
}

/// The window's size, its monitor's size and whether it is fullscreen, as the UI reports them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    pub width: f64,
    pub height: f64,
    pub screen_width: f64,
    pub screen_height: f64,
    /// The monitor in device pixels.
    pub screen_px: [f64; 2],
    pub fullscreen: bool,
}

pub const FULLSCREEN_TOLERANCE_PX: f64 = 2.0;

/// Whether the window covers the whole screen: fullscreen, or a window as large as the screen.
pub fn fills_screen(view: Option<&View>) -> bool {
    view.is_some_and(|view| {
        view.fullscreen
            || (view.width >= view.screen_width - FULLSCREEN_TOLERANCE_PX
                && view.height >= view.screen_height - FULLSCREEN_TOLERANCE_PX)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(width: f64, height: f64, screen_width: f64, screen_height: f64) -> View {
        View { width, height, screen_width, screen_height, screen_px: [screen_width, screen_height], fullscreen: false }
    }

    #[test]
    fn a_window_as_large_as_the_screen_counts_as_fullscreen() {
        assert!(fills_screen(Some(&view(2560.0, 1440.0, 2560.0, 1440.0))));
        assert!(fills_screen(Some(&view(1707.0, 960.0, 1708.0, 961.0))));
        assert!(!fills_screen(Some(&view(2560.0, 1300.0, 2560.0, 1440.0))));
        assert!(fills_screen(Some(&View { fullscreen: true, ..view(10.0, 10.0, 2560.0, 1440.0) })));
        assert!(!fills_screen(None));
    }

    #[test]
    fn close_button_and_fullscreen_rules() {
        let calibration = ScreenKind::Calibration(CalibrationPhase::Computing { stage: 0 });
        assert!(!calibration.shows_close() && calibration.needs_fullscreen());
        assert!(!ScreenKind::Finished { title: String::new(), text: String::new() }.shows_close());
        assert!(ScreenKind::CalibrationFailed.shows_close());
        assert!(!ScreenKind::Connecting.needs_fullscreen());
    }
}
