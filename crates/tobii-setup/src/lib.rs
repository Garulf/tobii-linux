//! The guided setup of the Eye Tracker 5 as a native GTK 4 app (`tobii-linux-setup`).
//!
//! The flow ([`flow::SetupFlow`]) runs on a worker thread that owns the tracker session and
//! drives tobii-gaze's operations; the window only draws the screens the flow publishes on
//! a [`channel::UiChannel`] and reports what the user does, so device I/O never runs on the
//! GTK main loop. [`simulated::SimulatedTracker`] stands in for the tracker to try the
//! pages without hardware.

pub mod channel;
pub mod cli;
pub mod device;
pub mod flow;
pub mod screen;
pub mod simulated;
