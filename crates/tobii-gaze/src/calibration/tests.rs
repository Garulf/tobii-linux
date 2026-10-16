//! The calibration sequence against a scripted device that follows the command semantics
//! recovered from the runtime and the Tobii client (docs/gaze-calibration.md), ported from
//! tests/test_calibration.py. Not device traffic.

use std::collections::VecDeque;

use tobii_device::{Channel, ChannelError, TrackerSession};
use tobii_protocol::commands::{RESPONSE_KIND, Stream, blob_field, command, string_field};
use tobii_protocol::fields::{Value, encode_payload, struct_field, triple_fixed42_fields, uint32_field, values};
use tobii_protocol::gaze::build::{int64_column, stream_message, uint32_column, vec2_column};
use tobii_protocol::ttp::Decoder;
use tobii_protocol::{Field, Message};

use super::*;
use crate::device::{EyeQuality, calibration_record, stimulus_points_payload};
use crate::operation::{CalibrationUi, DriverError, LogRecord, run};

const STATE_ERROR: u32 = 0x2000_0508;
const OPERATION_FAILED_STATUS: u32 = 0x2000_0407;
const START: u32 = gc::CALIBRATION_START;
const STOP: u32 = gc::CALIBRATION_STOP;
const CLEAR: u32 = gc::CALIBRATION_CLEAR;
const COLLECT: u32 = gc::CALIBRATION_COLLECT_2D;
const DISCARD: u32 = gc::CALIBRATION_DISCARD_2D;
const COMPUTE: u32 = gc::CALIBRATION_COMPUTE_AND_APPLY;
const APPLY: u32 = gc::CALIBRATION_APPLY;
const RETRIEVE: u32 = gc::CALIBRATION_RETRIEVE;
const STIMULUS: u32 = gc::GET_CALIBRATION_STIMULUS_POINTS;
const STATUS: u32 = command::GET_DEVICE_STATUS;
const SUBSCRIBE: u32 = command::SUBSCRIBE_STREAM;
const UNSUBSCRIBE: u32 = command::UNSUBSCRIBE_STREAM;
const FULL_SEQUENCE: [u32; 18] = [
    SUBSCRIBE,
    START,
    CLEAR,
    COLLECT,
    COMPUTE,
    COLLECT,
    COLLECT,
    COLLECT,
    COMPUTE,
    COLLECT,
    COLLECT,
    COLLECT,
    COMPUTE,
    STOP,
    UNSUBSCRIBE,
    RETRIEVE,
    STATUS,
    STIMULUS,
];

fn make_blob(points: &[[f64; 2]], calibration_id: u32) -> Vec<u8> {
    let mut header: Vec<u8> = [110, 84, 1, 0, calibration_id].iter().flat_map(|w: &u32| w.to_le_bytes()).collect();
    let mut name = b"human".to_vec();
    name.resize(64, 0);
    header.extend(name);
    let mut blob = ((4 + header.len()) as u32).to_le_bytes().to_vec();
    blob.extend(header);
    blob.extend(0x5A5A_5A5Au32.to_le_bytes());
    blob.extend((points.len() as i32).to_le_bytes());
    for &[x, y] in points {
        blob.extend(calibration_record([x, y], ([x + 0.01, y], 1), ([x, y - 0.01], 1)).unwrap());
    }
    blob
}

fn gaze_at(timestamps_ms: impl IntoIterator<Item = i64>, point: [f64; 2]) -> Vec<Message> {
    timestamps_ms
        .into_iter()
        .map(|t| {
            let columns = [
                (1, int64_column(t * 1000)),
                (7, uint32_column(0)),
                (13, uint32_column(0)),
                (5, vec2_column(point).unwrap()),
                (11, vec2_column(point).unwrap()),
            ];
            stream_message(Stream::Gaze, &columns).unwrap()
        })
        .collect()
}

type GazeDuringCollect = Box<dyn Fn(usize, [f64; 2]) -> Vec<Message> + Send>;

