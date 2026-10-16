//! The drawings of Tobii's display setup, from the XAML of ScreenPlaneSetupControl
//! (docs/display-setup.md), as path data and a parser into absolute drawing commands.

/// One absolute drawing command.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Segment {
    Move([f64; 2]),
    Line([f64; 2]),
    Cubic([f64; 2], [f64; 2], [f64; 2]),
    Close,
}

fn numbers(text: &str) -> Vec<f64> {
    let mut found = Vec::new();
    let bytes = text.as_bytes();
    let mut at = 0;
    while at < bytes.len() {
        let c = bytes[at];
        if !(c.is_ascii_digit() || c == b'-' || c == b'+' || c == b'.') {
            at += 1;
            continue;
        }
        let start = at;
        at += usize::from(c == b'-' || c == b'+');
        let mut dot = false;
        while at < bytes.len() && (bytes[at].is_ascii_digit() || (bytes[at] == b'.' && !dot)) {
            dot |= bytes[at] == b'.';
            at += 1;
        }
        if let Ok(value) = text[start..at].parse() {
            found.push(value);
        }
    }
    found
}

/// Parses SVG path data with the commands the drawings use (M L H V C S Z, absolute or relative).
pub fn parse_path(data: &str) -> Vec<Segment> {
    let mut segments = Vec::new();
    let mut current = [0.0, 0.0];
    let mut start = [0.0, 0.0];
    let mut last_control: Option<[f64; 2]> = None;
    let commands: Vec<(usize, char)> = data.char_indices().filter(|(_, c)| c.is_ascii_alphabetic()).collect();
    for (position, &(at, command)) in commands.iter().enumerate() {
        let end = commands.get(position + 1).map_or(data.len(), |&(next, _)| next);
        let values = numbers(&data[at + 1..end]);
        let relative = command.is_ascii_lowercase();
        let origin = |current: [f64; 2]| if relative { current } else { [0.0, 0.0] };
        let previous_control = last_control.take();
        match command.to_ascii_uppercase() {
            'M' | 'L' => {
                for (index, pair) in values.as_chunks::<2>().0.iter().enumerate() {
                    let base = origin(current);
                    current = [base[0] + pair[0], base[1] + pair[1]];
                    if command.eq_ignore_ascii_case(&'M') && index == 0 {
                        start = current;
                        segments.push(Segment::Move(current));
                    } else {
                        segments.push(Segment::Line(current));
                    }
                }
            }
            'H' => {
                for x in values {
                    current = [origin(current)[0] + x, current[1]];
                    segments.push(Segment::Line(current));
                }
            }
            'V' => {
                for y in values {
                    current = [current[0], origin(current)[1] + y];
                    segments.push(Segment::Line(current));
                }
            }
            'C' => {
                for six in values.as_chunks::<6>().0 {
                    let base = origin(current);
                    let point = |i: usize| [base[0] + six[i], base[1] + six[i + 1]];
                    let (first, second, end) = (point(0), point(2), point(4));
                    segments.push(Segment::Cubic(first, second, end));
                    current = end;
                    last_control = Some(second);
                }
            }
            'S' => {
                let mut previous = previous_control;
                for four in values.as_chunks::<4>().0 {
                    let base = origin(current);
                    let first = previous.map_or(current, |c| [2.0 * current[0] - c[0], 2.0 * current[1] - c[1]]);
                    let second = [base[0] + four[0], base[1] + four[1]];
                    let end = [base[0] + four[2], base[1] + four[3]];
                    segments.push(Segment::Cubic(first, second, end));
                    current = end;
                    previous = Some(second);
                }
                last_control = previous;
            }
            'Z' => {
                segments.push(Segment::Close);
                current = start;
            }
            _ => {}
        }
    }
    segments
}

/// A drawing: its size and paths of (data, stroke width, filled black inside).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Artwork {
    pub width: f64,
    pub height: f64,
    pub paths: &'static [(&'static str, f64, bool)],
}

