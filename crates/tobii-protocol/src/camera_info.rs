//! Camera hardware-info response (command 0x848) recovered from the runtime.
//!
//! See docs/ttp-replies.md. Field names give the byte offset at which the runtime stores
//! each value; no meaning has been established for them. docs/head-pose-output.md lists
//! which offsets the head-pose host code consumes.

use crate::error::{Result, fail};
use crate::fields::{
    KIND_RESPONSE, Reader, array_fields, fixed16_field, fixed42_array_field, fixed42_field, int32_field,
    response_status, status_name, struct_field, triple_fixed42_fields,
};
use crate::numeric::round_f32;
use crate::ttp::{Field, HARDWARE_INFO_COMMAND, Message, encode_payload};

pub const CAMERA_HEADER: u32 = 0x1A332C;
pub const CAMERA_RECORD_SIZE: usize = 0x2E8;
pub const CAMERA_SERIES_LIMIT: usize = 64;
pub const MAX_CAMERAS: usize = 4;
pub const MAX_POINTS: usize = 40;
pub const RESPONSE_FIELD_TYPES: [u8; 3] = [0x1B, 0x0C, 0x02];

pub type Triple = [f64; 3];

/// One camera record. Fixed-point values are kept as doubles; `float32_*` fields hold
/// values the runtime stores in float32 (fixed16 items, exact in float32).
#[derive(Debug, Clone, PartialEq)]
pub struct CameraRecord {
    pub int32_0x00: i32,
    pub float32_0x04: f64,
    pub float32_0x08: f64,
    pub triple_0x10: Triple,
    pub doubles_0x28: Vec<f64>,
    pub int32_0xa0: i32,
    pub int32_0xa4: i32,
    pub int32_0xa8: i32,
    pub series_0xb0: Vec<f64>,
    pub triple_0x2b0: Triple,
    pub triple_0x2c8: Triple,
    pub double_0x2e0: f64,
}

impl CameraRecord {
    pub fn series_count_0xac(&self) -> usize {
        self.series_0xb0.len()
    }
}

/// Response contents; `points_0xbb0` are narrowed to float32 by the response parser.
#[derive(Debug, Clone, PartialEq)]
pub struct HardwareInfo {
    pub cameras_0x8: Vec<CameraRecord>,
    pub points_0xbb0: Vec<Triple>,
    pub int32_0xf70: i32,
}

/// Wire order of 0x1403b11b0; the item order is also the record order.
pub fn read_camera(reader: &mut Reader) -> Result<CameraRecord> {
    reader.structure(CAMERA_HEADER)?;
    let int32_0x00 = reader.int32()?;
    let float32_0x04 = reader.fixed16()?;
    let float32_0x08 = reader.fixed16()?;
    let triple_0x10 = reader.triple_fixed42()?;
    let doubles_0x28 = (0..15).map(|_| reader.fixed42()).collect::<Result<_>>()?;
    let int32_0xa0 = reader.int32()?;
    let int32_0xa4 = reader.int32()?;
    let int32_0xa8 = reader.int32()?;
    let series_0xb0 = reader.fixed42_array(CAMERA_SERIES_LIMIT)?;
    Ok(CameraRecord {
        int32_0x00,
        float32_0x04,
        float32_0x08,
        triple_0x10,
        doubles_0x28,
        int32_0xa0,
        int32_0xa4,
        int32_0xa8,
        series_0xb0,
        triple_0x2b0: reader.triple_fixed42()?,
        triple_0x2c8: reader.triple_fixed42()?,
        double_0x2e0: reader.fixed42()?,
    })
}

pub fn read_points(reader: &mut Reader) -> Result<Vec<Triple>> {
    (0..reader.array()?).map(|_| Ok(reader.triple_fixed42()?.map(round_f32))).collect()
}

/// Response-parser stage only; the tracker's count limits are applied later.
pub fn read_hardware_info(payload: &[u8]) -> Result<HardwareInfo> {
    let mut reader = Reader::new(payload)?;
    let cameras_0x8 = (0..reader.array()?).map(|_| read_camera(&mut reader)).collect::<Result<_>>()?;
    let points_0xbb0 = read_points(&mut reader)?;
    Ok(HardwareInfo { cameras_0x8, points_0xbb0, int32_0xf70: reader.int32()? })
}

pub fn parse_hardware_info(message: &Message) -> Result<HardwareInfo> {
    if message.kind != KIND_RESPONSE || message.command != HARDWARE_INFO_COMMAND {
        return fail("Not a hardware-info response");
    }
    let status = response_status(message);
    if status != 0 {
        return fail(format!("Hardware-info response status {}", status_name(i64::from(status))));
    }
    let info = read_hardware_info(&message.payload)?;
    if info.cameras_0x8.len() > MAX_CAMERAS || info.points_0xbb0.len() > MAX_POINTS {
        return fail("Hardware-info response exceeds the tracker's record limits");
    }
    Ok(info)
}

/// Inverse of [`read_camera`], for synthetic vectors; no device encoder was examined.
pub fn camera_fields(camera: &CameraRecord) -> Result<Vec<Field>> {
    let mut items = vec![
        struct_field(CAMERA_HEADER >> 16, CAMERA_HEADER & 0xFFFF)?,
        int32_field(camera.int32_0x00),
        fixed16_field(camera.float32_0x04)?,
        fixed16_field(camera.float32_0x08)?,
    ];
    items.extend(triple_fixed42_fields(camera.triple_0x10)?);
    for &number in &camera.doubles_0x28 {
        items.push(fixed42_field(number)?);
    }
    items.extend([camera.int32_0xa0, camera.int32_0xa4, camera.int32_0xa8].map(int32_field));
    items.push(fixed42_array_field(&camera.series_0xb0)?);
    items.extend(triple_fixed42_fields(camera.triple_0x2b0)?);
    items.extend(triple_fixed42_fields(camera.triple_0x2c8)?);
    items.push(fixed42_field(camera.double_0x2e0)?);
    Ok(items)
}