struct CalibrationDevice {
    calibration_id: u32,
    blob: Vec<u8>,
    calibrating: bool,
    collected: Vec<[f64; 2]>,
    computes: usize,
    collects: usize,
    fail_compute_at: Option<usize>,
    refuse_every_compute: bool,
    fail_collect_at: Option<usize>,
    ignore_apply: bool,
    gaze_during_collect: GazeDuringCollect,
    requests: Decoder,
    outgoing: VecDeque<Vec<u8>>,
    log: Vec<Message>,
}

impl CalibrationDevice {
    fn new() -> Self {
        Self::with_points(&[[0.5, 0.5]])
    }

    fn with_points(points: &[[f64; 2]]) -> Self {
        Self {
            calibration_id: 0xBC38_1A37,
            blob: make_blob(points, 0xBC38_1A37),
            calibrating: false,
            collected: Vec::new(),
            computes: 0,
            collects: 0,
            fail_compute_at: None,
            refuse_every_compute: false,
            fail_collect_at: None,
            ignore_apply: false,
            gaze_during_collect: Box::new(|_, _| Vec::new()),
            requests: Decoder::default(),
            outgoing: VecDeque::new(),
            log: Vec::new(),
        }
    }

    fn reply(&mut self, request: &Message, items: &[Field], word4: u32) {
        let payload = if items.is_empty() { Vec::new() } else { encode_payload(items).unwrap() };
        let reply = Message::new(RESPONSE_KIND, request.request_id, 0, request.command, word4, payload);
        self.outgoing.push_back(reply.encode().unwrap());
    }

    fn point_of(request: &Message) -> ([f64; 2], i64) {
        let values = values(&request.payload).unwrap();
        let number = |v: &Value| match v {
            Value::Float(x) => *x,
            _ => panic!("not a coordinate"),
        };
        ([number(&values[0]), number(&values[1])], values[2].as_int().unwrap())
    }

    fn handle(&mut self, request: &Message) {
        match request.command {
            STATUS => {
                let items = [
                    struct_field(2, crate::device::STATUS_STRUCT).unwrap(),
                    uint32_field(7),
                    string_field(&self.calibration_id.to_string()).unwrap(),
                ];
                self.reply(request, &items, 0);
            }
            command::GET_DISPLAY_AREA => {
                let mut items = Vec::new();
                for corner in [[-296.625, 326.0, 111.8125], [300.375, 326.0, 111.8125], [-296.625, 10.25, -3.125]] {
                    items.extend(triple_fixed42_fields(corner).unwrap());
                }
                self.reply(request, &items, 0);
            }
            RETRIEVE => {
                let blob = blob_field(&self.blob);
                self.reply(request, &[blob], 0);
            }
            STIMULUS => {
                let points: Vec<StimulusPoint> = device::calibration_points(&self.blob)
                    .unwrap()
                    .iter()
                    .map(|r| StimulusPoint {
                        point: [r.point[0], r.point[1], 0.0],
                        left: EyeQuality { status: 2, bias: 0.0125, precision: 0.03125 },
                        right: EyeQuality { status: 2, bias: 0.015625, precision: 0.0625 },
                    })
                    .collect();
                self.reply(request, &stimulus_points_payload(&points).unwrap(), 0);
            }
            START => {
                self.reply(request, &[], if self.calibrating { STATE_ERROR } else { 0 });
                self.calibrating = true;
            }
            STOP => {
                self.reply(request, &[], if self.calibrating { 0 } else { STATE_ERROR });
                self.calibrating = false;
            }
            CLEAR => {
                self.collected.clear();
                self.reply(request, &[], 0);
            }
            COLLECT => self.collect(request),
            DISCARD => {
                let (point, _) = Self::point_of(request);
                let position = self.collected.iter().position(|p| *p == point).unwrap();
                self.collected.remove(position);
                self.reply(request, &[], 0);
            }
            COMPUTE => {
                self.computes += 1;
                if self.refuse_every_compute || Some(self.computes) == self.fail_compute_at || !self.calibrating {
                    self.reply(request, &[], OPERATION_FAILED_STATUS);
                    return;
                }
                self.calibration_id = self.calibration_id.wrapping_mul(31).wrapping_add(self.collected.len() as u32);
                self.blob = make_blob(&self.collected, self.calibration_id);
                self.reply(request, &[], 0);
            }
            APPLY => {
                let Value::Bytes(blob) = values(&request.payload).unwrap().remove(0) else { panic!("no blob") };
                if self.calibrating {
                    self.collected.extend(device::calibration_points(&blob).unwrap().iter().map(|r| r.point));
                } else if !self.ignore_apply {
                    self.calibration_id = device::blob_calibration_id(&blob).unwrap();
                    self.blob = blob;
                }
                self.reply(request, &[], 0);
            }
            _ => self.reply(request, &[], 0),
        }
    }