/// EyeTracker5.xaml, the detailed tracker above the lines.
pub const TRACKER: Artwork = Artwork {
    width: 812.708,
    height: 72.151,
    paths: &[
        (
            "M3.916,29.994 L17.39,10.54 C20.81,4.632 27.12,0.996 33.947,1 L776.691,1.393 C782.221,1.396 787.073,4.391 790.425,8.79 C796.916,15.994 807.916,28.994 807.916,28.994",
            2.0,
            true,
        ),
        (
            "M361.252,64.799 L24.564,64.543 C11.54,64.533 0.99,54.036 1,41.097 L1,41.097 C1.01,28.159 11.576,17.678 24.6,17.688 L788.144,18.269 C801.168,18.279 811.718,28.776 811.708,41.715 L811.708,41.715 C811.699,54.653 801.132,65.134 788.108,65.124 L451.649,64.868",
            2.0,
            true,
        ),
        ("M150.833,0.916 L145.819,18.081", 2.0, false),
        ("M662.232,1.995 L668.219,18.478", 2.0, false),
        (
            "M64.174,59.831 L24.461,59.802 C14.292,59.795 6.055,51.545 6.063,41.377 L6.063,41.377 C6.072,31.209 14.32,22.973 24.488,22.98 L64.202,23.01 C74.37,23.017 82.607,31.267 82.599,41.435 L82.599,41.435 C82.591,51.603 74.342,59.839 64.174,59.831",
            1.8,
            false,
        ),
        (
            "M788.859,59.831 L749.146,59.802 C738.977,59.795 730.74,51.545 730.749,41.377 L730.749,41.377 C730.757,31.209 739.005,22.973 749.173,22.98 L788.887,23.01 C799.055,23.017 807.292,31.267 807.284,41.435 L807.284,41.435 C807.276,51.603 799.027,59.839 788.859,59.831",
            1.8,
            false,
        ),
        (
            "M354.701,46.928 L354.704,42.908 C354.714,29.834 364.757,19.244 377.136,18.256 L437.609,18.302 C449.986,19.309 460.013,29.915 460.003,42.988 L460,47.008",
            1.8,
            false,
        ),
        (
            "M438.248,72.151 L376.346,72.101 C363.617,72.091 353.306,61.765 353.315,49.036 L353.316,48.128 C353.325,35.398 363.653,25.086 376.383,25.096 L438.285,25.145 C451.014,25.155 461.325,35.482 461.316,48.211 L461.315,49.119 C461.306,61.849 450.978,72.161 438.248,72.151",
            1.8,
            true,
        ),
        (
            "M380.375,48.629 C380.372,52.53 377.197,55.691 373.283,55.688 C369.369,55.685 366.199,52.52 366.202,48.618 C366.205,44.716 369.38,41.556 373.294,41.559 C377.208,41.561 380.378,44.727 380.375,48.629",
            1.8,
            false,
        ),
        ("M407.371,0.713 L407.372,18.004", 2.0, false),
    ],
};

/// EyeTracker5OnScreen.xaml, the flat tracker on the intro's monitor bezel.
pub const FLAT_TRACKER: Artwork = Artwork {
    width: 812.708,
    height: 49.437,
    paths: &[
        (
            "M407.658,48.182 L24.564,47.855 C11.54,47.845 0.99,37.348 1,24.41 L1,24.41 C1.01,11.471 11.576,0.99 24.6,1 L788.144,1.582 C801.168,1.591 811.718,12.088 811.708,25.027 L811.708,25.027 C811.699,37.966 801.132,48.447 788.108,48.437 L407.658,48.182",
            2.0,
            true,
        ),
        (
            "M64.481,43.144 L24.768,43.115 C14.599,43.107 6.362,34.858 6.37,24.689 L6.37,24.689 C6.379,14.522 14.627,6.285 24.795,6.293 L64.509,6.322 C74.677,6.33 82.914,14.579 82.906,24.748 L82.906,24.748 C82.898,34.915 74.649,43.151 64.481,43.144",
            2.0,
            false,
        ),
        (
            "M788.167,43.144 L748.453,43.115 C738.284,43.107 730.047,34.858 730.056,24.689 L730.056,24.689 C730.064,14.522 738.312,6.285 748.48,6.293 L788.194,6.322 C798.362,6.33 806.599,14.579 806.591,24.748 L806.591,24.748 C806.583,34.915 798.334,43.151 788.167,43.144",
            2.0,
            false,
        ),
        (
            "M437.549,48.398 L375.645,48.351 C362.916,48.341 352.605,38.014 352.615,25.285 L352.615,24.38 C352.625,11.652 362.952,1.341 375.68,1.351 L437.585,1.398 C450.314,1.407 460.625,11.734 460.615,24.463 L460.615,25.368 C460.605,38.097 450.278,48.407 437.549,48.398",
            2.0,
            false,
        ),
        (
            "M379.685,24.88 C379.682,28.781 376.507,31.942 372.593,31.939 C368.68,31.936 365.509,28.771 365.512,24.869 C365.515,20.967 368.69,17.806 372.604,17.809 C376.518,17.812 379.688,20.978 379.685,24.88",
            2.0,
            false,
        ),
    ],
};

