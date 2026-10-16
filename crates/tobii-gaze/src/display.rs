//! Display setup: where the monitor is in the tracker's coordinate frame.
//!
//! Tobii's configuration app computes the display area (top-left, top-right and
//! bottom-left corners in millimetres, tracker frame) from the monitor's physical size,
//! the tracker's mounting geometry (command 2110) and one user adjustment, the horizontal
//! position of the tracker under the screen. It writes the result with
//! tobii_set_display_area, which the runtime turns into command 1440 and stores in
//! screenplane.setpm. docs/display-setup.md has the decompiled sources and the check
//! against the Windows reference host.
//!
//! [`compose_display_area`] follows DisplayAreaUtils.ComposeDisplayArea: a flat rectangle
//! with its bottom edge centred on the origin is shifted by the mounting's external offset
//! plus the tracker offset, tilted about the x axis by the mounting angle and shifted by
//! the internal offset. The app works in double precision; only the corners it hands to
//! Stream Engine are float32 (tools/display_setup_oracle.py).

use std::path::{Path, PathBuf};

use tobii_protocol::Message;
use tobii_protocol::ProtocolError;
use tobii_protocol::commands::{command, header_only_request};
use tobii_protocol::fields::Value;

use crate::device::{self, pack_f32, setting_file};
use crate::error::{GazeError, Result};
use crate::math::{degrees, dist, float32, hypot, py_max, py_min, sum};
use crate::operation::{External, Io, OperationError, OperationResult};

pub type Vector = [f64; 3];

pub const EDID_SIZE: usize = 128;
pub const EDID_HEADER: [u8; 8] = [0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x00];
pub const EDID_SIZE_HINT_OFFSET: usize = 66;
pub const EDID_RESOLUTION_OFFSET: usize = 56;
pub const SIZE_HINT_ASPECT_TOLERANCE: f64 = 0.1;
pub const MM_PER_INCH: f64 = 25.4;
pub const SCREEN_PLANE_FLOATS: usize = 9;
pub const DISPLAY_AREA_TRAILER_VALUE: u32 = 12345;
pub const ET5_TOTAL_WIDTH_MM: f64 = 286.0;
pub const DEFAULT_MONITOR_WIDTH_MM: f64 = 376.3;
pub const DEFAULT_ASPECT: f64 = 16.0 / 9.0;
pub const MIN_LINE: f64 = 0.02;
pub const MAX_LINE: f64 = 0.98;
pub const MIN_LINE_DISTANCE: f64 = 0.05;

/// tobii_geometry_mounting_t, the 2110 reply in wire order.
///
/// The external offset places the display's bottom edge relative to the tracker before the
/// tilt, the internal offset moves the tilted plane into the tracker frame. Stream Engine
/// hands the values to the app as float32.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MountingGeometry {
    pub guides: i64,
    pub width_mm: f64,
    pub angle_deg: f64,
    pub external_offset_mm: Vector,
    pub internal_offset_mm: Vector,
}

impl MountingGeometry {
    pub fn as_float32(&self) -> MountingGeometry {
        MountingGeometry {
            guides: self.guides,
            width_mm: float32(self.width_mm),
            angle_deg: float32(self.angle_deg),
            external_offset_mm: self.external_offset_mm.map(float32),
            internal_offset_mm: self.internal_offset_mm.map(float32),
        }
    }
}

