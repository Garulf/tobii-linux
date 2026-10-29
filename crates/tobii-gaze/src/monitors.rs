//! The monitors of the desktop the setup runs on, and Tobii's rules for moving between them.
//!
//! Tobii's display setup (VirtualScreen, MonitorMappingModel; docs/setup-flow.md) starts on
//! the only monitor, else on the one the tracker was mapped to, else on the primary one
//! (the monitor at 0, 0). Its "previous" monitor is the one whose left edge is the nearest
//! to the left of the current one's, "next" the nearest to the right; only the left edge
//! counts.
//!
//! Sources, all read only: `xrandr --prop` under X11 (geometry, rotation, primary flag and
//! EDID of each connected output), else the EDIDs under /sys/class/drm without geometry,
//! and under WSL the Windows desktop as printed by the PowerShell query of the Python
//! module (screen bounds in physical pixels, device instance id and registry EDID).

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::display::{edid_native_resolution, edid_paths, edid_size_mm};
use crate::error::{GazeError, Result};

pub const EDID_ID: std::ops::Range<usize> = 8..18;
pub const QUERY_TIMEOUT_S: f64 = 20.0;

/// One monitor: `key` names it stably (Windows instance id, else the output name).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Monitor {
    pub key: String,
    pub name: String,
    /// x, y, width, height in pixels.
    pub bounds: Option<[i64; 4]>,
    pub primary: bool,
    pub rotation: u32,
    pub edid: Option<Vec<u8>>,
    pub instance_id: Option<String>,
    /// DRM connector under /sys/class/drm, such as `card0-DP-1`.
    pub connector: Option<String>,
}

impl Monitor {
    pub fn left(&self) -> Option<i64> {
        self.bounds.map(|bounds| bounds[0])
    }

    pub fn resolution_px(&self) -> Option<[i64; 2]> {
        self.bounds.map(|bounds| [bounds[2], bounds[3]])
    }

    /// The EDID size hint in the orientation the monitor is used in.
    pub fn size_mm(&self) -> Option<[f64; 2]> {
        let [width, height] = match &self.edid {
            Some(edid) => edid_size_mm(edid).ok()?,
            None => [0.0, 0.0],
        };
        if width == 0.0 || height == 0.0 {
            return None;
        }
        Some(if matches!(self.rotation, 90 | 270) { [height, width] } else { [width, height] })
    }

    pub fn native_resolution(&self) -> Option<[u32; 2]> {
        edid_native_resolution(self.edid.as_ref()?).ok()
    }

    /// EDID bytes 8 to 17 (manufacturer, product, serial, week, year) in hex, as Tobii's display id keeps them.
    pub fn edid_id(&self) -> Option<String> {
        let edid = self.edid.as_ref().filter(|edid| edid.len() >= EDID_ID.end)?;
        Some(edid[EDID_ID].iter().map(|b| format!("{b:02X}")).collect())
    }

    /// Tobii's display id string (displayinfo.setpm) when the Windows instance id is known.
    pub fn display_id(&self) -> Option<String> {
        let instance_id = self.instance_id.as_ref().filter(|id| !id.is_empty())?;
        let rotation = match self.rotation {
            0 => 1,
            90 => 2,
            180 => 3,
            270 => 4,
            _ => return None,
        };
        Some(format!("{instance_id}##eId:{}#or:{rotation}#v:1.0", self.edid_id().unwrap_or_default()))
    }
}

fn usable(size: Option<[f64; 2]>) -> Option<[f64; 2]> {
    size.filter(|[a, b]| *a != 0.0 && *b != 0.0)
}

/// AspectRatio.IsSameIgnoreOrientation; true when either size is unknown.
pub fn same_aspect_ignoring_orientation(a: Option<[f64; 2]>, b: Option<[f64; 2]>, tolerance: f64) -> bool {
    let (Some(a), Some(b)) = (usable(a), usable(b)) else {
        return true;
    };
    let (ratio, other) = (a[0] / a[1], b[0] / b[1]);
    (ratio - other).abs() <= tolerance || (ratio - 1.0 / other).abs() <= tolerance
}

