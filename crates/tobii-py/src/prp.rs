//! tobii_linux.prp: PRP envelopes, stream bodies and packets, and the PRP stream records of
//! tobii_linux.gaze.
//!
//! Also holds the conversions every game-service binding shares: the Python errors of
//! `tobii_service::ServiceError` and integer arguments refused with the PRP messages.

use pyo3::create_exception;
use pyo3::exceptions::{PyKeyError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyType};
use tobii_protocol::gaze::{GazeDataRecord, OpennessRecord, PairRecord, PointRecord};
use tobii_service::ServiceError;
use tobii_service::prp::{self, GazePoint, HeadPose, StreamSample};
use tobii_service::prp_streams::{self, PlatmodRecord, PrpRecord};

use crate::convert::{bytes_like, int};
use crate::errors::IntoPyResult;

create_exception!(tobii_linux.prp, ProtocolError, PyValueError);

pub fn service_error(error: ServiceError) -> PyErr {
    match error {
        ServiceError::Prp(message) => ProtocolError::new_err(message),
        ServiceError::Sesp(message) => crate::sesp::SespError::new_err(message),
        ServiceError::Invalid(message) => PyValueError::new_err(message),
        ServiceError::Truncated(message) => Python::attach(|py| {
            let class = py.import("struct").and_then(|module| module.getattr("error"));
            match class.and_then(|class| class.cast_into::<PyType>().map_err(PyErr::from)) {
                Ok(class) => PyErr::from_type(class, message),
                Err(error) => error,
            }
        }),
        ServiceError::Protocol(error) => crate::errors::protocol(error),
    }
}

impl<T> IntoPyResult<T> for Result<T, ServiceError> {
    fn py(self) -> PyResult<T> {
        self.map_err(service_error)
    }
}

pub fn prp_error(message: &str) -> PyErr {
    ProtocolError::new_err(message.to_string())
}

/// An integer in range of `T`, refused with `message` as a PRP ProtocolError.
pub fn ranged<T: TryFrom<i128>>(object: &Bound<'_, PyAny>, message: &str) -> PyResult<T> {
    int(object).and_then(|n| T::try_from(n).ok()).ok_or_else(|| prp_error(message))
}

/// `_u32` of the Python codecs.
pub fn u32_arg(object: &Bound<'_, PyAny>) -> PyResult<u32> {
    ranged(object, "value is outside unsigned 32-bit range")
}

/// `_i32` of the Python codecs.
pub fn i32_arg(object: &Bound<'_, PyAny>) -> PyResult<i32> {
    ranged(object, "value is outside signed 32-bit range")
}

/// The low 32 bits of any integer, as `value & 0xFFFFFFFF` gives them.
pub fn masked_u32(object: &Bound<'_, PyAny>) -> PyResult<u32> {
    object.call_method1("__and__", (0xFFFF_FFFFu32,))?.extract()
}

/// The low 64 bits of any integer, as a signed value.
pub fn masked_i64(object: &Bound<'_, PyAny>) -> PyResult<i64> {
    Ok(object.call_method1("__and__", (u64::MAX,))?.extract::<u64>()? as i64)
}

fn timestamp_arg(object: &Bound<'_, PyAny>) -> PyResult<i64> {
    ranged(object, "timestamp is outside signed 64-bit range")
}

fn message_tag(object: &Bound<'_, PyAny>) -> PyResult<u32> {
    ranged(object, "message tag is outside unsigned 32-bit range")
}

fn gaze_arg(sample: &Bound<'_, PyAny>) -> PyResult<GazePoint> {
    Ok(GazePoint {
        timestamp: timestamp_arg(&sample.getattr("timestamp")?)?,
        valid: sample.getattr("valid")?.is_truthy()?,
        x: sample.getattr("x")?.extract()?,
        y: sample.getattr("y")?.extract()?,
    })
}

fn three<'py>(sample: &Bound<'py, PyAny>, name: &str) -> PyResult<Vec<Bound<'py, PyAny>>> {
    let values: Vec<Bound<'py, PyAny>> = sample.getattr(name)?.try_iter()?.collect::<PyResult<_>>()?;
    if values.len() != 3 {
        return Err(prp_error("head pose requires three components per vector"));
    }
    Ok(values)
}

