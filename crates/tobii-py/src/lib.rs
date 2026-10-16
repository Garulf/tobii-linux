#![allow(missing_debug_implementations, reason = "Python classes are inspected from Python, not with Debug")]

//! `tobii_linux._native`: the Rust core behind the Python package during the migration.
//!
//! Each submodule mirrors one Python module, which re-exports or wraps it. Rules that keep
//! the bindings deadlock-free: native mutexes are only locked with the interpreter
//! detached (`py.detach`), and adapters that call back into Python attach on their own.

use pyo3::prelude::*;

mod calibration;
mod calibration_profiles;
mod camera;
mod commands;
mod convert;
mod device;
mod display_setup;
mod errors;
mod exit;
mod eye_positioning;
mod fields;
mod frames;
mod gaze;
mod gaze_device;
mod gaze_operation;
mod logging;
mod models;
mod monitors;
mod prp;
mod prp_service;
mod prp_session;
mod qpc_clock;
mod sesp;
mod sesp_service;
mod tet_tcp;
mod tet_tcp_service;
mod ttp;
mod usb;

fn submodule<'py>(
    parent: &Bound<'py, PyModule>,
    name: &str,
    fill: impl FnOnce(&Bound<'py, PyModule>) -> PyResult<()>,
) -> PyResult<()> {
    let module = PyModule::new(parent.py(), name)?;
    fill(&module)?;
    parent.add_submodule(&module)
}

#[pymodule]
fn _native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    logging::install();
    exit::register(m)?;
    errors::register(m)?;
    submodule(m, "ttp", ttp::register)?;
    submodule(m, "fields", fields::register)?;
    submodule(m, "commands", commands::register)?;
    submodule(m, "frames", frames::register)?;
    submodule(m, "camera_info", camera::register)?;
    submodule(m, "gaze", gaze::register)?;
    submodule(m, "usb", usb::register)?;
    submodule(m, "device", device::register)?;
    models::register(m)?;
    submodule(m, "gaze_device", gaze_device::register)?;
    submodule(m, "calibration", calibration::register)?;
    submodule(m, "calibration_profiles", calibration_profiles::register)?;
    submodule(m, "display_setup", display_setup::register)?;
    submodule(m, "eye_positioning", eye_positioning::register)?;
    submodule(m, "monitors", monitors::register)?;
    submodule(m, "prp", prp::register)?;
    submodule(m, "prp_session", prp_session::register)?;
    submodule(m, "prp_service", prp_service::register)?;
    submodule(m, "qpc_clock", qpc_clock::register)?;
    submodule(m, "sesp", sesp::register)?;
    submodule(m, "sesp_service", sesp_service::register)?;
    submodule(m, "tet_tcp", tet_tcp::register)?;
    submodule(m, "tet_tcp_service", tet_tcp_service::register)?;
    Ok(())
}
