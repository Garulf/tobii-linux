//! Easing curves and keyframe timelines of the setup's animations, as the page's CSS and
//! Web Animations define them, evaluated at a time instead of run by a browser.

/// CSS `cubic-bezier(x1, y1, x2, y2)` at progress `t` in 0..1.
pub fn cubic_bezier(x1: f64, y1: f64, x2: f64, y2: f64, t: f64) -> f64 {
    if t <= 0.0 {
        return 0.0;
    }
    if t >= 1.0 {
        return 1.0;
    }
    let curve = |a: f64, b: f64, s: f64| {
        let r = 1.0 - s;
        3.0 * r * r * s * a + 3.0 * r * s * s * b + s * s * s
    };
    let (mut low, mut high) = (0.0, 1.0);
    for _ in 0..60 {
        let middle = 0.5 * (low + high);
        if curve(x1, x2, middle) < t {
            low = middle;
        } else {
            high = middle;
        }
    }
    curve(y1, y2, 0.5 * (low + high))
}

pub fn ease_quadratic_in_out(t: f64) -> f64 {
    if t < 0.5 { 2.0 * t * t } else { 1.0 - (-2.0 * t + 2.0).powi(2) / 2.0 }
}

/// WPF BackEase (amplitude 1) in EaseInOut mode.
pub fn ease_back_in_out(t: f64) -> f64 {
    let ease_in = |x: f64| x * x * x - x * (std::f64::consts::PI * x).sin();
    if t < 0.5 { ease_in(2.0 * t) / 2.0 } else { (1.0 - ease_in(2.0 - 2.0 * t)) / 2.0 + 0.5 }
}

/// A value along keyframes (offset in 0..1, value), linear between them, held outside.
pub fn keyframes(progress: f64, frames: &[(f64, f64)]) -> f64 {
    let Some(&(first_offset, first)) = frames.first() else {
        return 0.0;
    };
    if progress <= first_offset {
        return first;
    }
    for pair in frames.windows(2) {
        let ((a, va), (b, vb)) = (pair[0], pair[1]);
        if progress <= b {
            return if b > a { va + (vb - va) * (progress - a) / (b - a) } else { vb };
        }
    }
    frames[frames.len() - 1].1
}

/// Keyframes over `duration_s` starting `delay_s` after time 0, holding the first value before
/// and the last after (Web Animations with fill both).
pub fn timeline(elapsed_s: f64, delay_s: f64, duration_s: f64, frames: &[(f64, f64)]) -> f64 {
    keyframes((elapsed_s - delay_s) / duration_s, frames)
}

/// Linear progress of an animation from `start_s` lasting `duration_s`, clamped to 0..1.
pub fn progress(now_s: f64, start_s: f64, duration_s: f64) -> f64 {
    ((now_s - start_s) / duration_s).clamp(0.0, 1.0)
}

/// The page's fade-in of text and buttons: invisible for a third of 1.5 s, then fading in.
pub fn fade_in(elapsed_s: f64) -> f64 {
    timeline(elapsed_s, 0.0, 1.5, &[(0.0, 0.0), (1.0 / 3.0, 0.0), (1.0, 1.0)])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curves_start_at_zero_and_end_at_one() {
        for curve in [ease_quadratic_in_out, ease_back_in_out] {
            assert!(curve(0.0).abs() < 1e-12 && (curve(1.0) - 1.0).abs() < 1e-12);
        }
        assert!(ease_back_in_out(0.1) < 0.0, "the back ease first moves away");
        assert!((cubic_bezier(0.0, 0.0, 1.0, 1.0, 0.3) - 0.3).abs() < 1e-9);
        assert!(cubic_bezier(0.075, 0.82, 0.165, 1.0, 0.2) > 0.6);
    }

    #[test]
    fn keyframes_interpolate_and_hold() {
        let frames = [(0.0, 0.0), (0.2, 1.0), (0.8, 1.0), (1.0, 0.0)];
        assert_eq!(keyframes(-1.0, &frames), 0.0);
        assert!((keyframes(0.1, &frames) - 0.5).abs() < 1e-12);
        assert_eq!(keyframes(0.5, &frames), 1.0);
        assert_eq!(keyframes(2.0, &frames), 0.0);
        assert_eq!(timeline(4.0, 4.0, 3.0, &[(0.0, 0.0), (1.0, 1.0)]), 0.0);
        assert_eq!(fade_in(0.4), 0.0);
        assert_eq!(fade_in(1.5), 1.0);
    }
}
