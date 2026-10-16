//! tobii_linux.gaze: device gaze (0x500) and presence (0x504) decoding and the platmod records.
//!
//! Records cross the boundary as tuples in dataclass field order; the Python module wraps
//! them in its dataclasses.

use pyo3::IntoPyObjectExt;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict, PyTuple};
use tobii_protocol::gaze::{
    self, DeviceGaze, EyeData, GazeDataRecord, GazeFrame, OpennessRecord, PairRecord, Parsed, PointRecord, build,
};

use crate::convert::{bytes_like, floats};
use crate::errors::IntoPyResult;
use crate::ttp::{PyField, fields_arg, message_arg};

pub const DEVICE_GAZE_FIELDS: [&str; 37] = [
    "timestamp",
    "left_origin_tracker",
    "left_origin_display",
    "left_position_relative_validity",
    "left_position_relative",
    "left_gaze_point_3d",
    "left_gaze_point_2d",
    "left_pupil_diameter",
    "left_validity",
    "right_origin_tracker",
    "right_origin_display",
    "right_position_relative_validity",
    "right_position_relative",
    "right_gaze_point_3d",
    "right_gaze_point_2d",
    "right_pupil_diameter",
    "right_validity",
    "combined_point",
    "combined_validity",
    "responsive_point",
    "responsive_validity",
    "left_position_guide_validity",
    "left_position_guide",
    "right_position_guide_validity",
    "right_position_guide",
    "left_origin_tracker_validity",
    "left_origin_display_validity",
    "right_origin_tracker_validity",
    "right_origin_display_validity",
    "left_eyeball_present",
    "left_eyeball_centre",
    "right_eyeball_present",
    "right_eyeball_centre",
    "left_openness",
    "left_openness_validity",
    "right_openness",
    "right_openness_validity",
];

fn vector<'py, const N: usize>(py: Python<'py>, values: [f64; N]) -> PyResult<Bound<'py, PyAny>> {
    Ok(PyTuple::new(py, values)?.into_any())
}

fn scalar<'py>(py: Python<'py>, value: impl IntoPyObject<'py>) -> PyResult<Bound<'py, PyAny>> {
    value.into_bound_py_any(py)
}

pub fn device_tuple<'py>(py: Python<'py>, g: &DeviceGaze) -> PyResult<Bound<'py, PyTuple>> {
    let items = [
        scalar(py, g.timestamp)?,
        vector(py, g.left_origin_tracker)?,
        vector(py, g.left_origin_display)?,
        scalar(py, g.left_position_relative_validity)?,
        vector(py, g.left_position_relative)?,
        vector(py, g.left_gaze_point_3d)?,
        vector(py, g.left_gaze_point_2d)?,
        scalar(py, g.left_pupil_diameter)?,
        scalar(py, g.left_validity)?,
        vector(py, g.right_origin_tracker)?,
        vector(py, g.right_origin_display)?,
        scalar(py, g.right_position_relative_validity)?,
        vector(py, g.right_position_relative)?,
        vector(py, g.right_gaze_point_3d)?,
        vector(py, g.right_gaze_point_2d)?,
        scalar(py, g.right_pupil_diameter)?,
        scalar(py, g.right_validity)?,
        vector(py, g.combined_point)?,
        scalar(py, g.combined_validity)?,
        vector(py, g.responsive_point)?,
        scalar(py, g.responsive_validity)?,
        scalar(py, g.left_position_guide_validity)?,
        vector(py, g.left_position_guide)?,
        scalar(py, g.right_position_guide_validity)?,
        vector(py, g.right_position_guide)?,
        scalar(py, g.left_origin_tracker_validity)?,
        scalar(py, g.left_origin_display_validity)?,
        scalar(py, g.right_origin_tracker_validity)?,
        scalar(py, g.right_origin_display_validity)?,
        scalar(py, g.left_eyeball_present)?,
        vector(py, g.left_eyeball_centre)?,
        scalar(py, g.right_eyeball_present)?,
        vector(py, g.right_eyeball_centre)?,
        scalar(py, g.left_openness)?,
        scalar(py, g.left_openness_validity)?,
        scalar(py, g.right_openness)?,
        scalar(py, g.right_openness_validity)?,
    ];
    PyTuple::new(py, items)
}