/// Screen.xaml, the monitor of the intro and of the monitor switcher (143 x 109.5).
pub const MONITOR_WIDTH: f64 = 143.0;
pub const MONITOR_HEIGHT: f64 = 109.5;
pub const MONITOR_BODY: &str = "M140.6 85.8H2.8c-0.6 0-1.1-0.5-1.1-1.1V3.1C1.7 2.5 2.2 2 2.8 2h137.8c0.6 0 1.1 0.5 1.1 1.1v81.6C141.7 85.3 141.2 85.8 140.6 85.8z";
pub const MONITOR_STAND: &str = "M64.2 84.8v14.9c0 0.7-0.5 1.3-1.2 1.3l-23.7 1.8c-0.4 0-0.7 0.4-0.7 0.7v3.3c0 0.4 0.3 0.7 0.7 0.7c0 0 31.4 0.4 32.5 0.4s32.5-0.4 32.5-0.4c0.4 0 0.7-0.3 0.7-0.7v-3.3c0-0.4-0.3-0.7-0.7-0.7l-23.7-1.8c-0.7-0.1-1.2-0.6-1.2-1.3V84.8H64.2z";
/// The black screen inside the monitor: x, y, width, height.
pub const MONITOR_SCREEN: [f64; 4] = [7.3, 7.6, 128.9, 72.5];
/// The monitor switcher: Screen.xaml with its 42 x 84 arrow 30 units beside the monitor.
pub const SWITCHER_CONTENT: [f64; 2] = [42.0 + 30.0 + MONITOR_WIDTH, MONITOR_HEIGHT];

/// The switcher's arrow polygon and the monitor's x offset for the previous (left) or next side.
pub fn switcher_parts(previous: bool) -> ([[f64; 2]; 3], f64) {
    if previous {
        ([[0.0, 42.0], [42.0, 0.0], [42.0, 84.0]], 72.0)
    } else {
        ([[173.0, 0.0], [215.0, 42.0], [173.0, 84.0]], 0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extent(segments: &[Segment]) -> [f64; 4] {
        let mut extent = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
        for segment in segments {
            let points: Vec<[f64; 2]> = match *segment {
                Segment::Move(p) | Segment::Line(p) => vec![p],
                Segment::Cubic(_, _, p) => vec![p],
                Segment::Close => vec![],
            };
            for [x, y] in points {
                extent = [extent[0].min(x), extent[1].min(y), extent[2].max(x), extent[3].max(y)];
            }
        }
        extent
    }

    #[test]
    fn absolute_and_relative_commands() {
        let segments = parse_path("M1,2 L3,4 C5,6 7,8 9,10 Z");
        assert_eq!(
            segments,
            [
                Segment::Move([1.0, 2.0]),
                Segment::Line([3.0, 4.0]),
                Segment::Cubic([5.0, 6.0], [7.0, 8.0], [9.0, 10.0]),
                Segment::Close
            ]
        );
        let relative = parse_path("M10 10h5v-2c-1 0-1.5-.5-2-2l1 1z");
        assert_eq!(relative[1], Segment::Line([15.0, 10.0]));
        assert_eq!(relative[2], Segment::Line([15.0, 8.0]));
        assert_eq!(relative[3], Segment::Cubic([14.0, 8.0], [13.5, 7.5], [13.0, 6.0]));
        assert_eq!(relative[4], Segment::Line([14.0, 7.0]));
        let smooth = parse_path("M0 0C1 1 2 1 3 0s2-1 3 0");
        assert_eq!(smooth[2], Segment::Cubic([4.0, -1.0], [5.0, -1.0], [6.0, 0.0]));
    }

    #[test]
    fn the_drawings_fit_their_frames() {
        let body = extent(&parse_path(MONITOR_BODY));
        for (found, expected) in body.iter().zip([1.7, 2.0, 141.7, 85.8]) {
            assert!((found - expected).abs() < 1e-9, "{body:?}");
        }
        let stand = extent(&parse_path(MONITOR_STAND));
        assert!(stand[3] <= MONITOR_HEIGHT && stand[1] >= 84.0);
        for art in [TRACKER, FLAT_TRACKER] {
            for (data, _, _) in art.paths {
                let [x0, y0, x1, y1] = extent(&parse_path(data));
                assert!(x0 >= 0.0 && y0 >= 0.0 && x1 <= art.width && y1 <= art.height + 0.02, "{data}");
            }
        }
    }
}