pub const ET5_MOUNTING: MountingGeometry = MountingGeometry {
    guides: 2,
    width_mm: 184.0,
    angle_deg: 20.0,
    external_offset_mm: [0.0, -0.16, 13.85],
    internal_offset_mm: [0.0, 5.38, 9.86],
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DisplayArea {
    pub top_left: Vector,
    pub top_right: Vector,
    pub bottom_left: Vector,
}

impl DisplayArea {
    pub fn width_mm(&self) -> f64 {
        dist(&self.top_left, &self.top_right)
    }

    pub fn height_mm(&self) -> f64 {
        dist(&self.top_left, &self.bottom_left)
    }

    pub fn corners(&self) -> [Vector; 3] {
        [self.top_left, self.top_right, self.bottom_left]
    }

    pub fn floats(&self) -> [f64; SCREEN_PLANE_FLOATS] {
        let mut values = [0.0; SCREEN_PLANE_FLOATS];
        for (index, value) in self.corners().iter().flatten().enumerate() {
            values[index] = *value;
        }
        values
    }

    pub fn from_floats(values: &[f64]) -> Result<DisplayArea> {
        if values.len() != SCREEN_PLANE_FLOATS {
            return Err(GazeError::Value("A display area needs nine numbers: top-left, top-right, bottom-left".into()));
        }
        let corner = |at: usize| [values[at], values[at + 1], values[at + 2]];
        Ok(DisplayArea { top_left: corner(0), top_right: corner(3), bottom_left: corner(6) })
    }

    pub fn as_float32(&self) -> DisplayArea {
        self.map(|corner| corner.map(float32))
    }

    /// DisplayArea.AlmostEquals: every corner within sqrt(0.1) mm.
    pub fn almost_equals(&self, other: &DisplayArea, squared_mm: f64) -> bool {
        self.corners().iter().zip(other.corners()).all(|(mine, theirs)| {
            sum(mine.iter().zip(theirs).map(|(a, b)| {
                let d = a - b;
                d * d
            })) < squared_mm
        })
    }

    fn map(&self, function: impl Fn(Vector) -> Vector) -> DisplayArea {
        DisplayArea {
            top_left: function(self.top_left),
            top_right: function(self.top_right),
            bottom_left: function(self.bottom_left),
        }
    }
}

/// What the app recovers from a stored display area (DisplayAreaUtils.DecomposeDisplayArea).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DisplaySetup {
    pub size_mm: [f64; 2],
    pub tracker_offset_mm: f64,
    pub extra_angle_deg: f64,
}

