//! tobii_linux.eye_refine: the yaw-dependent LBF eye refinement.

use pyo3::prelude::*;
use pyo3::types::{PyDict, PyTuple};
use tobii_headpose::FdpId;
use tobii_headpose::eye_refine::{self, Roi};

use super::lbf_predictor::model;
use super::{HeadPoseResult, fixed};
use crate::convert::bytes_like;

#[pyfunction]
fn mirrored_index(group: u32, index: u32) -> PyResult<i32> {
    eye_refine::mirrored_index(group, index).py()
}

#[pyfunction]
fn tracker_yaw_degrees(rigid_state: &[u8]) -> PyResult<f32> {
    Ok(eye_refine::tracker_yaw_degrees(&fixed(rigid_state)?))
}

#[pyfunction]
fn select_lbf_model(yaw_degrees: f32) -> (&'static str, bool) {
    let (name, mirrored) = eye_refine::select_lbf_model(yaw_degrees);
    (name.as_str(), mirrored)
}

#[pyfunction]
#[pyo3(signature = (width, height, roi=None))]
fn roi_inside(width: i64, height: i64, roi: Option<Roi>) -> bool {
    eye_refine::roi_inside(width, height, roi)
}

/// `(model name, mirrored, xs, ys)` from the records dict of normalized `(x, y)` pairs.
#[pyfunction]
fn lbf_inputs(
    point_ids: Vec<(u32, u32)>,
    records: &Bound<'_, PyDict>,
    width: i64,
    height: i64,
    yaw_degrees: f32,
) -> PyResult<(&'static str, bool, Vec<f32>, Vec<f32>)> {
    let record = |(group, index): (u32, u32)| -> Option<[f32; 2]> {
        let value = records.get_item((group, index)).ok()??;
        let (x, y): (f32, f32) = value.extract().ok()?;
        Some([x, y])
    };
    let (name, mirrored, xs, ys) = eye_refine::lbf_inputs(&point_ids, record, width, height, yaw_degrees).py()?;
    Ok((name.as_str(), mirrored, xs, ys))
}

/// `[(id, (x, y, z, present, flag, quality))]` written back for group 10.
#[pyfunction]
fn refined_records<'py>(
    py: Python<'py>,
    point_ids: Vec<(u32, u32)>,
    xs: Vec<f32>,
    ys: Vec<f32>,
    width: i64,
    height: i64,
    mirrored: bool,
) -> PyResult<Vec<(FdpId, Bound<'py, PyTuple>)>> {
    let written = eye_refine::refined_records(&point_ids, &xs, &ys, width, height, mirrored).py()?;
    written.into_iter().map(|(id, record)| Ok((id, record.into_pyobject(py)?))).collect()
}

/// The four LBF stages on a `height x width` 8-bit image, cropped to the ROI: `(xs, ys)`.
#[pyfunction]
#[pyo3(signature = (model, pixels, width, xs, ys, scale, mirrored, roi=None))]
#[allow(clippy::too_many_arguments, reason = "the Python signature")]
fn refine_region(
    py: Python<'_>,
    model: &Bound<'_, PyAny>,
    pixels: &Bound<'_, PyAny>,
    width: i64,
    xs: Vec<f32>,
    ys: Vec<f32>,
    scale: f32,
    mirrored: bool,
    roi: Option<Roi>,
) -> PyResult<(Vec<f32>, Vec<f32>)> {
    let model = self::model(model)?;
    let pixels = bytes_like(pixels)?;
    crate::exit::detached(py, || eye_refine::refine_region(&model, &pixels, width, &xs, &ys, scale, mirrored, roi)).py()
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    for function in [
        wrap_pyfunction!(mirrored_index, m)?,
        wrap_pyfunction!(tracker_yaw_degrees, m)?,
        wrap_pyfunction!(select_lbf_model, m)?,
        wrap_pyfunction!(roi_inside, m)?,
        wrap_pyfunction!(lbf_inputs, m)?,
        wrap_pyfunction!(refined_records, m)?,
        wrap_pyfunction!(refine_region, m)?,
    ] {
        m.add_function(function)?;
    }
    let table = PyDict::new(m.py());
    for (group, mirrored) in eye_refine::MIRRORED_INDEX {
        table.set_item(group, PyTuple::new(m.py(), mirrored)?)?;
    }
    m.add("MIRRORED_INDEX", table)?;
    Ok(())
}