/// A DeviceGaze from any object with its attributes (the Python dataclass).
pub fn device_arg(object: &Bound<'_, PyAny>) -> PyResult<DeviceGaze> {
    let get = |name: &str| object.getattr(name);
    let v2 = |name: &str| floats::<2>(&get(name)?);
    let v3 = |name: &str| floats::<3>(&get(name)?);
    let u = |name: &str| get(name)?.extract::<u32>();
    let f = |name: &str| get(name)?.extract::<f64>();
    Ok(DeviceGaze {
        timestamp: get("timestamp")?.extract()?,
        left_origin_tracker: v3("left_origin_tracker")?,
        left_origin_display: v3("left_origin_display")?,
        left_position_relative_validity: u("left_position_relative_validity")?,
        left_position_relative: v3("left_position_relative")?,
        left_gaze_point_3d: v3("left_gaze_point_3d")?,
        left_gaze_point_2d: v2("left_gaze_point_2d")?,
        left_pupil_diameter: f("left_pupil_diameter")?,
        left_validity: u("left_validity")?,
        right_origin_tracker: v3("right_origin_tracker")?,
        right_origin_display: v3("right_origin_display")?,
        right_position_relative_validity: u("right_position_relative_validity")?,
        right_position_relative: v3("right_position_relative")?,
        right_gaze_point_3d: v3("right_gaze_point_3d")?,
        right_gaze_point_2d: v2("right_gaze_point_2d")?,
        right_pupil_diameter: f("right_pupil_diameter")?,
        right_validity: u("right_validity")?,
        combined_point: v2("combined_point")?,
        combined_validity: u("combined_validity")?,
        responsive_point: v2("responsive_point")?,
        responsive_validity: u("responsive_validity")?,
        left_position_guide_validity: u("left_position_guide_validity")?,
        left_position_guide: v3("left_position_guide")?,
        right_position_guide_validity: u("right_position_guide_validity")?,
        right_position_guide: v3("right_position_guide")?,
        left_origin_tracker_validity: u("left_origin_tracker_validity")?,
        left_origin_display_validity: u("left_origin_display_validity")?,
        right_origin_tracker_validity: u("right_origin_tracker_validity")?,
        right_origin_display_validity: u("right_origin_display_validity")?,
        left_eyeball_present: u("left_eyeball_present")?,
        left_eyeball_centre: v3("left_eyeball_centre")?,
        right_eyeball_present: u("right_eyeball_present")?,
        right_eyeball_centre: v3("right_eyeball_centre")?,
        left_openness: f("left_openness")?,
        left_openness_validity: u("left_openness_validity")?,
        right_openness: f("right_openness")?,
        right_openness_validity: u("right_openness_validity")?,
    })
}

type V2 = (f64, f64);
type V3 = (f64, f64, f64);
type PointTuple = (i64, u32, V2);
type PairTuple = (i64, u32, V3, u32, V3);
type OpennessTuple = (i64, u32, f64, u32, f64);
type EyeTuple = (u32, V3, u32, V3, u32, V3, V2, u32, V3, u32, f64);
type GazeDataTuple = (i64, EyeTuple, EyeTuple);

fn v2([x, y]: [f64; 2]) -> V2 {
    (x, y)
}

fn v3([x, y, z]: [f64; 3]) -> V3 {
    (x, y, z)
}

fn point_tuple(r: &PointRecord) -> PointTuple {
    (r.timestamp, r.valid, v2(r.point))
}

fn pair_tuple(r: &PairRecord) -> PairTuple {
    (r.timestamp, r.left_valid, v3(r.left), r.right_valid, v3(r.right))
}

fn openness_tuple(r: &OpennessRecord) -> OpennessTuple {
    (r.timestamp, r.left_valid, r.left, r.right_valid, r.right)
}

fn eye_tuple(e: &EyeData) -> EyeTuple {
    (
        e.origin_valid,
        v3(e.origin),
        e.position_valid,
        v3(e.position),
        e.eye_valid,
        v3(e.gaze_point_3d),
        v2(e.gaze_point_2d),
        e.eyeball_valid,
        v3(e.eyeball_centre),
        e.pupil_valid,
        e.pupil_diameter,
    )
}

fn gaze_data_tuple(r: &GazeDataRecord) -> GazeDataTuple {
    (r.timestamp, eye_tuple(&r.left), eye_tuple(&r.right))
}

/// The six records of one frame in GazeFrame field order, after the device tuple.
pub fn frame_tuple<'py>(py: Python<'py>, frame: &GazeFrame) -> PyResult<Bound<'py, PyTuple>> {
    (
        device_tuple(py, &frame.device)?,
        point_tuple(&frame.gaze_point),
        point_tuple(&frame.responsive_gaze_point),
        pair_tuple(&frame.gaze_origin),
        gaze_data_tuple(&frame.gaze_data),
        openness_tuple(&frame.eye_openness),
        pair_tuple(&frame.user_position_guide),
    )
        .into_pyobject(py)
}

#[pyfunction]
fn decode_gaze_message<'py>(py: Python<'py>, payload: &Bound<'py, PyAny>) -> PyResult<Option<Bound<'py, PyTuple>>> {
    gaze::decode_gaze_message(&bytes_like(payload)?).py()?.map(|g| device_tuple(py, &g)).transpose()
}