fn head_arg(sample: &Bound<'_, PyAny>) -> PyResult<HeadPose> {
    let (position, rotation_valid, rotation) =
        (three(sample, "position")?, three(sample, "rotation_valid")?, three(sample, "rotation")?);
    let float = |values: &[Bound<'_, PyAny>]| -> PyResult<[f64; 3]> {
        Ok([values[0].extract()?, values[1].extract()?, values[2].extract()?])
    };
    Ok(HeadPose {
        timestamp: timestamp_arg(&sample.getattr("timestamp")?)?,
        position_valid: sample.getattr("position_valid")?.is_truthy()?,
        position: float(&position)?,
        rotation_valid: [
            rotation_valid[0].is_truthy()?,
            rotation_valid[1].is_truthy()?,
            rotation_valid[2].is_truthy()?,
        ],
        rotation: float(&rotation)?,
    })
}

type GazeTuple = (i64, bool, f64, f64);
type HeadTuple = (i64, bool, (f64, f64, f64), (bool, bool, bool), (f64, f64, f64));

fn head_tuple(head: &HeadPose) -> HeadTuple {
    let [px, py, pz] = head.position;
    let [vx, vy, vz] = head.rotation_valid;
    let [rx, ry, rz] = head.rotation;
    (head.timestamp, head.position_valid, (px, py, pz), (vx, vy, vz), (rx, ry, rz))
}

#[pyfunction]
#[pyo3(signature = (payload, max_payload=prp::DEFAULT_MAX_PAYLOAD))]
fn encode_envelope<'py>(
    py: Python<'py>,
    payload: &Bound<'py, PyAny>,
    max_payload: u64,
) -> PyResult<Bound<'py, PyBytes>> {
    Ok(PyBytes::new(py, &prp::encode_envelope(&bytes_like(payload)?, max_payload).py()?))
}

#[pyfunction]
#[pyo3(signature = (packet, max_payload=prp::DEFAULT_MAX_PAYLOAD))]
fn decode_envelope<'py>(
    py: Python<'py>,
    packet: &Bound<'py, PyAny>,
    max_payload: u64,
) -> PyResult<Bound<'py, PyBytes>> {
    Ok(PyBytes::new(py, prp::decode_envelope(&bytes_like(packet)?, max_payload).py()?))
}

#[pyfunction]
fn encode_gaze_body<'py>(py: Python<'py>, sample: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyBytes>> {
    Ok(PyBytes::new(py, &prp::encode_gaze_body(&gaze_arg(sample)?)))
}

#[pyfunction]
fn decode_gaze_body(body: &Bound<'_, PyAny>) -> PyResult<GazeTuple> {
    let gaze = prp::decode_gaze_body(&bytes_like(body)?).py()?;
    Ok((gaze.timestamp, gaze.valid, gaze.x, gaze.y))
}

#[pyfunction]
fn encode_head_body<'py>(py: Python<'py>, sample: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyBytes>> {
    Ok(PyBytes::new(py, &prp::encode_head_body(&head_arg(sample)?)))
}

#[pyfunction]
fn decode_head_body(body: &Bound<'_, PyAny>) -> PyResult<HeadTuple> {
    Ok(head_tuple(&prp::decode_head_body(&bytes_like(body)?).py()?))
}

/// `kind` is "gaze" or "head" for the Python sample class, anything else is unsupported.
#[pyfunction]
fn encode_stream_packet<'py>(
    py: Python<'py>,
    kind: Option<&str>,
    sample: &Bound<'py, PyAny>,
    message_tag: &Bound<'py, PyAny>,
    responsive: bool,
) -> PyResult<Bound<'py, PyBytes>> {
    let tag = self::message_tag(message_tag)?;
    let sample = match kind {
        Some("gaze") => StreamSample::Gaze(gaze_arg(sample)?),
        Some("head") => StreamSample::Head(head_arg(sample)?),
        _ => return prp::unsupported_sample().py(),
    };
    Ok(PyBytes::new(py, &prp::encode_stream_packet(&sample, tag, responsive).py()?))
}

fn platmod(name: &str, packed: &[u8]) -> PyResult<PlatmodRecord> {
    Ok(match name {
        "gaze_point" => PlatmodRecord::GazePoint(PointRecord::unpack(packed).py()?),
        "responsive_gaze_point" => PlatmodRecord::ResponsiveGazePoint(PointRecord::unpack(packed).py()?),
        "gaze_origin" => PlatmodRecord::GazeOrigin(PairRecord::unpack(packed).py()?),
        "gaze_data" => PlatmodRecord::GazeData(Box::new(GazeDataRecord::unpack(packed).py()?)),
        "eye_openness" => PlatmodRecord::EyeOpenness(OpennessRecord::unpack(packed).py()?),
        "user_position_guide" => PlatmodRecord::UserPositionGuide(PairRecord::unpack(packed).py()?),
        other => return Err(PyKeyError::new_err(other.to_string())),
    })
}

