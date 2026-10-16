//! Gaze configuration of the Eye Tracker 5: calibration, profiles, display setup, eye
//! positioning and the desktop's monitors.
//!
//! Codecs and models are pure functions checked against the original code by the emulation
//! oracles (tools/gaze_calibration_oracle.py, gaze_device_oracle.py,
//! display_setup_oracle.py). Procedures that talk to the device (backup, restore, display
//! setup, profile switching, the calibration sequence) are [`operation::Operation`]s: state
//! machines a caller drives, with [`operation::run`] driving them on a native
//! [`tobii_device::TrackerSession`] and a [`operation::CalibrationUi`].
//!
//! A setup application calls, in order: [`monitors::linux_monitors`] and
//! [`monitors::initial_monitor`]; [`display::read_display_area`], [`display::read_mounting`],
//! [`display::initial_lines`], [`display::area_from_lines`] and
//! [`display::apply_display_area`]; [`positioning::PositioningGuide`] on
//! [`positioning::guide_sample`]s of the gaze stream; [`calibration::backup_calibration`],
//! [`calibration::calibration_targets`] and a [`calibration::Calibrator`]; finally
//! [`profiles::ProfileStore`] to save and mark the result, or
//! [`calibration::restore_calibration`] to put the previous calibration back.

pub mod calibration;
pub mod device;
pub mod digest;
pub mod display;
pub mod error;
pub mod math;
pub mod monitors;
pub mod operation;
pub mod positioning;
pub mod profiles;

pub use calibration::{CalibrationOutcome, Calibrator, FocusCheck};
pub use device::{Eye, LocalCalibration};
pub use display::{DisplayArea, MountingGeometry};
pub use error::{GazeError, Result};
pub use operation::{Action, Input, Operation, OperationError, Step};
