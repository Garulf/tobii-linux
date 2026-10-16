//! The guided setup of the Eye Tracker 5 as a native GTK 4 app (`tobii-linux-setup`).
//!
//! The flow ([`flow::SetupFlow`]) runs on a worker thread that owns the tracker session and
//! drives tobii-gaze's operations; the window only draws the screens the flow publishes on
//! a [`channel::UiChannel`] and reports what the user does, so device I/O never runs on the
//! GTK main loop. The window ([`ui`], behind the `gtk` feature) is the only part that
//! needs GTK. What the screens show over time (the alignment step, the positioning and
//! calibration timelines, the keys) is plain state in [`alignment`], [`positioning_view`],
//! [`particles`] and [`keys`], tested without a display. [`simulated::SimulatedTracker`]
//! stands in for the tracker to try the pages without hardware.

pub mod alignment;
pub mod artwork;
pub mod channel;
pub mod cli;
pub mod device;
pub mod flow;
pub mod keys;
pub mod motion;
pub mod particles;
pub mod positioning_view;
pub mod screen;
pub mod simulated;
pub mod style;
#[cfg(feature = "gtk")]
pub mod ui;