    fn collect(&mut self, request: &Message) {
        self.collects += 1;
        if !self.calibrating || Some(self.collects) == self.fail_collect_at {
            self.reply(request, &[], if self.calibrating { OPERATION_FAILED_STATUS } else { STATE_ERROR });
            return;
        }
        let (point, _) = Self::point_of(request);
        self.collected.push(point);
        for message in (self.gaze_during_collect)(self.collects, point) {
            self.outgoing.push_back(message.encode().unwrap());
        }
        self.reply(request, &[], 0);
    }

    fn commands(&self) -> Vec<u32> {
        self.log.iter().map(|m| m.command).collect()
    }
}

impl Channel for CalibrationDevice {
    fn write(&mut self, data: &[u8]) -> Result<(), ChannelError> {
        for request in self.requests.feed(data).unwrap() {
            self.log.push(request.clone());
            self.handle(&request);
        }
        Ok(())
    }

    fn read(&mut self, _timeout_ms: u32) -> Result<Vec<u8>, ChannelError> {
        Ok(self.outgoing.pop_front().unwrap_or_default())
    }
}

#[derive(Default)]
struct RecordingUi {
    events: Vec<UiEvent>,
    interrupt_at: Option<usize>,
}

impl CalibrationUi for RecordingUi {
    fn event(&mut self, event: &UiEvent) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.events.push(event.clone());
        match event {
            UiEvent::ShowPoint { index, .. } if Some(*index) == self.interrupt_at => Err("cancelled".into()),
            _ => Ok(()),
        }
    }
}

struct Outcome {
    result: OperationResult<CalibrationOutcome, DriverError>,
    session: TrackerSession<CalibrationDevice>,
    progress: Progress,
    log: Vec<LogRecord>,
}

fn calibrate(device: CalibrationDevice, calibrator: Calibrator, ui: &mut RecordingUi) -> Outcome {
    let mut session = TrackerSession::new(device);
    let (operation, progress) = calibrator.operation();
    let mut log = Vec::new();
    let result = run(operation, &mut session, ui, &mut |record| log.push(record.clone()), &mut Vec::new());
    let progress = progress.lock().unwrap().clone();
    Outcome { result, session, progress, log }
}

fn float32_points(points: &[[f64; 2]]) -> Vec<[f64; 2]> {
    points.iter().map(|p| p.map(float32)).collect()
}

fn failed_step(result: &OperationResult<CalibrationOutcome, DriverError>) -> &str {
    match result {
        Err(OperationError::Failed { step, .. }) => step,
        _ => panic!("expected CalibrationFailed"),
    }
}