fn add(a: Vector, b: Vector) -> Vector {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn subtract(a: Vector, b: Vector) -> Vector {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn rotate_about_x([x, y, z]: Vector, radians: f64) -> Vector {
    let (cos, sin) = (radians.cos(), radians.sin());
    [x, y * cos - z * sin, y * sin + z * cos]
}

/// DisplayAreaUtils.FromPhysicalDisplaySize: bottom edge centred on the origin, z = 0.
pub fn flat_display_area([width, height]: [f64; 2]) -> DisplayArea {
    let half = 0.5 * width;
    DisplayArea { top_left: [-half, height, 0.0], top_right: [half, height, 0.0], bottom_left: [-half, 0.0, 0.0] }
}

/// The display area Tobii's display setup writes for a monitor of `size_mm`.
///
/// `tracker_offset_mm` is the horizontal position of the tracker centre relative to the
/// display centre, positive to the right; the app's moving-lines step sets it
/// (EyetrackerMonitorMappingDevice.WriteDisplaySetupToTracker adds it to the mounting's
/// external offset). Corners are float32, as tobii_display_area_t carries them.
pub fn compose_display_area(size_mm: [f64; 2], mounting: &MountingGeometry, tracker_offset_mm: f64) -> DisplayArea {
    let mounting = mounting.as_float32();
    let external = add(mounting.external_offset_mm, [tracker_offset_mm, 0.0, 0.0]);
    let radians = mounting.angle_deg * (std::f64::consts::PI / 180.0);
    flat_display_area(size_mm)
        .map(|p| subtract(p, external))
        .map(|p| rotate_about_x(p, radians))
        .map(|p| add(p, mounting.internal_offset_mm))
        .as_float32()
}

/// Vector3.AngleBetween: acos of the clamped normalized dot product.
fn angle_between(a: Vector, b: Vector) -> f64 {
    let length = py_max(hypot(&a) * hypot(&b), 1e-6);
    let cosine = sum(a.iter().zip(b).map(|(x, y)| x * y)) / length;
    py_max(-1.0, py_min(1.0, cosine)).acos()
}

/// Monitor size, tracker offset and extra tilt that reproduce `area`, as the app reads them back.
pub fn decompose_display_area(area: &DisplayArea, mounting: &MountingGeometry) -> DisplaySetup {
    let mounting = mounting.as_float32();
    let tilt = angle_between(subtract(area.top_left, area.bottom_left), [0.0, 1.0, 0.0]);
    let flat = area
        .map(|p| subtract(p, mounting.internal_offset_mm))
        .map(|p| rotate_about_x(p, -tilt))
        .map(|p| add(p, mounting.external_offset_mm));
    let (width, height) = (area.width_mm(), area.height_mm());
    DisplaySetup {
        size_mm: [width, height],
        tracker_offset_mm: -0.5 * width - flat.bottom_left[0],
        extra_angle_deg: degrees(tilt) - mounting.angle_deg,
    }
}

/// MovingLinesScreenPlaneSetup: monitor size and tracker offset from the two line positions.
///
/// `left` and `right` are where the tracker's ends sit on screen, normalized from the left
/// edge; the app derives the monitor width from the tracker width they span.
pub fn size_from_tracker_lines(
    left: f64,
    right: f64,
    tracker_width_mm: f64,
    aspect_ratio: f64,
) -> Result<([f64; 2], f64)> {
    if !(0.0 <= left && left < right && right <= 1.0) {
        return Err(GazeError::Value("Line positions must satisfy 0 <= left < right <= 1".into()));
    }
    let width = tracker_width_mm / (right - left);
    Ok(([width, width / aspect_ratio], ((left + right) * 0.5 - 0.5) * width))
}

/// Line positions the app starts from for a known size and offset (the inverse of the above).
pub fn tracker_lines(size_mm: [f64; 2], tracker_offset_mm: f64, tracker_width_mm: f64) -> (f64, f64) {
    let span = tracker_width_mm / size_mm[0];
    let centre = tracker_offset_mm / size_mm[0] + 0.5;
    (centre - span * 0.5, centre + span * 0.5)
}

/// EyeTrackerToLineWidthProportion: the tracker picture's width over the line distance.
///
/// GetTotalWidth gives 286 mm for an Eye Tracker 5 with two guides; with no or one guide
/// the picture is as wide as the lines.
pub fn picture_proportion(mounting: &MountingGeometry) -> f64 {
    if mounting.guides >= 2 { ET5_TOTAL_WIDTH_MM / mounting.width_mm } else { 1.0 }
}

/// ScreenPlaneSetupViewModel setters, applied left then right as the app sets them.
pub fn clamp_lines(left: f64, right: f64) -> (f64, f64) {
    let left = py_max(py_min(left, 1.0 - MIN_LINE_DISTANCE), MIN_LINE);
    let right = py_min(py_max(right, left + MIN_LINE_DISTANCE), MAX_LINE);
    (left, right)
}

/// Where the wizard puts the lines first, and the monitor aspect it keeps while they move.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LineSetup {
    pub left: f64,
    pub right: f64,
    pub size_mm: [f64; 2],
    pub tracker_offset_mm: f64,
    pub aspect: f64,
    pub source: &'static str,
}

/// Whether the device holds a display area, the Linux counterpart of Tobii's persisted monitor id.
pub fn is_mapped(device_area: Option<&DisplayArea>) -> bool {
    device_area.is_some_and(|area| area.width_mm() > 0.0 && area.height_mm() > 0.0)
}

/// ScreenPlaneSetup_FullScreenMessage_Instructions, or InstructionsG6Small for a one-guide tracker.
pub fn line_instructions(mounting: &MountingGeometry) -> &'static str {
    if mounting.guides == 1 {
        "Move the lines to the sides of your eye tracker."
    } else {
        "Move the lines to the marks on top of your eye tracker."
    }
}

fn nonzero(values: &[f64]) -> bool {
    values.iter().all(|&v| v != 0.0)
}

