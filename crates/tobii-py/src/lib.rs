#![allow(missing_debug_implementations, reason = "Python classes are inspected from Python, not with Debug")]

//! `tobii_linux._native`: the Rust core behind the Python package during the migration.
//!
//! Each submodule mirrors one Python module, which re-exports or wraps it. Rules that keep
//! the bindings deadlock-free: native mutexes are only locked with the interpreter
//! detached (`py.detach`), and adapters that call back into Python attach on their own.

use pyo3::prelude::*;

mod camera;
mod commands;
mod convert;
mod device;
mod errors;
mod exit;
mod fields;
mod frames;
mod gaze;
mod logging;
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
