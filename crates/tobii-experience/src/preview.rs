//! When the "Preview my gaze" bubble shows and on which output, without GTK.
//!
//! Tobii's `GazeTraceVisualizationExpApp` shows the bubble while the preview is enabled and
//! the app window is active and not minimized (docs/experience-app.md 4.8); here it also
//! needs state Tracking and the tracked monitor. Becoming shown subscribes to gaze and
//! becoming hidden unsubscribes. The output is the monitor of the last display setup
//! (`display-setup.json`), found by connector, else by the EDID manufacturer and model.

use std::path::Path;

use serde_json::Value;
use tobii_control::Request;

/// What the bubble's visibility depends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PreviewInputs {
    /// The "Preview my gaze" toggle.
    pub requested: bool,
    pub window_active: bool,
    pub window_minimized: bool,
    pub tracking: bool,
    /// The tracked monitor is connected.
    pub output_found: bool,
}

impl PreviewInputs {
    pub fn shown(&self) -> bool {
        self.requested && self.window_active && !self.window_minimized && self.tracking && self.output_found
    }
}

/// The bubble's visibility and the gaze subscription that goes with it.
#[derive(Debug, Clone, Default)]
pub struct Preview {
    inputs: PreviewInputs,
    shown: bool,
}

impl Preview {
    pub fn inputs(&self) -> PreviewInputs {
        self.inputs
    }

    pub fn shown(&self) -> bool {
        self.shown
    }

    /// Takes new inputs; returns the `SubscribeGaze` request to send when the bubble shows or
    /// hides.
    pub fn update(&mut self, inputs: PreviewInputs) -> Option<Request> {
        self.inputs = inputs;
        let shown = inputs.shown();
        (self.shown != shown).then(|| {
            self.shown = shown;
            Request::SubscribeGaze(shown)
        })
    }
}

/// The identity of the monitor the last display setup recorded.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RecordedOutput {
    /// DRM connector name, `card0-DP-1` or `DP-1`.
    pub connector: Option<String>,
    /// EDID manufacturer PNP id, for example `DEL`.
    pub manufacturer: Option<String>,
    pub product_code: Option<u16>,
    /// The EDID monitor name descriptor.
    pub model: Option<String>,
}

/// One output as the toolkit reports it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OutputInfo {
    pub connector: Option<String>,
    pub manufacturer: Option<String>,
    pub model: Option<String>,
}

impl RecordedOutput {
    /// Reads `monitor.connector`, `monitor.edid` (hex) and `monitor.edid_id` from a display
    /// record; `None` when the file is missing or unreadable.
    pub fn read(path: &Path) -> Option<RecordedOutput> {
        let record: Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
        Some(RecordedOutput::from_record(&record))
    }

    pub fn from_record(record: &Value) -> RecordedOutput {
        let text = |field: &str| {
            let value = record.get("monitor")?.get(field)?.as_str()?;
            Some(value.trim().to_string()).filter(|value| !value.is_empty())
        };
        let edid = text("edid").and_then(|hex| decode_hex(&hex)).filter(|edid| edid.len() >= 128);
        let id = edid
            .clone()
            .or_else(|| text("edid_id").and_then(|hex| decode_hex(&hex)).map(|id| [vec![0; 8], id].concat()));
        let id = id.filter(|bytes| bytes.len() >= 12);
        RecordedOutput {
            connector: text("connector"),
            manufacturer: id.as_deref().and_then(|bytes| pnp_id(u16::from_be_bytes([bytes[8], bytes[9]]))),
            product_code: id.as_deref().map(|bytes| u16::from_le_bytes([bytes[10], bytes[11]])),
            model: edid.as_deref().and_then(monitor_name),
        }
    }
}

/// The index of the output showing the recorded monitor: same connector, else same EDID
/// manufacturer and model. `None` hides the bubble.
pub fn choose_output(recorded: &RecordedOutput, outputs: &[OutputInfo]) -> Option<usize> {
    let by_connector = recorded.connector.as_deref().map(strip_card).and_then(|connector| {
        outputs.iter().position(|output| output.connector.as_deref().map(strip_card) == Some(connector))
    });
    by_connector.or_else(|| outputs.iter().position(|output| same_edid(recorded, output)))
}