/// MovingLinesScreenPlaneSetupFactory.CreateMovingLinesScreenPlaneSetup.
///
/// With a stored display area the size is the EDID hint when there is one, else the
/// decomposed size (DisplaySetupStorage), and the offset comes from the area. Otherwise the
/// width is the hint, if its aspect matches the screen's pixels, or 376.3 mm, the aspect is
/// the screen's, and the tracker starts centred.
pub fn initial_lines(
    mounting: &MountingGeometry,
    device_area: Option<&DisplayArea>,
    size_hint: Option<[f64; 2]>,
    screen_px: Option<[f64; 2]>,
) -> LineSetup {
    let hint = size_hint.filter(|hint| nonzero(hint));
    if let (true, Some(area)) = (is_mapped(device_area), device_area) {
        let stored = decompose_display_area(area, mounting);
        let size = hint.unwrap_or(stored.size_mm);
        let (left, right) = tracker_lines(size, stored.tracker_offset_mm, mounting.width_mm);
        let (left, right) = clamp_lines(left, right);
        let source = if hint.is_some() { "edid size and stored offset" } else { "stored display area" };
        return LineSetup {
            left,
            right,
            size_mm: size,
            tracker_offset_mm: stored.tracker_offset_mm,
            aspect: size[0] / size[1],
            source,
        };
    }
    let usable = size_hint.zip(screen_px).filter(|(hint, px)| size_hint_is_plausible(*hint, *px)).map(|(hint, _)| hint);
    let width = usable.map_or(DEFAULT_MONITOR_WIDTH_MM, |hint| hint[0]);
    let aspect = screen_px.filter(|px| nonzero(px)).map_or(DEFAULT_ASPECT, |px| px[0] / px[1]);
    let size = [width, width / aspect];
    let (left, right) = tracker_lines(size, 0.0, mounting.width_mm);
    let (left, right) = clamp_lines(left, right);
    let source = if usable.is_some() { "edid size estimate" } else { "default width estimate" };
    LineSetup { left, right, size_mm: size, tracker_offset_mm: 0.0, aspect, source }
}

/// What the wizard writes when the user confirms lines at `left` and `right`: size, offset and area.
pub fn area_from_lines(
    left: f64,
    right: f64,
    aspect: f64,
    mounting: &MountingGeometry,
) -> Result<([f64; 2], f64, DisplayArea)> {
    let (size, offset) = size_from_tracker_lines(left, right, mounting.width_mm, aspect)?;
    Ok((size, offset, compose_display_area(size, mounting, offset)))
}

/// Panel size in millimetres from its diagonal and aspect ratio.
pub fn size_from_diagonal(diagonal_inches: f64, [width, height]: [f64; 2]) -> [f64; 2] {
    let diagonal_mm = diagonal_inches * MM_PER_INCH;
    let scale = diagonal_mm / hypot(&[width, height]);
    [width * scale, height * scale]
}

fn edid_base_block(edid: &[u8]) -> Result<()> {
    if edid.len() < EDID_SIZE || !edid.starts_with(&EDID_HEADER) {
        return Err(GazeError::Value("Not an EDID base block".into()));
    }
    Ok(())
}

/// EdidDataInfoReader.TryGetMonitorSizeHintFromEdidData: the first detailed timing's image size.
pub fn edid_size_mm(edid: &[u8]) -> Result<[f64; 2]> {
    edid_base_block(edid)?;
    let [low_width, low_height, high] = [0, 1, 2].map(|i| u32::from(edid[EDID_SIZE_HINT_OFFSET + i]));
    Ok([f64::from((high & 0xF0) << 4 | low_width), f64::from((high & 0x0F) << 8 | low_height)])
}

/// EdidDataInfoReader.TryGetNativeResolutionFromEdidData: the first detailed timing's active pixels.
pub fn edid_native_resolution(edid: &[u8]) -> Result<[u32; 2]> {
    edid_base_block(edid)?;
    let d = |i: usize| u32::from(edid[EDID_RESOLUTION_OFFSET + i]);
    Ok([(d(2) & 0xF0) << 4 | d(0), (d(5) & 0xF0) << 4 | d(3)])
}

/// TryGetScreenSizeHint drops a hint whose aspect ratio differs from the screen's by 0.1 or more.
pub fn size_hint_is_plausible(size_mm: [f64; 2], resolution_px: [f64; 2]) -> bool {
    if !nonzero(&size_mm) || !nonzero(&resolution_px) {
        return false;
    }
    (size_mm[0] / size_mm[1] - resolution_px[0] / resolution_px[1]).abs() < SIZE_HINT_ASPECT_TOLERANCE
}

#[derive(Debug, Clone, PartialEq)]
pub struct DrmMonitor {
    pub connector: String,
    pub status: String,
    pub size_mm: [f64; 2],
    pub resolution_px: [u32; 2],
}

