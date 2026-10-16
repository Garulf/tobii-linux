//! Compact JSON payloads and grayscale PNG images for the demo's browser stream.

use tobii_headpose::pipeline::HeadPoseSample;
use tobii_protocol::frames::ImageFrame;
use tobii_protocol::gaze::{COLUMNS, DeviceGaze, EyeData, GazeFrame};

use crate::pyjson::{Json, object};

pub const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
pub const MESH_PIXEL_SCALE: i64 = 10;
pub const MESH_MILLIMETRE_SCALE: i64 = 10;
pub const PNG_LEVEL: u8 = 1;

/// Name of a face tracker state, as the browser shows it.
pub fn tracker_state(state: i64) -> Option<&'static str> {
    match state {
        0 => Some("searching"),
        1 => Some("recovering"),
        2 => Some("tracking"),
        _ => None,
    }
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

fn png_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let crc = crc32(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

/// An 8-bit grayscale PNG; every row uses filter type 0.
pub fn png_grayscale(width: usize, height: usize, pixels: &[u8]) -> Result<Vec<u8>, String> {
    if pixels.len() != width * height {
        return Err("Pixel count does not match the image size".into());
    }
    let mut rows = Vec::with_capacity(height * (width + 1));
    for row in pixels.chunks(width.max(1)).take(height) {
        rows.push(0);
        rows.extend_from_slice(row);
    }
    if width == 0 {
        rows = vec![0; height];
    }
    let mut header = Vec::with_capacity(13);
    header.extend_from_slice(&(width as u32).to_be_bytes());
    header.extend_from_slice(&(height as u32).to_be_bytes());
    header.extend_from_slice(&[8, 0, 0, 0, 0]);
    let mut png = PNG_SIGNATURE.to_vec();
    png_chunk(&mut png, b"IHDR", &header);
    png_chunk(&mut png, b"IDAT", &miniz_oxide::deflate::compress_to_vec_zlib(&rows, PNG_LEVEL));
    png_chunk(&mut png, b"IEND", &[]);
    Ok(png)
}

const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let bytes = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(bytes[0]) << 16) | (u32::from(bytes[1]) << 8) | u32::from(bytes[2]);
        for (index, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            if index <= chunk.len() {
                out.push(BASE64[(n >> shift) as usize & 63] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Python's `round(number, digits)`: the correctly rounded decimal, half to even.
pub fn py_round(number: f64, digits: usize) -> f64 {
    if !number.is_finite() {
        return number;
    }
    format!("{number:.digits$}").parse().unwrap_or(number)
}

/// Values rounded to `digits`; `None` when any of them is not finite.
pub fn rounded(values: impl IntoIterator<Item = f64>, digits: usize) -> Json {
    let result: Vec<f64> = values.into_iter().map(|v| py_round(v, digits)).collect();
    if result.iter().all(|v| v.is_finite()) {
        Json::List(result.into_iter().map(Json::Float).collect())
    } else {
        Json::Null
    }
}

pub fn rounded_scalar(number: Option<f64>, digits: usize) -> Json {
    match number {
        Some(number) if number.is_finite() => Json::Float(py_round(number, digits)),
        _ => Json::Null,
    }
}

/// Little-endian int16 values times `scale`, base64 encoded; `None` when anything is not
/// finite or out of range.
pub fn int16_base64(values: Option<&[f32]>, scale: i64) -> Json {
    let Some(values) = values else { return Json::Null };
    let scaled: Vec<f64> = values.iter().map(|&v| (f64::from(v) * scale as f64).round_ties_even()).collect();
    if !scaled.iter().all(|v| v.is_finite()) || scaled.iter().any(|v| v.abs() > 32767.0) {
        return Json::Null;
    }
    let bytes: Vec<u8> = scaled.iter().flat_map(|&v| (v as i16).to_le_bytes()).collect();
    Json::Str(base64(&bytes))
}

/// Sent once: wireframe edges and the scene the 3D view draws around the head (millimetres).
pub fn mesh_topology_event(
    edges: &[(usize, usize)],
    vertex_count: usize,
    display_corners: &[[f32; 3]],
    tracker: [f32; 3],
) -> Json {
    let corners = display_corners.iter().map(|corner| rounded(corner.map(f64::from), 1)).collect();
    object([
        ("vertices", vertex_count.into()),
        ("edges", Json::List(edges.iter().flat_map(|&(a, b)| [Json::from(a), Json::from(b)]).collect())),
        ("pixel_scale", MESH_PIXEL_SCALE.into()),
        ("millimetre_scale", MESH_MILLIMETRE_SCALE.into()),
        ("display", Json::List(corners)),
        ("tracker", rounded(tracker.map(f64::from), 1)),
    ])
}

/// What the overlay shows of one tracked frame.
#[derive(Debug, Clone, PartialEq)]
pub struct Tracked {
    pub face: Option<[i64; 4]>,
    pub frame_points: Option<Vec<[f32; 2]>>,
    pub quality_score: f32,
    pub accepted: bool,
}

impl From<&tobii_headpose::TrackedFrame> for Tracked {
    fn from(frame: &tobii_headpose::TrackedFrame) -> Self {
        Self {
            face: frame.face,
            frame_points: frame.frame_points.clone(),
            quality_score: frame.quality_score,
            accepted: frame.accepted,
        }
    }
}

/// Camera image with the overlay of the same frame: face box, landmarks, mesh and tracking state.
pub fn frame_event(
    sequence: u64,
    frame: &ImageFrame,
    tracked: Option<&Tracked>,
    tracker_state_value: Option<i64>,
    mesh_pixels: Option<&[f32]>,
) -> Result<Json, String> {
    let landmarks = tracked
        .and_then(|t| t.frame_points.as_ref())
        .map_or(Json::Null, |points| rounded(points.iter().flatten().map(|&v| f64::from(v)), 4));
    let face = tracked.and_then(|t| t.face).filter(|face| face[2] > 0).map_or(Json::Null, |face| face.into());
    let image = png_grayscale(frame.width as usize, frame.height as usize, &frame.pixels)?;
    let state = match tracked {
        Some(_) => tracker_state_value.and_then(tracker_state).map_or(Json::Null, Json::from),
        None => Json::Null,
    };
    Ok(object([
        ("seq", sequence.into()),
        ("t", frame.timestamp.into()),
        ("width", frame.width.into()),
        ("height", frame.height.into()),
        ("png", Json::Str(base64(&image))),
        ("state", state),
        ("face", face),
        ("landmarks", landmarks),
        ("quality", tracked.map_or(Json::Null, |t| rounded_scalar(Some(f64::from(t.quality_score)), 3))),
        ("accepted", tracked.map_or(Json::Null, |t| t.accepted.into())),
        ("mesh", int16_base64(mesh_pixels, MESH_PIXEL_SCALE)),
    ]))
}

/// Head-pose sample in millimetres and degrees, tracker diagnostics and the posed mesh.
pub fn pose_event(
    sample: &HeadPoseSample,
    tracked: &Tracked,
    tracker_state_value: Option<i64>,
    processing_ms: f64,
    mesh_millimetres: Option<&[f32]>,
) -> Json {
    let valid = sample.valid;
    let degrees = sample.rotation_rad.map(|v| f64::from(v).to_degrees());
    object([
        ("t", sample.timestamp_us.into()),
        ("valid", valid.into()),
        ("position", if valid { rounded(sample.position_mm.map(f64::from), 1) } else { Json::Null }),
        ("rotation", if valid { rounded(degrees, 2) } else { Json::Null }),
        ("state", tracker_state_value.and_then(tracker_state).map_or(Json::Null, Json::from)),
        ("quality", rounded_scalar(Some(f64::from(tracked.quality_score)), 3)),
        ("accepted", tracked.accepted.into()),
        ("ms", Json::Float(py_round(processing_ms, 1))),
        ("mesh3d", if valid { int16_base64(mesh_millimetres, MESH_MILLIMETRE_SCALE) } else { Json::Null }),
    ])
}

/// One eye of a gaze data record, with values only where the runtime marks them valid.
pub fn eye_event(eye: &EyeData, openness_valid: u32, openness: f64) -> Json {
    let valid = eye.eye_valid != 0;
    let pupil = (eye.pupil_valid != 0 && eye.pupil_diameter > 0.0).then_some(eye.pupil_diameter);
    object([
        ("valid", valid.into()),
        ("point", if valid { rounded(eye.gaze_point_2d, 4) } else { Json::Null }),
        ("pupil", rounded_scalar(pupil, 2)),
        ("openness", if openness_valid != 0 { rounded_scalar(Some(openness), 3) } else { Json::Null }),
        ("origin", if eye.origin_valid != 0 { rounded(eye.origin, 1) } else { Json::Null }),
    ])
}

/// One gaze frame and the point of the selected gaze source, normalised to the display.
pub fn gaze_event(frame: &GazeFrame, point: Option<[f64; 2]>, source_name: &str) -> Json {
    let (combined, openness) = (&frame.gaze_point, &frame.eye_openness);
    object([
        ("t", frame.timestamp_us().into()),
        ("source", source_name.into()),
        ("point", point.map_or(Json::Null, |p| rounded(p, 4))),
        ("combined_valid", (combined.valid != 0).into()),
        ("combined", if combined.valid != 0 { rounded(combined.point, 4) } else { Json::Null }),
        ("left", eye_event(&frame.gaze_data.left, openness.left_valid, openness.left)),
        ("right", eye_event(&frame.gaze_data.right, openness.right_valid, openness.right)),
    ])
}

enum Column {
    Int(i64),
    Float(f64),
    Floats(Vec<f64>),
}

fn device_column(device: &DeviceGaze, name: &str) -> Option<Column> {
    use Column::{Float, Floats, Int};
    let g = device;
    Some(match name {
        "timestamp" => Int(g.timestamp),
        "left_origin_tracker" => Floats(g.left_origin_tracker.to_vec()),
        "left_origin_display" => Floats(g.left_origin_display.to_vec()),
        "left_position_relative_validity" => Int(g.left_position_relative_validity.into()),
        "left_position_relative" => Floats(g.left_position_relative.to_vec()),
        "left_gaze_point_3d" => Floats(g.left_gaze_point_3d.to_vec()),
        "left_gaze_point_2d" => Floats(g.left_gaze_point_2d.to_vec()),
        "left_pupil_diameter" => Float(g.left_pupil_diameter),
        "left_validity" => Int(g.left_validity.into()),
        "right_origin_tracker" => Floats(g.right_origin_tracker.to_vec()),
        "right_origin_display" => Floats(g.right_origin_display.to_vec()),
        "right_position_relative_validity" => Int(g.right_position_relative_validity.into()),
        "right_position_relative" => Floats(g.right_position_relative.to_vec()),
        "right_gaze_point_3d" => Floats(g.right_gaze_point_3d.to_vec()),
        "right_gaze_point_2d" => Floats(g.right_gaze_point_2d.to_vec()),
        "right_pupil_diameter" => Float(g.right_pupil_diameter),
        "right_validity" => Int(g.right_validity.into()),
        "combined_point" => Floats(g.combined_point.to_vec()),
        "combined_validity" => Int(g.combined_validity.into()),
        "responsive_point" => Floats(g.responsive_point.to_vec()),
        "responsive_validity" => Int(g.responsive_validity.into()),
        "left_position_guide_validity" => Int(g.left_position_guide_validity.into()),
        "left_position_guide" => Floats(g.left_position_guide.to_vec()),
        "right_position_guide_validity" => Int(g.right_position_guide_validity.into()),
        "right_position_guide" => Floats(g.right_position_guide.to_vec()),
        "left_origin_tracker_validity" => Int(g.left_origin_tracker_validity.into()),
        "left_origin_display_validity" => Int(g.left_origin_display_validity.into()),
        "right_origin_tracker_validity" => Int(g.right_origin_tracker_validity.into()),
        "right_origin_display_validity" => Int(g.right_origin_display_validity.into()),
        "left_eyeball_present" => Int(g.left_eyeball_present.into()),
        "left_eyeball_centre" => Floats(g.left_eyeball_centre.to_vec()),
        "right_eyeball_present" => Int(g.right_eyeball_present.into()),
        "right_eyeball_centre" => Floats(g.right_eyeball_centre.to_vec()),
        "left_openness" => Float(g.left_openness),
        "left_openness_validity" => Int(g.left_openness_validity.into()),
        "right_openness" => Float(g.right_openness),
        "right_openness_validity" => Int(g.right_openness_validity.into()),
        _ => return None,
    })
}

/// Every named column of the device record, by column id.
pub fn gaze_columns_event(frame: &GazeFrame) -> Json {
    let mut columns: Vec<_> = COLUMNS.iter().filter_map(|(id, name, ..)| name.map(|name| (*id, name))).collect();
    columns.sort_by_key(|(id, _)| *id);
    let columns = columns
        .into_iter()
        .filter_map(|(id, name)| {
            let value = match device_column(&frame.device, name)? {
                Column::Int(value) => Json::Int(value),
                Column::Float(value) => rounded_scalar(Some(value), 4),
                Column::Floats(values) => rounded(values, 4),
            };
            Some(Json::List(vec![id.into(), name.into(), value]))
        })
        .collect();
    object([("t", frame.timestamp_us().into()), ("columns", Json::List(columns))])
}

/// One Server-Sent Events message; `payload` must not contain newlines.
pub fn sse_block(topic: &str, payload: &str) -> Vec<u8> {
    format!("event: {topic}\ndata: {payload}\n\n").into_bytes()
}

/// Whether `value` holds a float JSON cannot carry (Python's `allow_nan=False` refuses it).
pub fn has_non_finite(value: &Json) -> bool {
    match value {
        Json::Float(number) => !number.is_finite(),
        Json::List(items) => items.iter().any(has_non_finite),
        Json::Object(entries) => entries.iter().any(|(_, value)| has_non_finite(value)),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_rounding_and_base64() {
        assert_eq!(py_round(2.675, 2), 2.67);
        assert_eq!(py_round(0.125, 2), 0.12);
        assert_eq!(py_round(0.375, 2), 0.38);
        assert_eq!(py_round(597.0, 1), 597.0);
        assert_eq!(base64(b"ab"), "YWI=");
        assert_eq!(base64(b"abc"), "YWJj");
        assert_eq!(base64(b"a"), "YQ==");
        assert_eq!(crc32(b"IEND"), 0xAE42_6082);
    }

    #[test]
    fn grayscale_png_round_trip() {
        let png = png_grayscale(3, 2, &[1, 2, 3, 4, 5, 6]).unwrap();
        assert_eq!(&png[..8], PNG_SIGNATURE);
        let idat_length = u32::from_be_bytes(png[33..37].try_into().unwrap()) as usize;
        let rows = miniz_oxide::inflate::decompress_to_vec_zlib(&png[41..41 + idat_length]).unwrap();
        assert_eq!(rows, [0, 1, 2, 3, 0, 4, 5, 6]);
        assert!(png_grayscale(3, 3, &[0; 6]).is_err());
    }

    #[test]
    fn non_finite_values_become_null() {
        assert_eq!(rounded([1.0, f64::NAN], 1), Json::Null);
        assert_eq!(rounded_scalar(Some(f64::INFINITY), 1), Json::Null);
        assert_eq!(int16_base64(Some(&[4000.0]), 10), Json::Null);
        assert_eq!(int16_base64(Some(&[1.25, -2.0]), 10), Json::Str(base64(&[12, 0, 236, 255])));
    }

    #[test]
    fn sse_block_layout() {
        assert_eq!(sse_block("status", "{}"), b"event: status\ndata: {}\n\n");
    }
}