/// VirtualScreen.MonitorHasRecommendedAspectRatio: the mode's aspect equals the panel's native one.
pub fn has_recommended_aspect(monitor: &Monitor, resolution_px: Option<[f64; 2]>) -> bool {
    let mode = resolution_px.or_else(|| monitor.resolution_px().map(|[w, h]| [w as f64, h as f64]));
    let native = monitor.native_resolution().map(|[w, h]| [f64::from(w), f64::from(h)]);
    same_aspect_ignoring_orientation(mode, native, 0.001)
}

/// Index of the primary monitor: flagged primary, else placed at 0, 0, else the first.
pub fn primary(monitors: &[Monitor]) -> Option<usize> {
    monitors
        .iter()
        .position(|m| m.primary)
        .or_else(|| monitors.iter().position(|m| m.bounds.is_some_and(|b| b[0] == 0 && b[1] == 0)))
        .or_else(|| (!monitors.is_empty()).then_some(0))
}

/// InitSelectedMonitor: the only monitor, else the persisted one, else the primary one.
pub fn initial_monitor(monitors: &[Monitor], persisted_key: Option<&str>) -> Option<usize> {
    if monitors.len() == 1 {
        return Some(0);
    }
    let persisted =
        persisted_key.filter(|key| !key.is_empty()).and_then(|key| monitors.iter().position(|m| m.key == key));
    persisted.or_else(|| primary(monitors))
}

fn neighbour(monitors: &[Monitor], current: Option<&Monitor>, closer: impl Fn(i64, i64) -> bool) -> Option<usize> {
    let left = current?.left()?;
    if monitors.len() < 2 {
        return None;
    }
    let mut best: Option<(usize, i64)> = None;
    for (index, candidate) in monitors.iter().enumerate() {
        let Some(edge) = candidate.left().filter(|&edge| closer(edge, left)) else {
            continue;
        };
        if best.is_none_or(|(_, best_edge)| closer(best_edge, edge)) {
            best = Some((index, edge));
        }
    }
    best.map(|(index, _)| index)
}

/// GetPreviousDisplayDevice: the largest left edge below the current one's.
pub fn previous_monitor(monitors: &[Monitor], current: Option<&Monitor>) -> Option<usize> {
    neighbour(monitors, current, |a, b| a < b)
}

/// GetNextDisplayDevice: the smallest left edge above the current one's.
pub fn next_monitor(monitors: &[Monitor], current: Option<&Monitor>) -> Option<usize> {
    neighbour(monitors, current, |a, b| a > b)
}

fn rotation(name: &str) -> Option<u32> {
    match name {
        "normal" => Some(0),
        "left" => Some(90),
        "inverted" => Some(180),
        "right" => Some(270),
        _ => None,
    }
}

fn digits(text: &str, signed: bool) -> Option<(i64, &str)> {
    let sign = usize::from(signed && text.starts_with('-'));
    let end = sign + text[sign..].bytes().take_while(u8::is_ascii_digit).count();
    if end == sign {
        return None;
    }
    Some((text[..end].parse().ok()?, &text[end..]))
}

/// ` WxH+X+Y` at the start of `text`.
fn geometry(text: &str) -> Option<([i64; 4], &str)> {
    let (width, rest) = digits(text.strip_prefix(' ')?, false)?;
    let (height, rest) = digits(rest.strip_prefix('x')?, false)?;
    let (x, rest) = digits(rest.strip_prefix('+')?, true)?;
    let (y, rest) = digits(rest.strip_prefix('+')?, true)?;
    Some(([x, y, width, height], rest))
}

/// An output line `NAME connected[ primary][ WxH+X+Y][ ROTATION]...`.
fn output_line(line: &str) -> Option<Monitor> {
    let (name, rest) = line.split_once(' ')?;
    if name.is_empty() || name.chars().any(char::is_whitespace) {
        return None;
    }
    let rest = rest.strip_prefix("connected")?;
    let (primary, rest) = match rest.strip_prefix(" primary") {
        Some(rest) => (true, rest),
        None => (false, rest),
    };
    let (bounds, rest) = match geometry(rest) {
        Some((bounds, rest)) => (Some(bounds), rest),
        None => (None, rest),
    };
    let rotation = rest
        .strip_prefix(' ')
        .and_then(|rest| ["normal", "left", "inverted", "right"].into_iter().find(|name| rest.starts_with(name)))
        .and_then(rotation)
        .unwrap_or(0);
    Some(Monitor { key: name.into(), name: name.into(), bounds, primary, rotation, ..Monitor::default() })
}

