//! A scripted Eye Tracker 5 and a scripted user, for trying the setup without hardware.
//!
//! The device side answers the commands the setup sends with the semantics recovered from
//! the runtime and the Tobii client (docs/gaze-calibration.md, docs/display-setup.md); it
//! is NOT recorded device traffic. While gaze is subscribed it streams about 30 messages a
//! second carrying the columns the setup reads: per-eye validity and 2D gaze point, the
//! relative eye positions (3, 9) and the user position guide (37 to 40).
//!
//! The user sits too far back for the first seconds of eye positioning, then leans in to
//! the middle of the track box, and looks at whichever calibration target is the first one
//! of the current stage not collected yet, so the gaze-driven target choice finds it. Unlike
//! the Python simulation, the user looks at the targets as scaled into the calibration area
//! of the display area the device holds, so a large display does not stall the calibration.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use tobii_device::{Channel, ChannelError};
use tobii_gaze::calibration::{INTERACTIVE_POINTS, STAGES};
use tobii_gaze::device::{
    self as gd, EyeQuality, PointStatus, STATUS_CALIBRATION_ID, STATUS_STRUCT, StimulusPoint, gaze_command as gc,
};
use tobii_gaze::display::{DisplayArea, ET5_MOUNTING};
use tobii_gaze::math::float32;
use tobii_protocol::commands::{RESPONSE_KIND, Stream, blob_field, command, string_field};
use tobii_protocol::fields::{
    Value, array_fields, fixed42_field, struct_field, triple_fixed42_fields, uint32_field, value, values,
};
use tobii_protocol::gaze::build;
use tobii_protocol::ttp::{Decoder, encode_payload, fields};
use tobii_protocol::{Field, Message, ProtocolError};

pub const ALBEDO_AREA: DisplayArea = DisplayArea {
    top_left: [-296.6200866699219, 326.0040588378906, 111.8187484741211],
    top_right: [300.3799133300781, 326.0040588378906, 111.8187484741211],
    bottom_left: [-296.6200866699219, 10.267330169677734, -3.100020170211792],
};
pub const UNMAPPED_AREA: DisplayArea = DisplayArea { top_left: [0.0; 3], top_right: [0.0; 3], bottom_left: [0.0; 3] };
pub const DISPLAY_AREA_TRAILER: u32 = 0x3039;
pub const STATE_ERROR: u32 = 0x2000_0508;
pub const OPERATION_FAILED: u32 = 0x2000_0407;
pub const GAZE_PERIOD: Duration = Duration::from_micros(33_333);
pub const COLLECT_S: f64 = 0.7;
pub const COMPUTE_S: f64 = 0.4;
pub const SEATED_AFTER_S: f64 = 9.0;
pub const LEAN_IN_S: f64 = 1.0;
pub const FAR_Z: f64 = 0.85;
pub const IDEAL_Z: f64 = 0.5;
pub const EYE_SPACING: f64 = 0.12;
pub const EYE_INVALID: u32 = 4;
pub const DEFAULT_CALIBRATION_ID: u32 = 0x3B9A_CA07;
const BLOB_MARKER: u32 = 0x5A5A_5A5A;

/// Where the user sits (normalized track box, z 0.5 at the ideal distance).
///
/// From the first time gaze is subscribed the user sits too far back until the last second
/// of `seated_after_s`, then leans in to the ideal distance.
#[derive(Debug, Clone, PartialEq)]
pub struct SimulatedUser {
    pub seated_after_s: f64,
    pub start: Option<f64>,
    pub present: bool,
}

impl Default for SimulatedUser {
    fn default() -> Self {
        Self { seated_after_s: SEATED_AFTER_S, start: None, present: true }
    }
}

impl SimulatedUser {
    pub fn seated_after(seconds: f64) -> Self {
        Self { seated_after_s: seconds, ..Self::default() }
    }

    fn sit_down(&mut self, now: f64) {
        self.start.get_or_insert(now);
    }