#[pyfunction]
fn gaze_message<'py>(py: Python<'py>, message: &Bound<'py, PyAny>) -> PyResult<Option<Bound<'py, PyTuple>>> {
    gaze::gaze_message(&message_arg(message)?).py()?.map(|g| device_tuple(py, &g)).transpose()
}

/// Device tuple and records of one device gaze value.
#[pyfunction]
fn gaze_frame<'py>(py: Python<'py>, device: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyTuple>> {
    frame_tuple(py, &GazeFrame::new(device_arg(device)?))
}

#[pyfunction]
fn platmod_records<'py>(py: Python<'py>, device: &Bound<'py, PyAny>, host_gaze: bool) -> PyResult<Bound<'py, PyDict>> {
    let records = PyDict::new(py);
    for (name, data) in gaze::platmod_records(&device_arg(device)?, host_gaze) {
        records.set_item(name, PyBytes::new(py, &data))?;
    }
    Ok(records)
}

fn unknown(name: &str) -> PyErr {
    PyValueError::new_err(format!("Unknown platmod record '{name}'"))
}

fn pack_record(name: &str, record: &Bound<'_, PyAny>) -> PyResult<Vec<u8>> {
    let get = |attr: &str| record.getattr(attr);
    let pair = || -> PyResult<PairRecord> {
        Ok(PairRecord {
            timestamp: get("timestamp")?.extract()?,
            left_valid: get("left_valid")?.extract()?,
            left: floats::<3>(&get("left")?)?,
            right_valid: get("right_valid")?.extract()?,
            right: floats::<3>(&get("right")?)?,
        })
    };
    let eye = |eye: &Bound<'_, PyAny>| -> PyResult<EyeData> {
        let get = |attr: &str| eye.getattr(attr);
        Ok(EyeData {
            origin_valid: get("origin_valid")?.extract()?,
            origin: floats::<3>(&get("origin")?)?,
            position_valid: get("position_valid")?.extract()?,
            position: floats::<3>(&get("position")?)?,
            eye_valid: get("eye_valid")?.extract()?,
            gaze_point_3d: floats::<3>(&get("gaze_point_3d")?)?,
            gaze_point_2d: floats::<2>(&get("gaze_point_2d")?)?,
            eyeball_valid: get("eyeball_valid")?.extract()?,
            eyeball_centre: floats::<3>(&get("eyeball_centre")?)?,
            pupil_valid: get("pupil_valid")?.extract()?,
            pupil_diameter: get("pupil_diameter")?.extract()?,
        })
    };
    Ok(match name {
        "point" => PointRecord {
            timestamp: get("timestamp")?.extract()?,
            valid: get("valid")?.extract()?,
            point: floats::<2>(&get("point")?)?,
        }
        .pack(),
        "pair" => pair()?.pack(),
        "openness" => OpennessRecord {
            timestamp: get("timestamp")?.extract()?,
            left_valid: get("left_valid")?.extract()?,
            left: get("left")?.extract()?,
            right_valid: get("right_valid")?.extract()?,
            right: get("right")?.extract()?,
        }
        .pack(),
        "eye" => eye(record)?.pack(),
        "gaze_data" => GazeDataRecord {
            timestamp: get("timestamp")?.extract()?,
            left: eye(&get("left")?)?,
            right: eye(&get("right")?)?,
        }
        .pack(),
        other => return Err(unknown(other)),
    })
}

/// Little-endian platmod layout of a record object; `layout` is point, pair, openness, eye or gaze_data.
#[pyfunction]
fn pack<'py>(py: Python<'py>, layout: &str, record: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyBytes>> {
    Ok(PyBytes::new(py, &pack_record(layout, record)?))
}

/// Record tuple of packed platmod bytes, by record name.
#[pyfunction]
fn platmod_from_bytes<'py>(py: Python<'py>, name: &str, data: &[u8]) -> PyResult<Bound<'py, PyAny>> {
    Ok(match name {
        "gaze_point" | "responsive_gaze_point" => {
            point_tuple(&PointRecord::unpack(data).py()?).into_pyobject(py)?.into_any()
        }
        "eye_openness" => openness_tuple(&OpennessRecord::unpack(data).py()?).into_pyobject(py)?.into_any(),
        "gaze_origin" | "user_position_guide" => {
            pair_tuple(&PairRecord::unpack(data).py()?).into_pyobject(py)?.into_any()
        }
        "gaze_data" => gaze_data_tuple(&GazeDataRecord::unpack(data).py()?).into_pyobject(py)?.into_any(),
        "eye" => eye_tuple(&EyeData::unpack(data).py()?).into_pyobject(py)?.into_any(),
        other => return Err(unknown(other)),
    })
}