fn from_hex(text: &str) -> Result<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return Err(GazeError::Value("non-hexadecimal number found in fromhex() arg".into()));
    }
    (0..text.len())
        .step_by(2)
        .map(|at| {
            u8::from_str_radix(&text[at..at + 2], 16)
                .map_err(|_| GazeError::Value("non-hexadecimal number found in fromhex() arg".into()))
        })
        .collect()
}

/// Connected outputs of `xrandr --prop` that have a position, with their EDID when listed.
pub fn parse_xrandr(text: &str) -> Result<Vec<Monitor>> {
    let mut monitors = Vec::new();
    let mut current: Option<Monitor> = None;
    let mut edid = String::new();
    let mut in_edid = false;
    let mut finish = |current: Option<Monitor>, edid: &str| -> Result<()> {
        if let Some(mut monitor) = current {
            monitor.edid = if edid.is_empty() { None } else { Some(from_hex(edid)?) };
            monitors.push(monitor);
        }
        Ok(())
    };
    for line in text.lines() {
        if line.chars().next().is_some_and(|c| !c.is_whitespace()) {
            finish(current.take(), &edid)?;
            edid.clear();
            in_edid = false;
            current = output_line(line);
            continue;
        }
        let stripped = line.trim();
        if stripped.starts_with("EDID:") {
            in_edid = true;
        } else if in_edid && !stripped.is_empty() && stripped.bytes().all(|b| b.is_ascii_hexdigit()) {
            edid.push_str(stripped);
        } else {
            in_edid = false;
        }
    }
    finish(current.take(), &edid)?;
    monitors.retain(|monitor| monitor.bounds.is_some());
    Ok(monitors)
}

/// Connected DRM connectors with their non-empty EDIDs: (connector, EDID).
fn drm_connectors(root: &Path) -> Result<Vec<(String, Vec<u8>)>> {
    let mut connectors = Vec::new();
    for edid_path in edid_paths(root) {
        let directory = edid_path.parent().expect("edid has a parent");
        let status = directory.join("status");
        if status.exists() && std::fs::read_to_string(&status)?.trim() != "connected" {
            continue;
        }
        let edid = std::fs::read(&edid_path)?;
        if edid.is_empty() {
            continue;
        }
        let connector = directory.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        connectors.push((connector, edid));
    }
    Ok(connectors)
}

/// `card0-DP-1` -> `DP-1`.
fn output_name(connector: &str) -> &str {
    connector.split_once('-').map_or(connector, |(_, name)| name)
}

/// Connected DRM connectors with their EDID but no geometry (Wayland without xrandr).
pub fn drm_fallback(root: &Path) -> Result<Vec<Monitor>> {
    let monitors = drm_connectors(root)?.into_iter().map(|(connector, edid)| {
        let name = output_name(&connector).to_string();
        Monitor { key: name.clone(), name, edid: Some(edid), connector: Some(connector), ..Monitor::default() }
    });
    Ok(monitors.collect())
}

/// Each monitor's DRM connector, found by its EDID; among connectors with the same EDID the
/// one named like the monitor. Monitors without an EDID or a unique match get none.
pub fn with_drm_connectors(mut monitors: Vec<Monitor>, root: &Path) -> Vec<Monitor> {
    let connectors = drm_connectors(root).unwrap_or_default();
    for monitor in &mut monitors {
        let Some(edid) = &monitor.edid else { continue };
        let same: Vec<&str> =
            connectors.iter().filter(|(_, other)| other == edid).map(|(connector, _)| connector.as_str()).collect();
        let found = match same.as_slice() {
            [only] => Some(*only),
            _ => same.iter().copied().find(|connector| output_name(connector) == monitor.name),
        };
        monitor.connector = found.map(str::to_string);
    }
    monitors
}

fn executable_on_path(name: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|directory| directory.join(name).is_file()))
}