    pub fn head(&self, now: f64) -> [f64; 3] {
        let elapsed = now - self.start.unwrap_or(now);
        let settle = ((elapsed - (self.seated_after_s - LEAN_IN_S)).max(0.0) / LEAN_IN_S).min(1.0);
        let sway = 0.01 * (now * 1.3).sin();
        [0.5 + sway, 0.5 + 0.6 * sway, FAR_Z - (FAR_Z - IDEAL_Z) * settle]
    }
}

/// A 1100 blob with one record per collected point, the layout `calibration_points` reads.
pub fn calibration_blob(points: &[[f64; 2]], calibration_id: u32) -> Vec<u8> {
    let mut header = Vec::new();
    for word in [110u32, 84, 1, 0, calibration_id] {
        header.extend(word.to_le_bytes());
    }
    let mut name = b"human".to_vec();
    name.resize(64, 0);
    header.extend(name);
    let trailer_offset = 4 + header.len() as u32;
    let mut blob = trailer_offset.to_le_bytes().to_vec();
    blob.extend(header);
    blob.extend(BLOB_MARKER.to_le_bytes());
    blob.extend((points.len() as i32).to_le_bytes());
    for &[x, y] in points {
        let record = gd::calibration_record([x, y], ([x + 0.004, y], 1), ([x, y - 0.004], 1));
        blob.extend(record.expect("simulated points fit float32"));
    }
    blob
}

/// 1120 records for the collected points; `weak_points` get a poor bias and precision.
pub fn stimulus_quality(points: &[[f64; 2]], weak_points: &[[f64; 2]]) -> Vec<StimulusPoint> {
    points
        .iter()
        .map(|&[x, y]| {
            let weak = weak_points.contains(&[x, y]);
            let eye = EyeQuality {
                status: PointStatus::ValidAndUsed as i64,
                bias: if weak { 0.06 } else { 0.008 },
                precision: if weak { 0.05 } else { 0.01 },
            };
            StimulusPoint { point: [x, y, 0.0], left: eye, right: eye }
        })
        .collect()
}

/// Deterministic Gaussian noise for the gaze points (xorshift and Box-Muller).
#[derive(Debug, Clone)]
struct Noise(u64);

impl Noise {
    fn uniform(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        ((self.0 >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }

    fn gauss(&mut self, sigma: f64) -> f64 {
        let (u, v) = (self.uniform(), self.uniform());
        sigma * (-2.0 * u.ln()).sqrt() * (std::f64::consts::TAU * v).cos()
    }
}

/// Where the simulated user looks, when set instead of the built-in choice of target.
pub type LookingAt = Arc<dyn Fn() -> Option<[f64; 2]> + Send + Sync>;

/// The device state, shared so tests can inspect it while a session owns the channel.
#[derive(Debug)]
pub struct TrackerState {
    pub area: DisplayArea,
    pub calibration_id: u32,
    pub blob: Vec<u8>,
    pub fail_compute_at: Vec<usize>,
    pub weak_points: Vec<[f64; 2]>,
    pub collect_s: f64,
    pub compute_s: f64,
    pub user: SimulatedUser,
    pub log: Vec<Message>,
    pub calibrating: bool,
    pub collected: Vec<[f64; 2]>,
    pub computes: usize,
    pub subscribed: bool,
    collecting: Option<([f64; 2], f64)>,
}

impl TrackerState {
    pub fn commands(&self) -> Vec<u32> {
        self.log.iter().map(|message| message.command).collect()
    }
}

/// A channel that behaves like the tracker for the setup.
///
/// `mapped: false` starts with no display area stored, as a new installation;
/// `fail_compute_at` makes the nth compute fail as "all points failed"; `weak_points` get a
/// poor quality in 1120.
pub struct SimulatedTracker {
    pub state: Arc<Mutex<TrackerState>>,
    pub looking_at: Option<LookingAt>,
    requests: Decoder,
    outgoing: VecDeque<(f64, Vec<u8>)>,
    next_gaze: f64,
    noise: Noise,
    epoch: Instant,
}

impl std::fmt::Debug for SimulatedTracker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SimulatedTracker").field("state", &self.state).finish_non_exhaustive()
    }
}