/// `root/*/edid` in name order, as pathlib's glob finds them (hidden entries skipped).
pub(crate) fn edid_paths(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|name| !name.starts_with('.'))
        .collect();
    names.sort();
    names.into_iter().map(|name| root.join(name).join("edid")).filter(|path| path.exists()).collect()
}

/// Monitors with an EDID under the DRM sysfs tree, connected ones first.
pub fn drm_monitors(root: &Path) -> Result<Vec<DrmMonitor>> {
    let mut monitors = Vec::new();
    for edid_path in edid_paths(root) {
        let edid = std::fs::read(&edid_path)?;
        let (Ok(size), Ok(resolution)) = (edid_size_mm(&edid), edid_native_resolution(&edid)) else {
            continue;
        };
        let directory = edid_path.parent().expect("edid has a parent");
        let status_path = directory.join("status");
        let status = if status_path.exists() {
            std::fs::read_to_string(&status_path)?.trim().to_string()
        } else {
            "unknown".to_string()
        };
        let connector = directory.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        monitors.push(DrmMonitor { connector, status, size_mm: size, resolution_px: resolution });
    }
    monitors.sort_by_key(|monitor| monitor.status != "connected");
    Ok(monitors)
}

/// The 1430 reply: three 8001 triples (top-left, top-right, bottom-left) and a trailer array.
pub fn display_area_from_reply(values: &[Value]) -> Result<DisplayArea, ProtocolError> {
    let numbers: Vec<f64> = values
        .iter()
        .filter_map(|value| match value {
            Value::Float(x) => Some(*x),
            _ => None,
        })
        .collect();
    DisplayArea::from_floats(&numbers).map_err(|_| ProtocolError::new("Display area reply must hold three corners"))
}

/// The 2110 reply: guides, width, angle, then the external and internal offset triples.
pub fn mounting_from_reply(values: &[Value]) -> Result<MountingGeometry, ProtocolError> {
    let unexpected = || ProtocolError::new("Unexpected geometry mounting reply");
    let numbers: Vec<&Value> = values.iter().filter(|value| !matches!(value, Value::Struct(_))).collect();
    if numbers.len() != 9 {
        return Err(unexpected());
    }
    let guides = numbers[0].as_int().ok_or_else(unexpected)?;
    let floats: Vec<f64> = numbers[1..]
        .iter()
        .map(|value| match value {
            Value::Float(x) => Some(*x),
            other => other.as_int().map(|n| n as f64),
        })
        .collect::<Option<_>>()
        .ok_or_else(unexpected)?;
    Ok(MountingGeometry {
        guides,
        width_mm: floats[0],
        angle_deg: floats[1],
        external_offset_mm: [floats[2], floats[3], floats[4]],
        internal_offset_mm: [floats[5], floats[6], floats[7]],
    })
}

/// screenplane.setpm bytes: setting header, then bottom-left, top-left, top-right as float32.
pub fn screen_plane_setting(area: &DisplayArea) -> Result<Vec<u8>> {
    let mut payload = Vec::with_capacity(4 * SCREEN_PLANE_FLOATS);
    for corner in [area.bottom_left, area.top_left, area.top_right] {
        for value in corner {
            payload.extend(pack_f32(value)?);
        }
    }
    Ok(setting_file(&payload, 1))
}

pub fn parse_screen_plane_setting(data: &[u8]) -> Result<DisplayArea, ProtocolError> {
    let [bottom_left, top_left, top_right] = device::screen_plane_setting(data)?;
    Ok(DisplayArea { top_left, top_right, bottom_left })
}

pub fn save_display_area(path: &Path, area: &DisplayArea, overwrite: bool) -> Result<PathBuf> {
    if path.exists() && !overwrite {
        return Err(GazeError::file_exists(path));
    }
    if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, screen_plane_setting(area)?)?;
    Ok(path.to_path_buf())
}