/// `card0-DP-1` as GDK names it, `DP-1`.
fn strip_card(connector: &str) -> &str {
    connector
        .strip_prefix("card")
        .and_then(|rest| rest.split_once('-').filter(|(index, _)| index.chars().all(|c| c.is_ascii_digit())))
        .map_or(connector, |(_, name)| name)
}

/// The model must match the EDID name, or its product code when the EDID has no name. A
/// manufacturer given as a PNP id must equal the recorded one; a vendor name (a compositor
/// that looks the id up) cannot be compared and is accepted.
fn same_edid(recorded: &RecordedOutput, output: &OutputInfo) -> bool {
    let Some(model) = output.model.as_deref().map(str::trim) else {
        return false;
    };
    let model_matches = match (&recorded.model, recorded.product_code) {
        (Some(name), _) => model.eq_ignore_ascii_case(name),
        (None, Some(code)) => model.eq_ignore_ascii_case(&format!("0x{code:04x}")),
        (None, None) => false,
    };
    let is_pnp_id = |text: &str| text.len() == 3 && text.chars().all(|c| c.is_ascii_alphabetic());
    let manufacturer_matches = match (output.manufacturer.as_deref().map(str::trim), &recorded.manufacturer) {
        (Some(reported), Some(pnp)) if is_pnp_id(reported) => reported.eq_ignore_ascii_case(pnp),
        (_, recorded) => recorded.is_some(),
    };
    model_matches && manufacturer_matches
}

fn decode_hex(hex: &str) -> Option<Vec<u8>> {
    if !hex.len().is_multiple_of(2) {
        return None;
    }
    (0..hex.len()).step_by(2).map(|at| u8::from_str_radix(hex.get(at..at + 2)?, 16).ok()).collect()
}

/// EDID bytes 8 and 9: three letters of five bits each.
fn pnp_id(code: u16) -> Option<String> {
    let letter = |shift: u16| {
        let index = (code >> shift) & 0x1f;
        (1..=26).contains(&index).then(|| char::from(b'A' + index as u8 - 1))
    };
    [10, 5, 0].into_iter().map(letter).collect()
}