/// `(parsed type, value)` of one column value as the runtime's column reader types it.
#[pyfunction]
fn parsed_value<'py>(py: Python<'py>, items: &Bound<'py, PyAny>) -> PyResult<(Option<u32>, Bound<'py, PyAny>)> {
    let parsed = gaze::parsed_value(&fields_arg(items)?).py()?;
    let Some(parsed) = parsed else {
        return Ok((None, py.None().into_bound(py)));
    };
    let value = match parsed {
        Parsed::UInt32(n) => scalar(py, n)?,
        Parsed::Int32(n) => scalar(py, n)?,
        Parsed::Int64(n) => scalar(py, n)?,
        Parsed::Float(x) => scalar(py, x)?,
        Parsed::Vec2(v) => vector(py, v)?,
        Parsed::Vec3(v) => vector(py, v)?,
    };
    Ok((Some(parsed.kind() as u32), value))
}

#[pyfunction]
fn presence_message(payload: &Bound<'_, PyAny>) -> PyResult<Option<(i64, i32)>> {
    Ok(gaze::presence_message(&bytes_like(payload)?).py()?.map(|p| (p.timestamp_us, p.value)))
}

#[pyfunction]
fn platmod_presence(device_value: i64) -> i64 {
    gaze::platmod_presence(device_value)
}

#[pyfunction]
fn prp_presence(platmod_value: i64) -> u32 {
    gaze::prp_presence(platmod_value)
}

fn column(fields: Vec<tobii_protocol::Field>) -> Vec<PyField> {
    fields.iter().map(PyField::from_core).collect()
}

#[pyfunction]
fn uint32_column(value: u32) -> Vec<PyField> {
    column(build::uint32_column(value))
}

#[pyfunction]
fn int32_column(value: i32) -> Vec<PyField> {
    column(build::int32_column(value))
}

#[pyfunction]
fn int64_column(value: i64) -> Vec<PyField> {
    column(build::int64_column(value))
}

#[pyfunction]
fn fixed16_column(value: f64) -> PyResult<Vec<PyField>> {
    Ok(column(build::fixed16_column(value).py()?))
}

#[pyfunction]
fn vec2_column(values: &Bound<'_, PyAny>) -> PyResult<Vec<PyField>> {
    Ok(column(build::vec2_column(floats::<2>(values)?).py()?))
}

#[pyfunction]
fn vec3_column(values: &Bound<'_, PyAny>) -> PyResult<Vec<PyField>> {
    Ok(column(build::vec3_column(floats::<3>(values)?).py()?))
}

/// Column set payload from `(column id, value fields)` pairs, in order.
#[pyfunction]
fn columns_payload<'py>(py: Python<'py>, columns: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyBytes>> {
    let columns: Vec<(u32, Vec<tobii_protocol::Field>)> = columns
        .try_iter()?
        .map(|entry| {
            let entry = entry?;
            Ok((entry.get_item(0)?.extract()?, fields_arg(&entry.get_item(1)?)?))
        })
        .collect::<PyResult<_>>()?;
    Ok(PyBytes::new(py, &build::columns_payload(&columns).py()?))
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    for function in [
        wrap_pyfunction!(decode_gaze_message, m)?,
        wrap_pyfunction!(gaze_message, m)?,
        wrap_pyfunction!(gaze_frame, m)?,
        wrap_pyfunction!(platmod_records, m)?,
        wrap_pyfunction!(pack, m)?,
        wrap_pyfunction!(platmod_from_bytes, m)?,
        wrap_pyfunction!(parsed_value, m)?,
        wrap_pyfunction!(presence_message, m)?,
        wrap_pyfunction!(platmod_presence, m)?,
        wrap_pyfunction!(prp_presence, m)?,
        wrap_pyfunction!(uint32_column, m)?,
        wrap_pyfunction!(int32_column, m)?,
        wrap_pyfunction!(int64_column, m)?,
        wrap_pyfunction!(fixed16_column, m)?,
        wrap_pyfunction!(vec2_column, m)?,
        wrap_pyfunction!(vec3_column, m)?,
        wrap_pyfunction!(columns_payload, m)?,
    ] {
        m.add_function(function)?;
    }
    m.add("DEVICE_GAZE_FIELDS", PyTuple::new(m.py(), DEVICE_GAZE_FIELDS)?)?;
    let columns = PyDict::new(m.py());
    for (id, name, kind, present) in gaze::COLUMNS {
        columns.set_item(id, (name, kind as u32, present))?;
    }
    m.add("COLUMNS", columns)?;
    m.add("VEC2_STRUCT", gaze::VEC2_STRUCT)?;
    m.add("VEC3_STRUCT", gaze::VEC3_STRUCT)?;
    Ok(())
}
