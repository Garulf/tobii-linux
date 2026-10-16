//! tobii_linux.sesp_service: the legacy-module position transform and SESP head-pose
//! messages. The asyncio server that pairs SESP channels stays in Python.

use pyo3::prelude::*;
use pyo3::types::PyBytes;
use tobii_service::sesp_service::{self as service, HeadPoseSample, LegacyPositionTransform};

use crate::convert::floats;
use crate::prp::masked_i64;

type Triple = (f64, f64, f64);
type TransformTuple = (f64, f64, Triple);

fn triple([a, b, c]: [f64; 3]) -> Triple {
    (a, b, c)
}

/// A LegacyPositionTransform dataclass: `cos`, `sin` and `translation`.
fn transform_arg(transform: &Bound<'_, PyAny>) -> PyResult<LegacyPositionTransform> {
    Ok(LegacyPositionTransform {
        cos: transform.getattr("cos")?.extract()?,
        sin: transform.getattr("sin")?.extract()?,
        translation: floats::<3>(&transform.getattr("translation")?)?,
    })
}

/// `(cos, sin, translation)` the legacy module derives from a display area's corners.
#[pyfunction]
fn transform_from_display_area(area: &Bound<'_, PyAny>) -> PyResult<TransformTuple> {
    let corner = |name: &str| floats::<3>(&area.getattr(name)?);
    let transform =
        LegacyPositionTransform::from_display_area(corner("top_left")?, corner("top_right")?, corner("bottom_left")?);
    Ok((transform.cos, transform.sin, triple(transform.translation)))
}

#[pyfunction]
fn apply(transform: &Bound<'_, PyAny>, position: &Bound<'_, PyAny>) -> PyResult<Triple> {
    Ok(triple(transform_arg(transform)?.apply(floats::<3>(position)?)))
}

#[pyfunction]
fn invert(transform: &Bound<'_, PyAny>, position: &Bound<'_, PyAny>) -> PyResult<Triple> {
    Ok(triple(transform_arg(transform)?.invert(floats::<3>(position)?)))
}

/// SESP type 8 for a HeadPoseSample (`valid`, `position_mm`, `rotation_rad`).
#[pyfunction]
#[pyo3(signature = (sequence, sample, timestamp_us, transform=None))]
fn head_pose_message<'py>(
    py: Python<'py>,
    sequence: u32,
    sample: &Bound<'py, PyAny>,
    timestamp_us: &Bound<'py, PyAny>,
    transform: Option<&Bound<'py, PyAny>>,
) -> PyResult<Bound<'py, PyBytes>> {
    let sample = HeadPoseSample {
        valid: sample.getattr("valid")?.is_truthy()?,
        position_mm: floats::<3>(&sample.getattr("position_mm")?)?,
        rotation_rad: floats::<3>(&sample.getattr("rotation_rad")?)?,
    };
    let transform = transform.filter(|transform| !transform.is_none()).map(transform_arg).transpose()?;
    let message = service::head_pose_message(sequence, &sample, masked_i64(timestamp_us)?, transform.as_ref());
    Ok(PyBytes::new(py, &message))
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    for function in [
        wrap_pyfunction!(transform_from_display_area, m)?,
        wrap_pyfunction!(apply, m)?,
        wrap_pyfunction!(invert, m)?,
        wrap_pyfunction!(head_pose_message, m)?,
    ] {
        m.add_function(function)?;
    }
    m.add("MAX_PENDING_BYTES", service::MAX_PENDING_BYTES)?;
    m.add("CAPABILITIES", service::CAPABILITIES.to_vec())?;
    Ok(())
}