/// A screenplane.setpm file (from this tool or Windows), or text holding the nine numbers.
pub fn load_display_area(path: &Path) -> Result<DisplayArea> {
    let data = std::fs::read(path)?;
    if data.len() >= device::SETTING_FILE_HEADER_SIZE && data[..4] == 4u32.to_le_bytes() {
        return Ok(parse_screen_plane_setting(&data)?);
    }
    let text = String::from_utf8(data).map_err(|e| GazeError::Value(e.to_string()))?;
    let numbers = text
        .replace(',', " ")
        .split_whitespace()
        .map(|token| {
            token.parse::<f64>().map_err(|_| GazeError::Value(format!("could not convert string to float: '{token}'")))
        })
        .collect::<Result<Vec<f64>>>()?;
    DisplayArea::from_floats(&numbers)
}

fn request(command: u32) -> Message {
    header_only_request(0, command).expect("header-only command")
}

pub async fn read_display_area<E: External>(io: Io<E>) -> OperationResult<DisplayArea, E> {
    let reply = io.call(request(command::GET_DISPLAY_AREA), None).await.map_err(OperationError::External)?;
    Ok(display_area_from_reply(&reply)?)
}

pub async fn read_mounting<E: External>(io: Io<E>) -> OperationResult<MountingGeometry, E> {
    let reply = io.call(request(command::GET_GEOMETRY_MOUNTING), None).await.map_err(OperationError::External)?;
    Ok(mounting_from_reply(&reply)?)
}