pub fn hardware_info_response(info: &HardwareInfo, request_id: u32, descriptors: (u32, u32)) -> Result<Message> {
    let mut items = array_fields(info.cameras_0x8.len() as u32, descriptors.0)?.to_vec();
    for camera in &info.cameras_0x8 {
        items.extend(camera_fields(camera)?);
    }
    items.extend(array_fields(info.points_0xbb0.len() as u32, descriptors.1)?);
    for point in &info.points_0xbb0 {
        items.extend(triple_fixed42_fields(*point)?);
    }
    items.push(int32_field(info.int32_0xf70));
    Ok(Message::new(KIND_RESPONSE, request_id, 0, HARDWARE_INFO_COMMAND, 0, encode_payload(&items)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::hex;

    fn payload() -> Vec<u8> {
        let doubles: String = (0..15u64).map(|n| format!("0400000008{:016x}", n << 42)).collect();
        let camera = format!(
            "0500000004001a332c 0100000004fffffffe 030000000400018000 0300000004ffffc000
             050000000400031f41 04000000080000040000000000 0400000008fffff80000000000 04000000080000020000000000
             {doubles}
             010000000400000280 0100000004000001e0 010000000400000001 190000000c000000010000008000000000
             050000000400031f41 04000000080000000000000000 04000000080000000000000000 04000000080000040000000000
             050000000400031f41 0400000008fffffc0000000000 04000000080000000000000000 04000000080000000000000000
             04000000080000080000000000"
        );
        hex(&format!(
            "0000 050000000400020100 020000000400000000 {camera}
             050000000400020100 020000000400000000
             050000000400031f41 040000000800000c0000000000 0400000008fffff00000000000 04000000080000030000000000
             010000000400000005"
        ))
    }

    pub fn camera() -> CameraRecord {
        CameraRecord {
            int32_0x00: -2,
            float32_0x04: 1.5,
            float32_0x08: -0.25,
            triple_0x10: [1.0, -2.0, 0.5],
            doubles_0x28: (0..15).map(f64::from).collect(),
            int32_0xa0: 640,
            int32_0xa4: 480,
            int32_0xa8: 1,
            series_0xb0: vec![0.125],
            triple_0x2b0: [0.0, 0.0, 1.0],
            triple_0x2c8: [-1.0, 0.0, 0.0],
            double_0x2e0: 2.0,
        }
    }

    fn info() -> HardwareInfo {
        HardwareInfo { cameras_0x8: vec![camera()], points_0xbb0: vec![[3.0, -4.0, 0.75]], int32_0xf70: 5 }
    }

    fn response(payload: Vec<u8>, word4: u32, command: u32, kind: u32) -> Message {
        Message::new(kind, 7, 0, command, word4, payload)
    }

    #[test]
    fn reconstructed_vector_round_trips() {
        let message = response(payload(), 0, 0x848, 0x52);
        assert_eq!(parse_hardware_info(&message).unwrap(), info());
        assert_eq!(hardware_info_response(&info(), 7, (0, 0)).unwrap(), message);
    }

    #[test]
    fn series_beyond_sixty_four_is_dropped_and_points_are_float32() {
        let mut long = camera();
        long.series_0xb0 = (0..70).map(f64::from).collect();
        let info = HardwareInfo { cameras_0x8: vec![long], points_0xbb0: vec![], int32_0xf70: 0 };
        let parsed = parse_hardware_info(&hardware_info_response(&info, 1, (0, 0)).unwrap()).unwrap();
        assert_eq!(parsed.cameras_0x8[0].series_count_0xac(), 64);
        let info = HardwareInfo { cameras_0x8: vec![], points_0xbb0: vec![[0.1, 0.2, 0.3]], int32_0xf70: 0 };
        let parsed = parse_hardware_info(&hardware_info_response(&info, 1, (0, 0)).unwrap()).unwrap();
        assert_eq!(parsed.points_0xbb0[0], [0.10000000149011612, 0.20000000298023224, 0.30000001192092896]);
    }

    #[test]
    fn limits_status_and_malformed() {
        let five = HardwareInfo { cameras_0x8: vec![camera(); 5], points_0xbb0: vec![], int32_0xf70: 0 };
        let message = hardware_info_response(&five, 1, (0, 0)).unwrap();
        assert_eq!(read_hardware_info(&message.payload).unwrap().cameras_0x8.len(), 5);
        assert!(parse_hardware_info(&message).is_err());
        for (word4, command, kind) in [(0x2000_0503, 0x848, 0x52), (0, 0x847, 0x52), (0, 0x848, 0x51)] {
            assert!(parse_hardware_info(&response(payload(), word4, command, kind)).is_err());
        }
        let good = payload();
        let wrong_header = String::from_utf8(good.iter().flat_map(|b| format!("{b:02x}").into_bytes()).collect())
            .unwrap()
            .replace("001a332c", "0019332c");
        for bad in [
            good[..good.len() - 1].to_vec(),
            good[..40].to_vec(),
            hex(&wrong_header),
            [&b"\0\x01"[..], &good[2..]].concat(),
        ] {
            assert!(parse_hardware_info(&response(bad, 0, 0x848, 0x52)).is_err());
        }
    }
}