/// Options of a [`SimulatedTracker`].
#[derive(Debug, Clone)]
pub struct Simulation {
    pub mapped: bool,
    pub calibration_id: u32,
    pub fail_compute_at: Vec<usize>,
    pub weak_points: Vec<[f64; 2]>,
    pub user: SimulatedUser,
    pub collect_s: f64,
    pub compute_s: f64,
}

impl Default for Simulation {
    fn default() -> Self {
        Self {
            mapped: true,
            calibration_id: DEFAULT_CALIBRATION_ID,
            fail_compute_at: Vec::new(),
            weak_points: Vec::new(),
            user: SimulatedUser::default(),
            collect_s: COLLECT_S,
            compute_s: COMPUTE_S,
        }
    }
}

/// The calibration targets the setup shows for the display area the device holds.
fn targets(area: &DisplayArea) -> Vec<[f64; 2]> {
    let size = tobii_gaze::display::is_mapped(Some(area)).then(|| [area.width_mm(), area.height_mm()]);
    tobii_gaze::calibration::calibration_targets(size, &INTERACTIVE_POINTS)
}

/// The target being collected, else the first of the current stage not collected yet, else the centre.
fn default_looking_at(state: &TrackerState, now: f64) -> [f64; 2] {
    if let Some((point, until)) = state.collecting
        && now < until
    {
        return point;
    }
    let targets = targets(&state.area);
    for stage in STAGES {
        if let Some(&index) = stage.iter().find(|&&i| !state.collected.contains(&targets[i])) {
            return targets[index];
        }
    }
    [0.5, 0.5]
}

fn bad(error: ProtocolError) -> ChannelError {
    ChannelError::new(false, error)
}

impl SimulatedTracker {
    pub fn new(options: Simulation) -> Self {
        let state = TrackerState {
            area: if options.mapped { ALBEDO_AREA } else { UNMAPPED_AREA },
            calibration_id: options.calibration_id,
            blob: calibration_blob(&INTERACTIVE_POINTS, options.calibration_id),
            fail_compute_at: options.fail_compute_at,
            weak_points: options.weak_points,
            collect_s: options.collect_s,
            compute_s: options.compute_s,
            user: options.user,
            log: Vec::new(),
            calibrating: false,
            collected: Vec::new(),
            computes: 0,
            subscribed: false,
            collecting: None,
        };
        Self {
            state: Arc::new(Mutex::new(state)),
            looking_at: None,
            requests: Decoder::default(),
            outgoing: VecDeque::new(),
            next_gaze: 0.0,
            noise: Noise(0x9E37_79B9_7F4A_7C15 ^ 7),
            epoch: Instant::now(),
        }
    }

    fn now(&self) -> f64 {
        self.epoch.elapsed().as_secs_f64()
    }

    fn send(&mut self, message: &Message, delay_s: f64) -> Result<(), ChannelError> {
        let at = self.now() + delay_s;
        let encoded = message.encode().map_err(bad)?;
        let position = self.outgoing.iter().position(|(due, _)| *due > at).unwrap_or(self.outgoing.len());
        self.outgoing.insert(position, (at, encoded));
        Ok(())
    }

    fn reply(&mut self, request: &Message, items: &[Field], status: u32, delay_s: f64) -> Result<(), ChannelError> {
        let payload = if items.is_empty() { Vec::new() } else { encode_payload(items).map_err(bad)? };
        let reply = Message::new(RESPONSE_KIND, request.request_id, 0, request.command, status, payload);
        self.send(&reply, delay_s)
    }

