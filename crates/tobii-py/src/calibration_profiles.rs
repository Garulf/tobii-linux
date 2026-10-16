//! tobii_linux.calibration_profiles: named calibration profiles on disk and switching between
//! them on the device.
//!
//! Profiles cross the boundary as (name, path, (calibration_id, blob)); the Python module
//! keeps its dataclasses and ProfileStore class around the store directory.

use std::path::PathBuf;

use pyo3::IntoPyObjectExt;
use pyo3::prelude::*;
use pyo3::types::PyBytes;
use tobii_gaze::Operation;
use tobii_gaze::profiles::{self, Profile, ProfileStore};

use crate::gaze_device::{eye_arg, local_arg, local_tuple};
use crate::gaze_operation::{IntoGazeResult, ProfileError, PyOperation};

type ProfileTuple<'py> = (String, PathBuf, (u32, Bound<'py, PyBytes>));

fn profile_tuple<'py>(py: Python<'py>, profile: &Profile) -> ProfileTuple<'py> {
    (profile.name.clone(), profile.path.clone(), local_tuple(py, &profile.calibration))
}

#[pyfunction]
fn default_directory(xdg_data_home: Option<&str>, home: Option<&str>) -> PathBuf {
    profiles::default_directory(xdg_data_home, home)
}

#[pyfunction]
fn check_name(name: &str) -> PyResult<String> {
    profiles::check_name(name).map(str::to_string).gaze()
}

#[pyfunction]
fn profile_path(directory: PathBuf, name: &str) -> PyResult<PathBuf> {
    ProfileStore::new(directory).path(name).gaze()
}

#[pyfunction]
fn profile_names(directory: PathBuf) -> PyResult<Vec<String>> {
    ProfileStore::new(directory).names().gaze()
}

#[pyfunction]
fn load_profile<'py>(py: Python<'py>, directory: PathBuf, name: &str) -> PyResult<ProfileTuple<'py>> {
    Ok(profile_tuple(py, &ProfileStore::new(directory).load(name).gaze()?))
}

#[pyfunction]
fn save_profile(directory: PathBuf, name: &str, calibration: &Bound<'_, PyAny>, overwrite: bool) -> PyResult<PathBuf> {
    ProfileStore::new(directory).save(name, &local_arg(calibration)?, overwrite).gaze()
}

#[pyfunction]
fn current_profile(directory: PathBuf) -> PyResult<Option<String>> {
    ProfileStore::new(directory).current().gaze()
}

#[pyfunction]
fn set_current_profile(directory: PathBuf, name: &str) -> PyResult<()> {
    ProfileStore::new(directory).set_current(name).gaze()
}

#[pyfunction]
fn save_from_device(directory: PathBuf, name: String, overwrite: bool) -> PyOperation {
    let store = ProfileStore::new(directory);
    let operation = Operation::new(move |io| profiles::save_from_device(io, store, name, overwrite));
    PyOperation::new(operation, |py, profile| profile_tuple(py, &profile).into_py_any(py))
}

#[pyfunction]
fn switch(directory: PathBuf, name: String, eye: u32, dry_run: bool) -> PyResult<PyOperation> {
    let (store, eye) = (ProfileStore::new(directory), eye_arg(eye)?);
    let operation = Operation::new(move |io| profiles::switch(io, store, name, eye, dry_run));
    Ok(PyOperation::new(operation, |py, result| {
        (result.profile, result.device_id_before, result.device_id_after, result.applied).into_py_any(py)
    }))
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("ProfileError", m.py().get_type::<ProfileError>())?;
    m.add("PROFILE_SUFFIX", profiles::PROFILE_SUFFIX)?;
    m.add("CURRENT_FILE", profiles::CURRENT_FILE)?;
    m.add("MAX_NAME_LENGTH", profiles::MAX_NAME_LENGTH)?;
    m.add_function(wrap_pyfunction!(default_directory, m)?)?;
    m.add_function(wrap_pyfunction!(check_name, m)?)?;
    m.add_function(wrap_pyfunction!(profile_path, m)?)?;
    m.add_function(wrap_pyfunction!(profile_names, m)?)?;
    m.add_function(wrap_pyfunction!(load_profile, m)?)?;
    m.add_function(wrap_pyfunction!(save_profile, m)?)?;
    m.add_function(wrap_pyfunction!(current_profile, m)?)?;
    m.add_function(wrap_pyfunction!(set_current_profile, m)?)?;
    m.add_function(wrap_pyfunction!(save_from_device, m)?)?;
    m.add_function(wrap_pyfunction!(switch, m)?)?;
    Ok(())
}