/// Standard output of a command that succeeded within the query timeout.
fn command_output(program: &str, arguments: &[&str]) -> Option<String> {
    let mut child = Command::new(program)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        std::io::Read::read_to_string(&mut stdout, &mut text).ok().map(|_| text)
    });
    let deadline = Instant::now() + Duration::from_secs_f64(QUERY_TIMEOUT_S);
    let status = loop {
        if let Some(status) = child.try_wait().ok()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let text = reader.join().ok()??;
    status.success().then_some(text)
}

/// The monitors of the Linux desktop: xrandr's outputs with their DRM connectors, else the
/// DRM connectors.
pub fn linux_monitors() -> Result<Vec<Monitor>> {
    let drm = Path::new("/sys/class/drm");
    if executable_on_path("xrandr")
        && let Some(text) = command_output("xrandr", &["--prop"])
    {
        let monitors = parse_xrandr(&text)?;
        if !monitors.is_empty() {
            return Ok(with_drm_connectors(monitors, drm));
        }
    }
    drm_fallback(drm)
}

/// One output as the desktop's toolkit places it: its connector name, logical x, y, width
/// and height, and its scale. Under Wayland this is the only source of the layout: xrandr
/// sees XWayland's outputs, under other names, and /sys/class/drm has no positions.
#[derive(Debug, Clone, PartialEq)]
pub struct Placed {
    pub connector: String,
    pub geometry: [i64; 4],
    pub scale: f64,
}

/// The monitors given the toolkit's placement: `known` (xrandr's or the DRM connectors)
/// when it places every output under the same name, else one monitor per placed output
/// with the EDID of the monitor or DRM connector of that name. Positions stay logical, so
/// only their order counts; sizes are in device pixels. A panel placed across its native
/// orientation is taken as turned 90 degrees.
pub fn placed_monitors(placed: &[Placed], known: Vec<Monitor>, root: &Path) -> Vec<Monitor> {
    let named = |name: &str| known.iter().any(|m| m.name == name && m.bounds.is_some());
    if placed.is_empty() || placed.iter().all(|output| named(&output.connector)) {
        return known;
    }
    let connectors = drm_connectors(root).unwrap_or_default();
    placed
        .iter()
        .map(|output| {
            let [x, y, width, height] = output.geometry;
            let device = |length: i64| (length as f64 * output.scale).round() as i64;
            let drm = connectors.iter().find(|(connector, _)| output_name(connector) == output.connector);
            let edid = known
                .iter()
                .find(|m| m.name == output.connector)
                .and_then(|m| m.edid.clone())
                .or_else(|| drm.map(|(_, edid)| edid.clone()));
            let landscape_panel = edid.as_deref().and_then(|edid| edid_native_resolution(edid).ok()).map(|[w, h]| w > h);
            let rotation = if landscape_panel == Some(height > width) { 90 } else { 0 };
            Monitor {
                key: output.connector.clone(),
                name: output.connector.clone(),
                bounds: Some([x, y, device(width), device(height)]),
                rotation,
                edid,
                connector: drm.map(|(connector, _)| connector.clone()),
                ..Monitor::default()
            }
        })
        .collect()
}

/// The Linux desktop's monitors as the toolkit places `placed`; see [`placed_monitors`].
pub fn placed_linux_monitors(placed: &[Placed]) -> Vec<Monitor> {
    placed_monitors(placed, linux_monitors().unwrap_or_default(), Path::new("/sys/class/drm"))
}