/// 1440 set_display_area with the corners, then 1430 to read back what the device holds.
pub async fn apply_display_area<E: External>(io: Io<E>, area: DisplayArea) -> OperationResult<DisplayArea, E> {
    let message = device::set_display_area(0, area.top_left, area.top_right, area.bottom_left)?;
    io.call(message, None).await.map_err(OperationError::External)?;
    read_display_area(io).await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// albedo's 1430 reply and EDID, from tests/test_display_setup.py.
    const ALBEDO_AREA: DisplayArea = DisplayArea {
        top_left: [-296.6200866699219, 326.0040588378906, 111.8187484741211],
        top_right: [300.3799133300781, 326.0040588378906, 111.8187484741211],
        bottom_left: [-296.6200866699219, 10.267330169677734, -3.100020170211792],
    };

    fn albedo_edid() -> Vec<u8> {
        let mut edid = vec![0u8; 128];
        edid[..8].copy_from_slice(&EDID_HEADER);
        edid[56..62].copy_from_slice(&[0x00, 0x00, 0xA0, 0xA0, 0x00, 0x50]);
        edid[66..69].copy_from_slice(&[0x55, 0x50, 0x21]);
        edid
    }

    #[test]
    fn albedo_area_from_edid_size_and_device_offset_is_exact() {
        let size = edid_size_mm(&albedo_edid()).unwrap();
        assert_eq!(size, [597.0, 336.0]);
        let offset = decompose_display_area(&ALBEDO_AREA, &ET5_MOUNTING).tracker_offset_mm;
        assert_eq!(compose_display_area(size, &ET5_MOUNTING, offset), ALBEDO_AREA);
    }

    #[test]
    fn rounded_offset_leaves_y_and_z_exact() {
        let area = compose_display_area([597.0, 336.0], &ET5_MOUNTING, -1.88);
        for (mine, theirs) in area.corners().iter().zip(ALBEDO_AREA.corners()) {
            assert_eq!(mine[1..], theirs[1..]);
            assert!((mine[0] - theirs[0]).abs() < 0.01);
        }
    }

    #[test]
    fn decompose_recovers_size_offset_and_tilt() {
        let setup = decompose_display_area(&ALBEDO_AREA, &ET5_MOUNTING);
        assert_eq!(setup.size_mm[0], 597.0);
        assert!((setup.size_mm[1] - 336.0).abs() < 0.01);
        assert!((setup.tracker_offset_mm + 1.88).abs() < 0.01);
        assert!(setup.extra_angle_deg.abs() < 0.01);
    }

    #[test]
    fn compose_then_decompose_round_trips() {
        for (size, offset) in [([520.0, 290.0], 3.0), ([800.0, 340.0], -40.0), ([336.0, 597.0], 0.0)] {
            let setup = decompose_display_area(&compose_display_area(size, &ET5_MOUNTING, offset), &ET5_MOUNTING);
            assert!((setup.size_mm[0] - size[0]).abs() < 1e-3);
            assert!((setup.size_mm[1] - size[1]).abs() < 1e-3);
            assert!((setup.tracker_offset_mm - offset).abs() < 1e-3);
            assert!(setup.extra_angle_deg.abs() < 1e-3);
        }
    }

    #[test]
    fn flat_mounting_puts_the_bottom_edge_on_the_offsets() {
        let mounting = MountingGeometry {
            guides: 0,
            width_mm: 184.0,
            angle_deg: 0.0,
            external_offset_mm: [0.0, -10.0, 0.0],
            internal_offset_mm: [0.0, 0.0, 5.0],
        };
        let area = compose_display_area([400.0, 200.0], &mounting, 0.0);
        assert_eq!(area.bottom_left, [-200.0, 10.0, 5.0]);
        assert_eq!(area.top_right, [200.0, 210.0, 5.0]);
    }

    #[test]
    fn lines_round_trip_and_limits() {
        let (left, right) = tracker_lines([597.0, 336.0], -1.88, 184.0);
        let (size, offset) = size_from_tracker_lines(left, right, 184.0, 597.0 / 336.0).unwrap();
        assert!((size[0] - 597.0).abs() < 1e-9 && (size[1] - 336.0).abs() < 1e-9 && (offset + 1.88).abs() < 1e-9);
        assert!(size_from_tracker_lines(0.6, 0.5, 184.0, 1.0).is_err());
        assert_eq!(clamp_lines(0.0, 1.0), (0.02, 0.98));
        assert_eq!(clamp_lines(0.5, 0.51), (0.5, 0.55));
        assert_eq!(clamp_lines(0.99, 0.995), (0.95, 0.98));
        assert_eq!(picture_proportion(&ET5_MOUNTING), 286.0 / 184.0);
    }

    #[test]
    fn initial_lines_reproduce_the_stored_area() {
        let edid = edid_size_mm(&albedo_edid()).unwrap();
        let lines = initial_lines(&ET5_MOUNTING, Some(&ALBEDO_AREA), Some(edid), Some([2560.0, 1440.0]));
        assert_eq!(lines.source, "edid size and stored offset");
        let (_, _, area) = area_from_lines(lines.left, lines.right, lines.aspect, &ET5_MOUNTING).unwrap();
        assert!(area.almost_equals(&ALBEDO_AREA, 0.1));
        let estimate = initial_lines(&ET5_MOUNTING, None, Some([597.0, 336.0]), Some([1280.0, 1024.0]));
        assert_eq!((estimate.source, estimate.size_mm[0], estimate.aspect), ("default width estimate", 376.3, 1.25));
    }

    #[test]
    fn edid_resolution_and_plausibility() {
        assert_eq!(edid_native_resolution(&albedo_edid()).unwrap(), [2560, 1440]);
        assert!(size_hint_is_plausible([597.0, 336.0], [2560.0, 1440.0]));
        assert!(!size_hint_is_plausible([600.0, 0.0], [2560.0, 1440.0]));
        assert!(!size_hint_is_plausible([160.0, 90.0], [1280.0, 1024.0]));
        assert!(edid_size_mm(&[0; 128]).is_err());
    }

    #[test]
    fn size_from_diagonal_of_27_inches() {
        let [width, height] = size_from_diagonal(27.0, [16.0, 9.0]);
        assert!((width - 597.7).abs() < 0.1 && (height - 336.2).abs() < 0.1);
    }

    #[test]
    fn screen_plane_setting_round_trips() {
        let data = screen_plane_setting(&ALBEDO_AREA).unwrap();
        assert_eq!(&data[..12], &[4, 0, 0, 0, 1, 0, 0, 0, 36, 0, 0, 0]);
        assert_eq!(parse_screen_plane_setting(&data).unwrap(), ALBEDO_AREA);
    }

    #[test]
    fn mapped_and_instructions() {
        assert!(is_mapped(Some(&ALBEDO_AREA)));
        assert!(!is_mapped(None));
        assert!(!is_mapped(Some(&DisplayArea { top_left: [0.0; 3], top_right: [0.0; 3], bottom_left: [0.0; 3] })));
        let one_guide = MountingGeometry { guides: 1, ..ET5_MOUNTING };
        assert!(line_instructions(&one_guide).contains("sides"));
    }
}