    fn handle(&mut self, request: &Message) -> Result<(), ChannelError> {
        let now = self.now();
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.log.push(request.clone());
        let (items, status, delay): (Vec<Field>, u32, f64) = match request.command {
            command::GET_DEVICE_STATUS => {
                let id = string_field(&state.calibration_id.to_string()).map_err(bad)?;
                let header = struct_field(2, STATUS_STRUCT).map_err(bad)?;
                (vec![header, uint32_field(STATUS_CALIBRATION_ID as u32), id], 0, 0.0)
            }
            command::GET_DISPLAY_AREA => {
                let mut items = Vec::new();
                for corner in state.area.corners() {
                    items.extend(triple_fixed42_fields(corner).map_err(bad)?);
                }
                items.extend(array_fields(0, DISPLAY_AREA_TRAILER).map_err(bad)?);
                (items, 0, 0.0)
            }
            command::GET_GEOMETRY_MOUNTING => {
                let m = ET5_MOUNTING;
                let mut items = vec![
                    uint32_field(m.guides as u32),
                    fixed42_field(m.width_mm).map_err(bad)?,
                    fixed42_field(m.angle_deg).map_err(bad)?,
                ];
                items.extend(triple_fixed42_fields(m.external_offset_mm).map_err(bad)?);
                items.extend(triple_fixed42_fields(m.internal_offset_mm).map_err(bad)?);
                (items, 0, 0.0)
            }
            command::SUBSCRIBE_STREAM | command::UNSUBSCRIBE_STREAM => {
                state.subscribed = request.command == command::SUBSCRIBE_STREAM;
                if state.subscribed {
                    state.user.sit_down(now);
                }
                (Vec::new(), 0, 0.0)
            }
            gc::SET_DISPLAY_AREA => {
                let numbers: Vec<f64> = values(&request.payload)
                    .map_err(bad)?
                    .into_iter()
                    .filter_map(|v| if let Value::Float(x) = v { Some(x) } else { None })
                    .collect();
                if let Ok(area) = DisplayArea::from_floats(&numbers) {
                    state.area = area;
                }
                (Vec::new(), 0, 0.0)
            }
            gc::CALIBRATION_RETRIEVE => (vec![blob_field(&state.blob)], 0, 0.0),
            gc::GET_CALIBRATION_STIMULUS_POINTS => {
                let points: Vec<[f64; 2]> =
                    gd::calibration_points(&state.blob).map_err(bad)?.iter().map(|r| r.point).collect();
                let quality = stimulus_quality(&points, &state.weak_points);
                (gd::stimulus_points_payload(&quality).map_err(bad)?, 0, 0.0)
            }
            gc::CALIBRATION_START => {
                let status = if state.calibrating { STATE_ERROR } else { 0 };
                state.calibrating = true;
                (Vec::new(), status, 0.0)
            }
            gc::CALIBRATION_STOP => {
                let status = if state.calibrating { 0 } else { STATE_ERROR };
                state.calibrating = false;
                (Vec::new(), status, 0.0)
            }
            gc::CALIBRATION_CLEAR => {
                state.collected.clear();
                (Vec::new(), 0, 0.0)
            }
            gc::CALIBRATION_COLLECT_2D => {
                if !state.calibrating {
                    (Vec::new(), STATE_ERROR, 0.0)
                } else {
                    let point = request_point(request)?;
                    state.collected.push(point);
                    state.collecting = Some((point, now + state.collect_s));
                    (Vec::new(), 0, state.collect_s)
                }
            }
            gc::CALIBRATION_DISCARD_2D => {
                let point = request_point(request)?;
                if let Some(at) = state.collected.iter().position(|p| *p == point) {
                    state.collected.remove(at);
                }
                (Vec::new(), 0, 0.0)
            }
            gc::CALIBRATION_COMPUTE_AND_APPLY => {
                state.computes += 1;
                if state.fail_compute_at.contains(&state.computes) || !state.calibrating {
                    (Vec::new(), OPERATION_FAILED, state.compute_s)
                } else {
                    state.calibration_id =
                        state.calibration_id.wrapping_mul(31).wrapping_add(state.collected.len() as u32);
                    state.blob = calibration_blob(&state.collected, state.calibration_id);
                    (Vec::new(), 0, state.compute_s)
                }
            }
            gc::CALIBRATION_APPLY => {
                let blob = match fields(&request.payload).map_err(bad)?.first().map(value) {
                    Some(Ok(Value::Bytes(blob))) => blob,
                    _ => return Err(bad(ProtocolError::new("1110 carries no blob"))),
                };
                if state.calibrating {
                    let points = gd::calibration_points(&blob).map_err(bad)?;
                    state.collected.extend(points.iter().map(|r| r.point));
                } else {
                    state.calibration_id = gd::blob_calibration_id(&blob).map_err(bad)?;
                    state.blob = blob;
                }
                (Vec::new(), 0, 0.0)
            }
            _ => (Vec::new(), 0, 0.0),
        };
        drop(state);
        self.reply(request, &items, status, delay)
    }