#[test]
fn sequence_targets_and_outcome() {
    let mut ui = RecordingUi::default();
    let run = calibrate(CalibrationDevice::new(), Calibrator::default(), &mut ui);
    let outcome = run.result.unwrap();
    let device = run.session.channel();
    assert_eq!(device.commands(), FULL_SEQUENCE);
    let collects: Vec<([f64; 2], i64)> =
        device.log.iter().filter(|m| m.command == COLLECT).map(CalibrationDevice::point_of).collect();
    assert_eq!(collects.iter().map(|(p, _)| *p).collect::<Vec<_>>(), float32_points(&INTERACTIVE_POINTS));
    assert!(collects.iter().all(|(_, eye)| *eye == 3));
    assert_eq!(
        outcome.calibration,
        LocalCalibration { calibration_id: device.calibration_id, blob: device.blob.clone() }
    );
    assert_eq!(outcome.stimulus_points.len(), 7);
    assert_eq!(outcome.points.iter().map(|p| p.point).collect::<Vec<_>>(), float32_points(&INTERACTIVE_POINTS));
    let stages: Vec<&UiEvent> = ui.events.iter().filter(|e| matches!(e, UiEvent::StageDone { .. })).collect();
    assert_eq!(
        stages,
        [&UiEvent::StageDone { stage: 0 }, &UiEvent::StageDone { stage: 1 }, &UiEvent::StageDone { stage: 2 }]
    );
    assert!(!device.calibrating);
    assert_eq!(
        run.log[0],
        LogRecord { kind: "command", values: vec![("step", LogValue::Text("subscribe gaze".into()))] }
    );
}

#[test]
fn improve_reapplies_the_previous_calibration_after_clear() {
    let points = [[0.5, 0.5]; 7];
    let previous = LocalCalibration { calibration_id: 0xBC38_1A37, blob: make_blob(&points, 0xBC38_1A37) };
    let calibrator = Calibrator { previous: Some(previous), ..Calibrator::default() };
    let run = calibrate(CalibrationDevice::with_points(&points), calibrator, &mut RecordingUi::default());
    assert_eq!(run.session.channel().commands()[..4], [SUBSCRIBE, START, CLEAR, APPLY]);
    assert_eq!(run.result.unwrap().points.len(), 14);
}

#[test]
fn compute_failure_stops_without_apply() {
    let device = CalibrationDevice { fail_compute_at: Some(2), ..CalibrationDevice::new() };
    let before = device.blob.clone();
    let run = calibrate(device, Calibrator { restarts: 0, ..Calibrator::default() }, &mut RecordingUi::default());
    assert_eq!(failed_step(&run.result), "compute stage 1 (1070)");
    let device = run.session.channel();
    assert_eq!(device.commands(), [&FULL_SEQUENCE[..9], &[STOP, UNSUBSCRIBE]].concat());
    assert!(!device.commands().contains(&APPLY));
    assert!(!device.calibrating);
    assert_ne!(device.blob, before);
}

#[test]
fn refused_compute_restarts_from_the_first_target() {
    let device = CalibrationDevice { fail_compute_at: Some(1), ..CalibrationDevice::new() };
    let mut ui = RecordingUi::default();
    let run = calibrate(device, Calibrator::default(), &mut ui);
    let commands = run.session.channel().commands();
    assert_eq!(commands[..9], [SUBSCRIBE, START, CLEAR, COLLECT, COMPUTE, STIMULUS, STOP, START, CLEAR]);
    assert_eq!(commands[9..], FULL_SEQUENCE[3..]);
    let failures: Vec<(usize, usize)> = run.progress.compute_failures.iter().map(|f| (f.stage, f.attempt)).collect();
    assert_eq!(failures, [(0, 0)]);
    assert!(ui.events[1..].contains(&UiEvent::ShowPoint { index: 0, x: 0.5, y: 0.5 }));
    assert!(ui.events.contains(&UiEvent::Restarting { attempt: 1 }));
    assert_eq!(run.result.unwrap().points.len(), 7);
    assert!(run.log.iter().any(|r| r.kind == "compute_failed"));
}