/// `\\?\DISPLAY#GSM5B7F#5&5028a6b&0&UID41221#{guid}` -> `DISPLAY\GSM5B7F\5&5028A6B&0&UID41221`.
pub fn instance_id_from_interface(interface: Option<&str>) -> Option<String> {
    let interface = interface.filter(|interface| !interface.is_empty())?;
    let path = interface.strip_prefix(r"\\?\").unwrap_or(interface);
    let path = match path.rfind("#{") {
        Some(at) if path.ends_with('}') && !path[at + 2..path.len() - 1].contains('}') => &path[..at],
        _ => path,
    };
    Some(path.replace('#', "\\").to_uppercase())
}

const BASE64_ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// `base64.b64decode` without validation: characters outside the alphabet are skipped.
pub fn base64_decode(text: &str) -> Result<Vec<u8>> {
    let mut sextets = Vec::new();
    let mut padding = 0;
    for byte in text.bytes() {
        if byte == b'=' {
            padding += 1;
            if sextets.len() % 4 >= 2 && (sextets.len() + padding) % 4 == 0 {
                break;
            }
            continue;
        }
        if let Some(position) = BASE64_ALPHABET.iter().position(|&c| c == byte) {
            sextets.push(position as u32);
        }
    }
    if sextets.len() % 4 == 1 || (sextets.len() % 4 != 0 && (sextets.len() + padding) % 4 != 0) {
        return Err(GazeError::Value("Incorrect padding".into()));
    }
    let mut bytes = Vec::with_capacity(sextets.len() * 3 / 4);
    for chunk in sextets.chunks(4) {
        let word = chunk.iter().enumerate().fold(0u32, |word, (i, &s)| word | s << (18 - 6 * i));
        bytes.extend_from_slice(&word.to_be_bytes()[1..chunk.len()]);
    }
    Ok(bytes)
}

fn truthy(value: Option<&serde_json::Value>) -> bool {
    match value {
        None | Some(serde_json::Value::Null) => false,
        Some(serde_json::Value::Bool(flag)) => *flag,
        Some(serde_json::Value::Number(number)) => number.as_f64().is_some_and(|n| n != 0.0),
        Some(serde_json::Value::String(text)) => !text.is_empty(),
        Some(serde_json::Value::Array(items)) => !items.is_empty(),
        Some(serde_json::Value::Object(fields)) => !fields.is_empty(),
    }
}

fn integer(value: &serde_json::Value) -> Option<i64> {
    value.as_i64().or_else(|| value.as_f64().filter(|n| n.is_finite()).map(|n| n.trunc() as i64))
}

fn windows_rotation(orientation: Option<&serde_json::Value>) -> u32 {
    let orientation = orientation.map_or(Some(0.0), serde_json::Value::as_f64);
    match orientation {
        Some(1.0) => 90,
        Some(2.0) => 180,
        Some(3.0) => 270,
        _ => 0,
    }
}

/// Monitors from the JSON the Windows desktop query prints (one object or a list).
pub fn parse_windows_monitors(text: &str) -> Result<Vec<Monitor>> {
    let invalid = |message: &str| GazeError::Value(message.to_string());
    let parsed: serde_json::Value = serde_json::from_str(text).map_err(|e| invalid(&e.to_string()))?;
    let items = match parsed {
        serde_json::Value::Array(items) => items,
        object @ serde_json::Value::Object(_) => vec![object],
        _ => return Err(invalid("Expected a JSON object or list of objects")),
    };
    items
        .iter()
        .map(|item| {
            let name = item.get("name").and_then(serde_json::Value::as_str).ok_or_else(|| invalid("name"))?;
            let instance_id = instance_id_from_interface(item.get("interface").and_then(serde_json::Value::as_str));
            let edid = match item.get("edid").and_then(serde_json::Value::as_str).filter(|_| truthy(item.get("edid"))) {
                Some(encoded) => Some(base64_decode(encoded)?),
                None => None,
            };
            let bounds: Vec<i64> = item
                .get("bounds")
                .and_then(serde_json::Value::as_array)
                .and_then(|values| values.iter().map(integer).collect())
                .ok_or_else(|| invalid("bounds"))?;
            let bounds: [i64; 4] = bounds.try_into().map_err(|_| invalid("bounds"))?;
            Ok(Monitor {
                key: instance_id.clone().unwrap_or_else(|| name.to_string()),
                name: name.to_string(),
                bounds: Some(bounds),
                primary: truthy(item.get("primary")),
                rotation: windows_rotation(item.get("orientation")),
                edid,
                instance_id,
                connector: None,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const EDID_UID41221: &str = "00ffffffffffff001e6d7f5b0b220100061e0104b53c22789f8cb5af4f43ab260e5054254b007140818081c0a9c0b300d1c08100\
        d1cf28de0050a0a038500830080455502100001a000000fd003090e6e63c010a202020202020000000fc003237474c3835300a\
        2020202020000000ff003030364e54435a32363235310a01fa";

    fn edid() -> Vec<u8> {
        from_hex(EDID_UID41221).unwrap()
    }

    fn monitor(key: &str, bounds: [i64; 4]) -> Monitor {
        Monitor { key: key.into(), name: key.into(), bounds: Some(bounds), ..Monitor::default() }
    }

    #[test]
    fn xrandr_connected_outputs_with_geometry_rotation_and_edid() {
        let hex = EDID_UID41221;
        let text = format!(
            "Screen 0: minimum 320 x 200, current 4000 x 2560, maximum 16384 x 16384\n\
             DP-1 connected primary 2560x1440+1440+588 (normal left inverted right x axis y axis) 597mm x 336mm\n\
             \tEDID:\n\t\t{}\n\t\t{}\n\tnon-desktop: 0\n\t\tsupported: 0, 1\n   2560x1440    143.97*+  59.95\n\
             HDMI-1 connected 1440x2560+0+0 left (normal left inverted right x axis y axis) 597mm x 336mm\n\
             DP-2 disconnected (normal left inverted right x axis y axis)\n\
             HDMI-2 connected (normal left inverted right x axis y axis)\n",
            &hex[..128],
            &hex[128..]
        );
        let monitors = parse_xrandr(&text).unwrap();
        assert_eq!(monitors.len(), 2);
        assert_eq!((monitors[0].primary, monitors[0].bounds), (true, Some([1440, 588, 2560, 1440])));
        assert_eq!(monitors[0].edid.as_deref(), Some(edid().as_slice()));
        assert_eq!((monitors[1].rotation, monitors[1].edid.clone()), (90, None));
        assert_eq!(monitors[0].size_mm(), Some([597.0, 336.0]));
    }

    #[test]
    fn windows_query_output_gives_instance_ids_and_display_id() {
        let encoded: String = {
            let bytes = edid();
            let mut text = String::new();
            for chunk in bytes.chunks(3) {
                let word = chunk.iter().enumerate().fold(0u32, |w, (i, &b)| w | u32::from(b) << (16 - 8 * i));
                for i in 0..4 {
                    text.push(if i <= chunk.len() {
                        BASE64_ALPHABET[(word >> (18 - 6 * i) & 63) as usize] as char
                    } else {
                        '='
                    });
                }
            }
            text
        };
        let json = format!(
            r#"[{{"name": "\\\\.\\DISPLAY2", "primary": true, "bounds": [0, 0, 2560, 1440], "orientation": 0,
                "interface": "\\\\?\\DISPLAY#GSM5B7F#5&5028a6b&0&UID41221#{{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}}",
                "edid": "{encoded}"}}]"#
        );
        let monitors = parse_windows_monitors(&json).unwrap();
        assert_eq!(monitors[0].key, r"DISPLAY\GSM5B7F\5&5028A6B&0&UID41221");
        assert_eq!(
            monitors[0].display_id().unwrap(),
            r"DISPLAY\GSM5B7F\5&5028A6B&0&UID41221##eId:1E6D7F5B0B220100061E#or:1#v:1.0"
        );
        assert_eq!(base64_decode(&encoded).unwrap(), edid());
        assert!(base64_decode("abc").is_err());
    }

    #[test]
    fn previous_next_and_initial_follow_tobii() {
        let all = [
            monitor("primary", [0, 0, 2560, 1440]),
            monitor("portrait", [-1440, -588, 1440, 2560]),
            monitor("third", [2560, 0, 1920, 1080]),
        ];
        assert_eq!(previous_monitor(&all, Some(&all[0])), Some(1));
        assert_eq!(next_monitor(&all, Some(&all[0])), Some(2));
        assert_eq!(previous_monitor(&all, Some(&all[1])), None);
        assert_eq!(next_monitor(&all[..1], Some(&all[0])), None);
        assert_eq!(initial_monitor(&all[1..2], None), Some(0));
        assert_eq!(initial_monitor(&all, Some("portrait")), Some(1));
        assert_eq!(initial_monitor(&all, Some("unplugged")), Some(0));
    }

    fn drm_root(name: &str, connectors: &[(&str, &[u8], &str)]) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("tobii-gaze-drm-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (connector, edid, status) in connectors {
            let directory = root.join(connector);
            std::fs::create_dir_all(&directory).unwrap();
            std::fs::write(directory.join("edid"), edid).unwrap();
            std::fs::write(directory.join("status"), format!("{status}\n")).unwrap();
        }
        root
    }

    #[test]
    fn monitors_get_their_drm_connector() {
        let other = {
            let mut other = edid();
            other[12] ^= 0xff;
            other
        };
        let root = drm_root(
            "connectors",
            &[("card0-DP-1", &edid(), "connected"), ("card0-HDMI-A-1", &other, "connected"), ("card0-DP-2", &[], "")],
        );
        let fallback = drm_fallback(&root).unwrap();
        let names: Vec<_> = fallback.iter().map(|m| (m.key.as_str(), m.connector.as_deref())).collect();
        assert_eq!(names, [("DP-1", Some("card0-DP-1")), ("HDMI-A-1", Some("card0-HDMI-A-1"))]);

        let mut xrandr = vec![monitor("XWAYLAND0", [0, 0, 2560, 1440]), monitor("XWAYLAND1", [2560, 0, 1920, 1080])];
        xrandr[0].edid = Some(other.clone());
        let found = with_drm_connectors(xrandr, &root);
        assert_eq!(found[0].connector.as_deref(), Some("card0-HDMI-A-1"), "found by EDID, not by name");
        assert_eq!(found[1].connector, None, "no EDID, no connector");

        let twins = drm_root("twins", &[("card0-DP-1", &edid(), "connected"), ("card1-DP-3", &edid(), "connected")]);
        let mut same = vec![monitor("DP-3", [0, 0, 2560, 1440]), monitor("HDMI-1", [2560, 0, 2560, 1440])];
        same[0].edid = Some(edid());
        same[1].edid = Some(edid());
        let found = with_drm_connectors(same, &twins);
        assert_eq!(found[0].connector.as_deref(), Some("card1-DP-3"), "equal EDIDs: the one of the same name");
        assert_eq!(found[1].connector, None, "equal EDIDs and no name match: unknown");
        for root in [root, twins] {
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn placed_outputs_win_over_xwayland_names() {
        let root = drm_root("placed", &[("card1-DP-1", &edid(), "connected"), ("card1-HDMI-A-1", &edid(), "connected")]);
        let placed = [
            Placed { connector: "DP-1".into(), geometry: [1440, 0, 1280, 720], scale: 2.0 },
            Placed { connector: "HDMI-A-1".into(), geometry: [0, -400, 1440, 2560], scale: 1.0 },
        ];
        let xwayland = vec![monitor("XWAYLAND0", [0, 0, 2560, 1440]), monitor("XWAYLAND1", [2560, 0, 1440, 2560])];
        let found = placed_monitors(&placed, xwayland, &root);
        let summary: Vec<_> = found.iter().map(|m| (m.key.as_str(), m.bounds, m.rotation, m.connector.as_deref())).collect();
        assert_eq!(
            summary,
            [
                ("DP-1", Some([1440, 0, 2560, 1440]), 0, Some("card1-DP-1")),
                ("HDMI-A-1", Some([0, -400, 1440, 2560]), 90, Some("card1-HDMI-A-1")),
            ],
            "logical positions, device-pixel sizes, a landscape panel placed in portrait turned"
        );
        assert_eq!(found[0].edid, Some(edid()));
        assert_eq!(previous_monitor(&found, Some(&found[0])), Some(1), "the switcher can move between them");

        let x11 = vec![monitor("DP-1", [0, 0, 2560, 1440]), monitor("HDMI-A-1", [2560, 0, 1440, 2560])];
        assert_eq!(placed_monitors(&placed, x11.clone(), &root), x11, "xrandr naming every output is kept");
        assert_eq!(placed_monitors(&[], x11.clone(), &root), x11, "nothing placed keeps the known monitors");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn aspect_check_ignores_orientation() {
        let mut with_edid = monitor("x", [0, 0, 2560, 1440]);
        with_edid.edid = Some(edid());
        assert!(has_recommended_aspect(&with_edid, None));
        assert!(!has_recommended_aspect(&with_edid, Some([1280.0, 1024.0])));
        assert!(has_recommended_aspect(&monitor("x", [0, 0, 1280, 1024]), None));
    }
}