    fn stream(&mut self, now: f64) -> Result<(), ChannelError> {
        let subscribed = self.state.lock().unwrap_or_else(PoisonError::into_inner).subscribed;
        if !subscribed || now < self.next_gaze {
            return Ok(());
        }
        self.next_gaze = now + GAZE_PERIOD.as_secs_f64();
        let columns = self.gaze_columns(now).map_err(bad)?;
        let message = build::stream_message(Stream::Gaze, &columns).map_err(bad)?;
        self.send(&message, 0.0)
    }

    /// The gaze columns the device would send now.
    pub fn gaze_columns(&mut self, now: f64) -> Result<Vec<(u32, Vec<Field>)>, ProtocolError> {
        let (present, head, built_in) = {
            let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            (state.user.present, state.user.head(now), default_looking_at(&state, now))
        };
        let timestamp = build::int64_column((now * 1e6) as i64);
        if !present {
            return Ok(vec![
                (1, timestamp),
                (7, build::uint32_column(EYE_INVALID)),
                (13, build::uint32_column(EYE_INVALID)),
                (38, build::uint32_column(0)),
                (40, build::uint32_column(0)),
            ]);
        }
        let [x, y, z] = head;
        let left = [x + EYE_SPACING / 2.0, y, z];
        let right = [x - EYE_SPACING / 2.0, y, z];
        let target = self.looking_at.as_ref().and_then(|looking| looking()).unwrap_or(built_in);
        let point = [target[0] + self.noise.gauss(0.002), target[1] + self.noise.gauss(0.002)];
        Ok(vec![
            (1, timestamp),
            (5, build::vec2_column(point)?),
            (11, build::vec2_column(point)?),
            (7, build::uint32_column(0)),
            (13, build::uint32_column(0)),
            (3, build::vec3_column(left)?),
            (9, build::vec3_column(right)?),
            (29, build::uint32_column(1)),
            (30, build::uint32_column(1)),
            (37, build::vec3_column(left)?),
            (38, build::uint32_column(1)),
            (39, build::vec3_column(right)?),
            (40, build::uint32_column(1)),
        ])
    }
}

fn request_point(request: &Message) -> Result<[f64; 2], ChannelError> {
    let items = fields(&request.payload).map_err(bad)?;
    let number = |at: usize| match items.get(at).map(value) {
        Some(Ok(Value::Float(x))) => Ok(float32(x)),
        _ => Err(bad(ProtocolError::new("Calibration point request carries no point"))),
    };
    Ok([number(0)?, number(1)?])
}

impl Channel for SimulatedTracker {
    fn write(&mut self, data: &[u8]) -> Result<(), ChannelError> {
        for request in self.requests.feed(data).map_err(bad)? {
            self.handle(&request)?;
        }
        Ok(())
    }