#[test]
fn compute_refused_every_time_gives_up() {
    let device = CalibrationDevice { refuse_every_compute: true, ..CalibrationDevice::new() };
    let run = calibrate(device, Calibrator { restarts: 1, ..Calibrator::default() }, &mut RecordingUi::default());
    assert!(matches!(run.result, Err(OperationError::Failed { .. })));
    let commands = run.session.channel().commands();
    assert_eq!(commands.iter().filter(|&&c| c == COMPUTE).count(), 2);
    assert_eq!(commands[commands.len() - 2..], [STOP, UNSUBSCRIBE]);
    assert!(!commands.contains(&APPLY));
}

#[test]
fn collect_failure_stops() {
    let device = CalibrationDevice { fail_collect_at: Some(3), ..CalibrationDevice::new() };
    let run = calibrate(device, Calibrator::default(), &mut RecordingUi::default());
    assert_eq!(failed_step(&run.result), "target 2");
    let commands = run.session.channel().commands();
    assert_eq!(commands[commands.len() - 3..], [COLLECT, STOP, UNSUBSCRIBE]);
}

#[test]
fn start_failure_sends_no_stop() {
    let device = CalibrationDevice { calibrating: true, ..CalibrationDevice::new() };
    let run = calibrate(device, Calibrator::default(), &mut RecordingUi::default());
    assert_eq!(failed_step(&run.result), "calibration start (1010)");
    assert_eq!(run.session.channel().commands(), [SUBSCRIBE, START, UNSUBSCRIBE]);
}

#[test]
fn ui_failure_stops_and_reports_the_target() {
    let mut ui = RecordingUi { interrupt_at: Some(4), ..RecordingUi::default() };
    let run = calibrate(CalibrationDevice::new(), Calibrator::default(), &mut ui);
    assert_eq!(failed_step(&run.result), "target 4");
    let commands = run.session.channel().commands();
    assert_eq!(commands[commands.len() - 2..], [STOP, UNSUBSCRIBE]);
    assert!(!commands.contains(&APPLY));
}

#[test]
fn focus_change_discards_and_collects_again() {
    let device = CalibrationDevice {
        gaze_during_collect: Box::new(|number, point| {
            let looking_at = if number == 3 { INTERACTIVE_POINTS[3] } else { point };
            let start = 1000 * number as i64;
            gaze_at((start..start + 600).step_by(30), looking_at)
        }),
        ..CalibrationDevice::new()
    };
    let mut ui = RecordingUi::default();
    let calibrator = Calibrator { focus: Some(FocusCheck { screen_px: [2560.0, 1440.0] }), ..Calibrator::default() };
    let run = calibrate(device, calibrator, &mut ui);
    let device = run.session.channel();
    assert_eq!(device.commands()[..9], [SUBSCRIBE, START, CLEAR, COLLECT, COMPUTE, COLLECT, COLLECT, DISCARD, COLLECT]);
    assert_eq!(CalibrationDevice::point_of(&device.log[7]).0, CalibrationDevice::point_of(&device.log[6]).0);
    assert!(ui.events.contains(&UiEvent::PointDone { index: 2, accepted: false }));
    let outcome = run.result.unwrap();
    assert_eq!(outcome.discarded, [(2, 1)]);
    assert_eq!(outcome.points.len(), 7);
}

#[test]
fn focus_lost_on_every_attempt_fails() {
    let device = CalibrationDevice {
        gaze_during_collect: Box::new(|number, point| if number > 1 { gaze_at([0, 600], point) } else { Vec::new() }),
        ..CalibrationDevice::new()
    };
    let calibrator = Calibrator { focus: Some(FocusCheck::default()), max_attempts: 2, ..Calibrator::default() };
    let run = calibrate(device, calibrator, &mut RecordingUi::default());
    assert!(matches!(&run.result, Err(OperationError::Failed { cause, .. })
        if matches!(**cause, OperationError::Gaze(GazeError::Calibration(_)))));
    assert_eq!(run.session.channel().commands()[5..10], [COLLECT, DISCARD, COLLECT, DISCARD, STOP]);
}

