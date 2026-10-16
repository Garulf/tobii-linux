//! Eye Tracker 5 sessions over one vendor channel: startup, realms, request/reply calls,
//! stream subscription and demultiplexing, clock mapping and reconnect supervision.
//!
//! Built from the runtime's tracker_lib_usb sequence (docs/usb-streams.md) and run against
//! a real tracker. Every type here is generic over [`Channel`], so the same code drives a
//! USB channel, a Python channel during the migration and the fake devices of the tests.

pub mod channel;
pub mod clock;
pub mod error;
pub mod session;
pub mod stop;
pub mod streams;
pub mod supervisor;
pub mod threads;
pub mod usb_session;

pub use channel::{Channel, ChannelError};
pub use clock::{ClockMapper, ClockOffsets, HostClock};
pub use error::{DeviceError, Result};
pub use session::{CameraCalibration, TrackerSession};
pub use stop::{StopFlag, StopSignal};
pub use streams::{StreamItem, StreamReader, StreamValue};
pub use supervisor::{Backoff, LinkState, TrackerLink};