    fn read(&mut self, timeout_ms: u32) -> Result<Vec<u8>, ChannelError> {
        let deadline = self.now() + f64::from(timeout_ms) / 1000.0;
        loop {
            let now = self.now();
            self.stream(now)?;
            if self.outgoing.front().is_some_and(|(due, _)| *due <= now) {
                return Ok(self.outgoing.pop_front().map(|(_, data)| data).unwrap_or_default());
            }
            if now >= deadline {
                return Ok(Vec::new());
            }
            std::thread::sleep(Duration::from_secs_f64((deadline - now).min(0.005)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tobii_device::TrackerSession;
    use tobii_gaze::operation::{Operation, run_calls};
    use tobii_protocol::gaze::gaze_message;

    #[test]
    fn the_blob_reads_back_with_its_points_and_id() {
        let blob = calibration_blob(&[[0.5, 0.5], [0.1, 0.9]], 0x1234);
        assert_eq!(gd::blob_calibration_id(&blob).unwrap(), 0x1234);
        let points = gd::calibration_points(&blob).unwrap();
        assert_eq!(points.iter().map(|p| p.point).collect::<Vec<_>>(), [[0.5, 0.5], [float32(0.1), float32(0.9)]]);
    }

    #[test]
    fn display_area_and_mounting_round_trip_through_the_session() {
        let mut session = TrackerSession::new(SimulatedTracker::new(Simulation::default()));
        let area = run_calls(Operation::new(tobii_gaze::display::read_display_area), &mut session).unwrap();
        assert_eq!(area, ALBEDO_AREA);
        let mounting = run_calls(Operation::new(tobii_gaze::display::read_mounting), &mut session).unwrap();
        assert_eq!(mounting.as_float32(), ET5_MOUNTING.as_float32());
        let unmapped = SimulatedTracker::new(Simulation { mapped: false, ..Simulation::default() });
        let mut session = TrackerSession::new(unmapped);
        let area = run_calls(Operation::new(tobii_gaze::display::read_display_area), &mut session).unwrap();
        assert!(!tobii_gaze::display::is_mapped(Some(&area)));
    }

    #[test]
    fn backup_reads_the_stored_calibration() {
        let mut session = TrackerSession::new(SimulatedTracker::new(Simulation::default()));
        let backup = Operation::new(|io| async move { tobii_gaze::calibration::backup_calibration(&io).await });
        let calibration = run_calls(backup, &mut session).unwrap();
        assert_eq!(calibration.calibration_id, DEFAULT_CALIBRATION_ID);
        assert_eq!(gd::calibration_points(&calibration.blob).unwrap().len(), 7);
    }

    #[test]
    fn subscribed_gaze_streams_the_user_position_guide() {
        let user = SimulatedUser::seated_after(0.0);
        let mut session = TrackerSession::new(SimulatedTracker::new(Simulation { user, ..Simulation::default() }));
        session.subscribe(Stream::Gaze).unwrap();
        let mut seen = Vec::new();
        while seen.is_empty() {
            for message in session.receive(100).unwrap() {
                seen.extend(gaze_message(&message).unwrap());
            }
        }
        let g = &seen[0];
        assert_eq!((g.left_position_guide_validity, g.right_position_guide_validity), (1, 1));
        assert!(g.left_position_guide[0] > g.right_position_guide[0]);
        assert!((g.left_gaze_point_2d[0] - 0.5).abs() < 0.05);
    }

    #[test]
    fn the_user_looks_at_targets_scaled_into_a_large_display() {
        let mut tracker = SimulatedTracker::new(Simulation::default());
        let large = tobii_gaze::display::compose_display_area([800.0, 450.0], &ET5_MOUNTING, 0.0);
        let mut state = tracker.state.lock().unwrap();
        assert_eq!(default_looking_at(&state, 0.0), [0.5, 0.5]);
        state.area = large;
        state.collected.push([0.5, 0.5]);
        let target = default_looking_at(&state, 0.0);
        drop(state);
        assert!((target[0] - (0.125 + 0.1 * 0.75)).abs() < 1e-6, "{target:?}");
        assert!(tracker.gaze_columns(1.0).is_ok());
    }

    #[test]
    fn collect_outside_a_calibration_is_refused() {
        let mut session = TrackerSession::new(SimulatedTracker::new(Simulation::default()));
        let result = session.try_call(|id| gd::calibration_collect_2d(id, 0.5, 0.5, gd::Eye::Both));
        assert!(result.is_err());
    }
}
