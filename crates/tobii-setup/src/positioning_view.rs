//! The eye positioning screen as a timeline (EyesPositioningView, docs/setup-flow.md): a
//! faint track box of golden ratio, the eyes as circles inside it, "These are your eyes..."
//! (0.5 to 3 s), the eyes fading in (3 to 4 s), "See how much you can move around." (4 to
//! 7 s), hints from 7 s. Leaving, the right eye blinks (0.4 s) and the box fades out (1 s).

use tobii_gaze::positioning::EyeView;

use crate::motion::{progress, timeline};

pub const WELCOME_ONE: &str = "These are your eyes...";
pub const WELCOME_TWO: &str = "See how much you can move around.";
pub const SIT_UPRIGHT: &str = "Sit in an upright position in front of the screen.";
pub const GOOD: &str = "Good. Stay like this, the calibration starts in a moment, or press any key.";
pub const BLINK_S: f64 = 0.4;
pub const FADE_OUT_S: f64 = 1.0;
pub const CURSOR_HIDE_S: f64 = 2.0;

/// The track box: height min(width, height) / 2.25, width 1.618 times that, centred.
pub fn track_box(width: f64, height: f64) -> [f64; 4] {
    let box_height = (width.min(height) / 2.25).floor();
    let box_width = (box_height * 1.618).floor();
    [(width - box_width) / 2.0, (height - box_height) / 2.0, box_width, box_height]
}

/// An eye as drawn in a box of `box_width` x `box_height`: centre, diameter, grey, opacity.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EyeDraw {
    pub centre: [f64; 2],
    pub diameter: f64,
    pub grey: f64,
    pub opacity: f64,
}

pub fn eye_draw(eye: &EyeView, size: i64, box_width: f64, box_height: f64, elapsed_s: f64) -> EyeDraw {
    let diameter = (size as f64 * box_height / 640.0).max(4.0);
    let reveal = (elapsed_s - 3.0).clamp(0.0, 1.0);
    EyeDraw {
        centre: [eye.x * box_width, eye.y * box_height],
        diameter,
        grey: eye.grey as f64 / 255.0,
        opacity: eye.opacity * reveal,
    }
}

/// Text opacities `elapsed_s` after the screen appeared: (welcome one, welcome two, hint).
pub fn text_opacities(elapsed_s: f64) -> (f64, f64, f64) {
    let one = timeline(elapsed_s, 0.0, 3.0, &[(0.0, 0.0), (1.0 / 6.0, 0.0), (0.5, 1.0), (2.0 / 3.0, 1.0), (1.0, 0.0)]);
    let two = timeline(elapsed_s, 4.0, 3.0, &[(0.0, 0.0), (1.0 / 3.0, 1.0), (2.0 / 3.0, 1.0), (1.0, 0.0)]);
    let hint = timeline(elapsed_s, 7.0, 1.0, &[(0.0, 0.0), (1.0, 1.0)]);
    (one, two, hint)
}

/// The Linux addition: "Good..." once the position is good and the eyes are shown.
pub fn status_text(good: bool, elapsed_s: f64) -> &'static str {
    if good && elapsed_s > 3.0 { GOOD } else { "" }
}

/// While leaving: the right eye's vertical scale (the blink) and the box's opacity.
pub fn leaving(now: f64, since: f64) -> (f64, f64) {
    let blink = progress(now, since, BLINK_S);
    let fade = 1.0 - progress(now, since + BLINK_S, FADE_OUT_S);
    (blink, fade)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_box_has_the_golden_ratio() {
        assert_eq!(track_box(1280.0, 720.0), [381.5, 200.0, 517.0, 320.0]);
    }

    #[test]
    fn texts_follow_the_intro() {
        assert_eq!(text_opacities(0.2), (0.0, 0.0, 0.0));
        assert_eq!(text_opacities(1.75).0, 1.0);
        assert_eq!(text_opacities(5.5).1, 1.0);
        assert_eq!(text_opacities(9.0), (0.0, 0.0, 1.0));
        assert_eq!(status_text(true, 2.0), "");
        assert_eq!(status_text(true, 4.0), GOOD);
    }

    #[test]
    fn eyes_fade_in_after_three_seconds() {
        let eye = EyeView { x: 0.5, y: 0.25, grey: 255, opacity: 1.0 };
        let drawn = eye_draw(&eye, 50, 517.0, 320.0, 3.5);
        assert_eq!((drawn.centre, drawn.diameter, drawn.opacity), ([258.5, 80.0], 25.0, 0.5));
        assert_eq!(leaving(1.0, 1.0), (0.0, 1.0));
        assert_eq!(leaving(1.9, 1.0), (1.0, 0.5));
    }
}