/// `(operation, stream, native data)` of the service callback for one packed platmod record.
#[pyfunction]
fn prp_record<'py>(
    py: Python<'py>,
    name: &str,
    packed: &Bound<'py, PyAny>,
) -> PyResult<(u32, u32, Bound<'py, PyBytes>)> {
    let record = prp_streams::prp_record(&platmod(name, &bytes_like(packed)?)?);
    Ok((record.kind, record.stream, PyBytes::new(py, &record.native)))
}

fn record(kind: u32, stream: u32, native: &Bound<'_, PyAny>) -> PyResult<PrpRecord> {
    Ok(PrpRecord { kind, stream, native: bytes_like(native)? })
}

#[pyfunction]
#[pyo3(signature = (kind, stream, native, extended=true))]
fn stream_body<'py>(
    py: Python<'py>,
    kind: u32,
    stream: u32,
    native: &Bound<'py, PyAny>,
    extended: bool,
) -> PyResult<Option<Bound<'py, PyBytes>>> {
    let body = prp_streams::stream_body(&record(kind, stream, native)?, extended).py()?;
    Ok(body.map(|body| PyBytes::new(py, &body)))
}

/// The package for one record; the tag is checked only where the serializer has a case.
#[pyfunction]
#[pyo3(signature = (kind, stream, native, message_tag=None, extended=true))]
fn stream_packet<'py>(
    py: Python<'py>,
    kind: u32,
    stream: u32,
    native: &Bound<'py, PyAny>,
    message_tag: Option<&Bound<'py, PyAny>>,
    extended: bool,
) -> PyResult<Option<Bound<'py, PyBytes>>> {
    let record = record(kind, stream, native)?;
    let Some(body) = prp_streams::stream_body(&record, extended).py()? else {
        return Ok(None);
    };
    let tag = message_tag.map(self::message_tag).transpose()?.unwrap_or(0);
    Ok(Some(PyBytes::new(py, &prp::stream_package(record.kind, tag, record.stream, &body))))
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("ProtocolError", m.py().get_type::<ProtocolError>())?;
    for function in [
        wrap_pyfunction!(encode_envelope, m)?,
        wrap_pyfunction!(decode_envelope, m)?,
        wrap_pyfunction!(encode_gaze_body, m)?,
        wrap_pyfunction!(decode_gaze_body, m)?,
        wrap_pyfunction!(encode_head_body, m)?,
        wrap_pyfunction!(decode_head_body, m)?,
        wrap_pyfunction!(encode_stream_packet, m)?,
        wrap_pyfunction!(prp_record, m)?,
        wrap_pyfunction!(stream_body, m)?,
        wrap_pyfunction!(stream_packet, m)?,
    ] {
        m.add_function(function)?;
    }
    m.add("MAGIC", prp::MAGIC)?;
    m.add("HEAD_POSE", prp::HEAD_POSE)?;
    m.add("GAZE_POINT", prp::GAZE_POINT)?;
    m.add("RESPONSIVE_GAZE_POINT", prp::RESPONSIVE_GAZE_POINT)?;
    m.add("DEFAULT_MAX_PAYLOAD", prp::DEFAULT_MAX_PAYLOAD)?;
    m.add("OPERATION_STREAM", prp_streams::OPERATION_STREAM)?;
    m.add("OPERATION_COMPOUND_STREAM", prp_streams::OPERATION_COMPOUND_STREAM)?;
    m.add("PRP_GAZE_POINT", prp_streams::PRP_GAZE_POINT)?;
    m.add("PRP_GAZE_ORIGIN", prp_streams::PRP_GAZE_ORIGIN)?;
    m.add("PRP_GAZE_DATA", prp_streams::PRP_GAZE_DATA)?;
    m.add("PRP_RESPONSIVE_GAZE_POINT", prp_streams::PRP_RESPONSIVE_GAZE_POINT)?;
    m.add("PRP_EYE_OPENNESS", prp_streams::PRP_EYE_OPENNESS)?;
    m.add("PRP_COMPOUND_USER_POSITION_GUIDE", prp_streams::PRP_COMPOUND_USER_POSITION_GUIDE)?;
    Ok(())
}