/// The monitor name descriptor (tag 0xFC) of a base EDID block.
fn monitor_name(edid: &[u8]) -> Option<String> {
    let (descriptors, _) = edid[54..126].as_chunks::<18>();
    descriptors
        .iter()
        .find(|descriptor| descriptor[..3] == [0, 0, 0] && descriptor[3] == 0xfc)
        .map(|descriptor| String::from_utf8_lossy(&descriptor[5..]).split('\n').next().unwrap_or("").trim().to_string())
        .filter(|name| !name.is_empty())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn active() -> PreviewInputs {
        PreviewInputs {
            requested: true,
            window_active: true,
            window_minimized: false,
            tracking: true,
            output_found: true,
        }
    }

    #[test]
    fn bubble_only_while_window_active() {
        let mut preview = Preview::default();
        assert!(!preview.shown(), "the toggle starts off");
        assert_eq!(preview.update(PreviewInputs { requested: false, ..active() }), None);
        assert_eq!(preview.update(active()), Some(Request::SubscribeGaze(true)));
        assert!(preview.shown());
        assert_eq!(preview.update(active()), None, "no change, no request");

        assert_eq!(
            preview.update(PreviewInputs { window_active: false, ..active() }),
            Some(Request::SubscribeGaze(false))
        );
        assert!(!preview.shown(), "deactivated: hidden and unsubscribed");
        assert_eq!(preview.update(PreviewInputs { window_active: false, ..active() }), None);
        assert_eq!(preview.update(active()), Some(Request::SubscribeGaze(true)), "reactivated: shown");

        assert_eq!(
            preview.update(PreviewInputs { window_minimized: true, ..active() }),
            Some(Request::SubscribeGaze(false))
        );
        assert_eq!(preview.update(active()), Some(Request::SubscribeGaze(true)));
        assert_eq!(preview.update(PreviewInputs { tracking: false, ..active() }), Some(Request::SubscribeGaze(false)));
        assert_eq!(preview.update(active()), Some(Request::SubscribeGaze(true)));
        assert_eq!(preview.update(PreviewInputs { requested: false, ..active() }), Some(Request::SubscribeGaze(false)));
    }

    #[test]
    fn no_matching_output_hides_overlay() {
        let recorded = RecordedOutput {
            connector: Some("card0-DP-1".into()),
            manufacturer: Some("DEL".into()),
            product_code: Some(0xa0b1),
            model: Some("DELL U2720Q".into()),
        };
        let others = [
            OutputInfo {
                connector: Some("HDMI-A-1".into()),
                manufacturer: Some("DEL".into()),
                model: Some("DELL P2419H".into()),
            },
            OutputInfo {
                connector: Some("eDP-1".into()),
                manufacturer: Some("BOE".into()),
                model: Some("0x0bca".into()),
            },
        ];
        assert_eq!(choose_output(&recorded, &others), None);
        assert_eq!(choose_output(&recorded, &[]), None);

        let mut preview = Preview::default();
        assert_eq!(preview.update(active()), Some(Request::SubscribeGaze(true)));
        assert_eq!(
            preview.update(PreviewInputs { output_found: false, ..active() }),
            Some(Request::SubscribeGaze(false)),
            "the monitor went away: hidden, the toggle stays on"
        );
        assert!(preview.inputs().requested);
        assert_eq!(preview.update(active()), Some(Request::SubscribeGaze(true)), "it came back");
    }

    #[test]
    fn output_by_connector_then_edid() {
        let recorded = RecordedOutput {
            connector: Some("card1-DP-2".into()),
            manufacturer: Some("DEL".into()),
            product_code: Some(0xa0b1),
            model: Some("DELL U2720Q".into()),
        };
        let outputs = [
            OutputInfo {
                connector: Some("DP-1".into()),
                manufacturer: Some("DEL".into()),
                model: Some("DELL U2720Q".into()),
            },
            OutputInfo {
                connector: Some("DP-2".into()),
                manufacturer: Some("GSM".into()),
                model: Some("LG ULTRAGEAR".into()),
            },
        ];
        assert_eq!(choose_output(&recorded, &outputs), Some(1), "the connector wins over the EDID");

        let moved = RecordedOutput { connector: Some("card1-DP-3".into()), ..recorded.clone() };
        assert_eq!(choose_output(&moved, &outputs), Some(0), "else the EDID manufacturer and model");

        let vendor_named =
            [OutputInfo { connector: None, manufacturer: Some("Dell Inc.".into()), model: Some("dell u2720q".into()) }];
        assert_eq!(choose_output(&moved, &vendor_named), Some(0), "a vendor name cannot be checked against the PNP id");

        let wrong_vendor =
            [OutputInfo { connector: None, manufacturer: Some("GSM".into()), model: Some("DELL U2720Q".into()) }];
        assert_eq!(choose_output(&moved, &wrong_vendor), None);

        let by_code = RecordedOutput { model: None, ..moved.clone() };
        let coded = [OutputInfo { connector: None, manufacturer: Some("DEL".into()), model: Some("0xA0B1".into()) }];
        assert_eq!(choose_output(&by_code, &coded), Some(0), "the product code when the EDID has no name");

        assert_eq!(choose_output(&RecordedOutput::default(), &outputs), None, "an empty record matches nothing");
    }

    #[test]
    fn record_fields() {
        let mut edid = [0u8; 128];
        edid[..8].copy_from_slice(&[0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x00]);
        edid[8..12].copy_from_slice(&[0x10, 0xac, 0xb1, 0xa0]);
        edid[72..77].copy_from_slice(&[0, 0, 0, 0xfc, 0]);
        edid[77..90].copy_from_slice(b"DELL U2720Q\n ");
        let hex: String = edid.iter().map(|b| format!("{b:02x}")).collect();
        let full = RecordedOutput::from_record(
            &json!({"monitor": {"connector": "card0-DP-1", "edid": hex, "edid_id": "10ACB1A0000000000000"}}),
        );
        assert_eq!(
            full,
            RecordedOutput {
                connector: Some("card0-DP-1".into()),
                manufacturer: Some("DEL".into()),
                product_code: Some(0xa0b1),
                model: Some("DELL U2720Q".into()),
            }
        );

        let today =
            RecordedOutput::from_record(&json!({"monitor": {"key": "DP-1", "edid_id": "10ACB1A0000000000000"}}));
        assert_eq!(
            today,
            RecordedOutput {
                connector: None,
                manufacturer: Some("DEL".into()),
                product_code: Some(0xa0b1),
                model: None
            },
            "edid_id holds the manufacturer and product code"
        );
        assert_eq!(RecordedOutput::from_record(&json!({"monitor": {"edid_id": "zz"}})), RecordedOutput::default());
    }
}