fn backup(device: CalibrationDevice) -> (OperationResult<LocalCalibration, DriverError>, CalibrationDevice) {
    let mut session = TrackerSession::new(device);
    let operation = crate::operation::Operation::new(|io| async move { backup_calibration(&io).await });
    let result = crate::operation::run_calls(operation, &mut session);
    (result, session.into_channel())
}

#[test]
fn backup_reads_status_blob_status_and_saves_a_setting_file() {
    let device = CalibrationDevice::with_points(&[[0.5, 0.5], [0.1, 0.9]]);
    let blob = device.blob.clone();
    let (result, device) = backup(device);
    let backup = result.unwrap();
    assert_eq!(device.commands(), [STATUS, RETRIEVE, STATUS]);
    assert_eq!(backup, LocalCalibration { calibration_id: 0xBC38_1A37, blob: blob.clone() });
    let directory = std::env::temp_dir().join(format!("tobii-gaze-backup-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("calibration.setpm");
    save_backup(&path, &backup, true).unwrap();
    let data = std::fs::read(&path).unwrap();
    assert_eq!(
        data[..16],
        [
            &4u32.to_le_bytes()[..],
            &1u32.to_le_bytes(),
            &((data.len() - 12) as u32).to_le_bytes(),
            &0xBC38_1A37u32.to_le_bytes()
        ]
        .concat()
    );
    assert_eq!(data[16..], blob);
    assert_eq!(load_backup(&path).unwrap(), backup);
    assert!(
        matches!(save_backup(&path, &backup, false), Err(GazeError::Io(e)) if e.kind() == std::io::ErrorKind::AlreadyExists)
    );
    std::fs::remove_dir_all(&directory).unwrap();
    let check = BackupCheck::new(&backup, &LocalCalibration { calibration_id: 7, blob: make_blob(&[[0.9, 0.1]], 7) });
    assert_eq!((check.ids_match(), check.blob_matches(), check.ok()), (false, false, false));
}

fn restore(
    device: CalibrationDevice,
    saved: LocalCalibration,
) -> (OperationResult<u32, DriverError>, CalibrationDevice) {
    let mut session = TrackerSession::new(device);
    let operation = crate::operation::Operation::new(|io| async move { restore_calibration(&io, &saved).await });
    let result = crate::operation::run_calls(operation, &mut session);
    (result, session.into_channel())
}

#[test]
fn restore_applies_and_checks_the_id() {
    let saved = LocalCalibration { calibration_id: 0xBC38_1A37, blob: make_blob(&[[0.5, 0.5]], 0xBC38_1A37) };
    let device = CalibrationDevice { calibration_id: 5, ..CalibrationDevice::new() };
    let (result, device) = restore(device, saved.clone());
    assert_eq!(result.unwrap(), 0xBC38_1A37);
    assert_eq!(device.commands(), [APPLY, STATUS]);
    assert_eq!(device.blob, saved.blob);
    let ignoring = CalibrationDevice { calibration_id: 5, ignore_apply: true, ..CalibrationDevice::new() };
    assert!(matches!(restore(ignoring, saved).0, Err(OperationError::Gaze(GazeError::Calibration(_)))));
}

fn samples(entries: &[(i64, Option<[f64; 2]>)]) -> Vec<GazeSample> {
    entries.iter().map(|&(t, point)| GazeSample { timestamp_us: t * 1000, point }).collect()
}

fn fixation_of(points: &[Option<[f64; 2]>]) -> bool {
    let entries: Vec<(i64, Option<[f64; 2]>)> = points.iter().enumerate().map(|(i, &p)| (i as i64 * 30, p)).collect();
    fixation(&samples(&entries), FIXATION_WINDOW_MS, FIXATION_SPREAD, FIXATION_MIN_SAMPLES)
}

#[test]
fn fixation_needs_a_still_binocular_gaze() {
    let still: Vec<Option<[f64; 2]>> = (0..12).map(|i| Some([0.2 + 0.001 * f64::from(i), 0.3])).collect();
    assert!(fixation_of(&still));
    let mut saccade = vec![Some([0.2, 0.3]); 8];
    saccade.extend([Some([0.5, 0.3]); 4]);
    assert!(!fixation_of(&saccade));
    saccade.extend([Some([0.5, 0.3]); 8]);
    assert!(fixation_of(&saccade));
    let mut missing = vec![Some([0.2, 0.3]); 11];
    missing.push(None);
    assert!(!fixation_of(&missing));
    assert!(!fixation_of(&[Some([0.2, 0.3]); 3]));
}

#[test]
fn focus_check_follows_the_client() {
    let targets = &INTERACTIVE_POINTS[1..4];
    let check = FocusCheck::default();
    let steady: Vec<_> = (0..600).step_by(30).map(|t| (t, Some(targets[0]))).collect();
    assert!(!check.focus_changed(&samples(&steady), targets, 0));
    let mut glance: Vec<_> = (0..300).step_by(30).map(|t| (t, Some(targets[0]))).collect();
    glance.extend([(300, Some(targets[1])), (330, Some(targets[1]))]);
    glance.extend((360..600).step_by(30).map(|t| (t, Some(targets[0]))));
    assert!(!check.focus_changed(&samples(&glance), targets, 0));
    let mut elsewhere = vec![(0, Some(targets[0]))];
    elsewhere.extend((30..600).step_by(30).map(|t| (t, Some(targets[2]))));
    assert!(check.focus_changed(&samples(&elsewhere), targets, 0));
    let missing = [(0, Some(targets[0])), (100, None), (400, None), (430, Some(targets[0]))];
    assert!(check.focus_changed(&samples(&missing), targets, 0));
    let mut between: Vec<_> = (0..200).step_by(30).map(|t| (t, Some([0.3, 0.5]))).collect();
    between.push((200, Some(targets[0])));
    assert!(!check.focus_changed(&samples(&between), targets, 0));
    assert!(!check.focus_changed(&[], targets, 0));
}

#[test]
fn gaze_sample_averages_valid_eyes() {
    let g = DeviceGaze {
        timestamp: 5,
        left_validity: 0,
        right_validity: 4,
        left_gaze_point_2d: [0.25, 0.5],
        right_gaze_point_2d: [0.75, 0.5],
        ..DeviceGaze::default()
    };
    assert_eq!(gaze_sample(&g), GazeSample { timestamp_us: 5, point: Some([0.25, 0.5]) });
    assert_eq!(binocular_sample(&g).point, None);
}

#[test]
fn calibration_area_and_targets() {
    assert_eq!(calibration_area([597.0, 336.1]), [0.0, 0.0, 1.0, 1.0]);
    assert_eq!(
        calibration_targets(Some([597.0, 336.1]), &INTERACTIVE_POINTS)[1],
        [0.10000000149011612, 0.8999999761581421]
    );
    let [x, y, w, h] = calibration_area([800.0, 450.0]);
    assert!((w - 0.75).abs() < 1e-12 && (h - 340.0 / 450.0).abs() < 1e-12 && (x - 0.125).abs() < 1e-12);
    assert!(y > 0.0);
    assert_eq!(calibration_targets(Some([800.0, 450.0]), &INTERACTIVE_POINTS)[0], [0.5, 0.5]);
}

#[test]
fn display_size_from_display_area() {
    let mut device = CalibrationDevice::new();
    let request = header_only_request(1, command::GET_DISPLAY_AREA).unwrap();
    device.write(&request.encode().unwrap()).unwrap();
    let mut decoder = Decoder::default();
    let reply = decoder.feed(&device.read(0).unwrap()).unwrap().remove(0);
    let [width, height] = display_size_mm(&values(&reply.payload).unwrap()).unwrap();
    assert!((width - 597.0).abs() < 1e-9);
    assert!((height - crate::math::hypot(&[326.0 - 10.25, 111.8125 + 3.125])).abs() < 1e-4);
}
