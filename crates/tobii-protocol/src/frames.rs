//! Stream data messages (kind 0x53) and the camera image frames they carry.
//!
//! Layout recovered from the runtime's stream decoder (0x1402ce2a0, column readers
//! 0x1403b1b90 and 0x1403b1b40) and dispatcher (0x1402c2700); see docs/usb-streams.md.
//! Parsing is stricter than the runtime: duplicate or malformed columns are rejected
//! instead of being overwritten or skipped.

use crate::commands::{STREAM_DATA_KIND, Stream};
use crate::error::{Result, fail};
use crate::fields::wire;
use crate::ttp::{Field, Message, be_u32, fields};

pub const FRAME_STRUCT: u32 = 0x0BB8;
pub const COLUMN_STRUCT: u32 = 0x0BB9;
pub const COLUMN_MEMBERS: u32 = 2;
pub const HEAD_POSE_PIXEL_FORMAT: u32 = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    pub column_id: u32,
    pub value: Vec<Field>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageFrame {
    pub timestamp: i64,
    pub pixel_format: u32,
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrimaryCameraFrame {
    pub timestamp: i64,
    pub word2: u32,
    pub width: u32,
    pub stride: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

/// `(member count, structure id)` of a structure header item.
pub fn struct_code(field: &Field) -> Result<(u32, u32)> {
    if field.tag != wire::STRUCT || field.data.len() != 4 {
        return fail("Expected a struct header field");
    }
    let code = be_u32(&field.data, 0);
    Ok(((code >> 16) & 0xFFF, code & 0xFFFF))
}

/// Index after one column value, counting the members of nested structures.
pub fn take_value(items: &[Field], mut index: usize) -> Result<usize> {
    let mut outstanding: u64 = 1;
    while outstanding > 0 {
        let Some(item) = items.get(index) else {
            return fail("Column value missing");
        };
        outstanding -= 1;
        if item.tag == wire::STRUCT {
            outstanding += u64::from(struct_code(item)?.0);
        }
        index += 1;
    }
    Ok(index)
}

pub fn stream_columns(payload: &[u8]) -> Result<Vec<Column>> {
    let items = fields(payload)?;
    let Some(first) = items.first() else {
        return fail("Empty stream payload");
    };
    let (count, kind) = struct_code(first)?;
    if kind != FRAME_STRUCT {
        return fail("Stream payload does not start with a column set");
    }
    let mut columns = Vec::with_capacity(count as usize);
    let mut index = 1;
    for _ in 0..count {
        if index + 1 >= items.len() {
            return fail("Truncated column set");
        }
        if struct_code(&items[index])? != (COLUMN_MEMBERS, COLUMN_STRUCT) {
            return fail("Expected a column header");
        }
        let column_id = uint32_value(&items[index + 1])?;
        let end = take_value(&items, index + 2)?;
        columns.push(Column { column_id, value: items[index + 2..end].to_vec() });
        index = end;
    }
    if index != items.len() {
        return fail("Unexpected fields after the column set");
    }
    Ok(columns)
}

pub fn uint32_value(field: &Field) -> Result<u32> {
    if field.tag != wire::UINT32 || field.data.len() != 4 {
        return fail("Expected a uint32 field");
    }
    Ok(be_u32(&field.data, 0))
}

pub fn int64_value(field: &Field) -> Result<i64> {
    if field.tag != wire::INT64 || field.data.len() != 8 {
        return fail("Expected an int64 field");
    }
    Ok(i64::from_be_bytes(field.data[..8].try_into().expect("eight bytes")))
}

pub fn blob_value(field: &Field) -> Result<&[u8]> {
    if field.tag != wire::BLOB || field.data.len() < 4 {
        return fail("Expected a blob field");
    }
    if be_u32(&field.data, 0) as usize != field.data.len() - 4 {
        return fail("Blob size does not match field length");
    }
    Ok(&field.data[4..])
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ColumnValue {
    Int64(i64),
    UInt32(u32),
    Blob(Vec<u8>),
}

impl ColumnValue {
    fn decode(tag: u8, field: &Field) -> Result<Self> {
        Ok(match tag {
            wire::INT64 => ColumnValue::Int64(int64_value(field)?),
            wire::UINT32 => ColumnValue::UInt32(uint32_value(field)?),
            _ => ColumnValue::Blob(blob_value(field)?.to_vec()),
        })
    }
}

/// Values of exactly the `layout` columns (id and wire type) of one stream data message.
pub fn column_values(message: &Message, stream: Stream, layout: &[(u32, u8)]) -> Result<Vec<ColumnValue>> {
    if message.kind != STREAM_DATA_KIND || message.command != stream.wire_id() {
        return fail("Not a data message for this stream");
    }
    if message.word4 != 0 {
        return fail("Stream message carries a nonzero status word");
    }
    let mut values: Vec<Option<ColumnValue>> = vec![None; layout.len()];
    for column in stream_columns(&message.payload)? {
        let Some(slot) = layout.iter().position(|(id, _)| *id == column.column_id) else {
            continue;
        };
        if values[slot].is_some() {
            return fail("Duplicate column");
        }
        let tag = layout[slot].1;
        if column.value.len() != 1 || column.value[0].tag != tag {
            return fail("Unexpected column type");
        }
        values[slot] = Some(ColumnValue::decode(tag, &column.value[0])?);
    }
    values.into_iter().map(|value| value.map_or_else(|| fail("Required image column missing"), Ok)).collect()
}

pub const IMAGE_LAYOUT: [(u32, u8); 5] =
    [(1, wire::INT64), (2, wire::UINT32), (3, wire::UINT32), (4, wire::UINT32), (5, wire::BLOB)];
pub const PRIMARY_CAMERA_LAYOUT: [(u32, u8); 6] =
    [(1, wire::INT64), (2, wire::UINT32), (3, wire::UINT32), (4, wire::UINT32), (5, wire::UINT32), (6, wire::BLOB)];

fn int64(value: &ColumnValue) -> i64 {
    match value {
        ColumnValue::Int64(n) => *n,
        _ => unreachable!("layout guarantees an int64"),
    }
}

fn uint32(value: &ColumnValue) -> u32 {
    match value {
        ColumnValue::UInt32(n) => *n,
        _ => unreachable!("layout guarantees a uint32"),
    }
}

fn blob(value: ColumnValue) -> Vec<u8> {
    match value {
        ColumnValue::Blob(bytes) => bytes,
        _ => unreachable!("layout guarantees a blob"),
    }
}

/// Image stream 0x501, with the checks receive_image (0x140266430) applies.
pub fn image_frame(message: &Message) -> Result<ImageFrame> {
    let mut values = column_values(message, Stream::Image, &IMAGE_LAYOUT)?;
    let pixels = blob(values.pop().expect("five columns"));
    let frame = ImageFrame {
        timestamp: int64(&values[0]),
        pixel_format: uint32(&values[1]),
        width: uint32(&values[2]),
        height: uint32(&values[3]),
        pixels,
    };
    if frame.pixel_format != HEAD_POSE_PIXEL_FORMAT {
        return fail("receive_image only accepts pixel format 8");
    }
    if frame.pixels.len() as u64 != u64::from(frame.width) * u64::from(frame.height) {
        return fail("Pixel byte count is not width times height");
    }
    Ok(frame)
}

/// Primary camera stream 0x50e, with the checks of 0x140266d50 and 0x140285be0.
pub fn primary_camera_frame(message: &Message) -> Result<PrimaryCameraFrame> {
    let mut values = column_values(message, Stream::PrimaryCameraImage, &PRIMARY_CAMERA_LAYOUT)?;
    let pixels = blob(values.pop().expect("six columns"));
    let frame = PrimaryCameraFrame {
        timestamp: int64(&values[0]),
        word2: uint32(&values[1]),
        width: uint32(&values[2]),
        stride: uint32(&values[3]),
        height: uint32(&values[4]),
        pixels,
    };
    if frame.pixels.len() as u64 != u64::from(frame.stride) * u64::from(frame.height) {
        return fail("Pixel byte count is not stride times height");
    }
    if frame.stride != frame.width {
        return fail("The head-pose path requires stride equal to width");
    }
    Ok(frame)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::commands::{blob_field, struct_header};
    use crate::fields::{int32_field, int64_field, uint32_field};
    use crate::ttp::REQUEST_KIND as REQUEST_KIND_FOR_TESTS;
    use crate::ttp::encode_payload;

    pub fn column(column_id: u32, value: Vec<Field>) -> Vec<Field> {
        [vec![struct_header(2, COLUMN_STRUCT).unwrap(), uint32_field(column_id)], value].concat()
    }

    pub fn stream_message(stream: Stream, columns: &[Vec<Field>], word4: u32, trailing: &[Field]) -> Message {
        let mut items = vec![struct_header(columns.len() as u32, FRAME_STRUCT).unwrap()];
        for entry in columns {
            items.extend(entry.iter().cloned());
        }
        items.extend(trailing.iter().cloned());
        Message::new(STREAM_DATA_KIND, 0, 0, stream.wire_id(), word4, encode_payload(&items).unwrap())
    }

    pub fn image_columns(pixel_format: u32, width: u32, height: u32, pixels: Option<Vec<u8>>) -> Vec<Vec<Field>> {
        let pixels = pixels.unwrap_or_else(|| (0..width * height).map(|n| n as u8).collect());
        vec![
            column(1, vec![int64_field(123_456_789)]),
            column(2, vec![uint32_field(pixel_format)]),
            column(3, vec![uint32_field(width)]),
            column(4, vec![uint32_field(height)]),
            column(5, vec![blob_field(&pixels)]),
        ]
    }

    pub fn good_image() -> Vec<Vec<Field>> {
        image_columns(8, 4, 3, None)
    }

    #[test]
    fn image_frame_decodes() {
        let frame = image_frame(&stream_message(Stream::Image, &good_image(), 0, &[])).unwrap();
        assert_eq!(
            frame,
            ImageFrame { timestamp: 123_456_789, pixel_format: 8, width: 4, height: 3, pixels: (0..12).collect() }
        );
    }

    #[test]
    fn column_order_and_unknown_nested_column() {
        let nested = column(
            9,
            vec![struct_header(2, 0x1234).unwrap(), uint32_field(1), struct_header(1, 0x99).unwrap(), int32_field(-5)],
        );
        let mut columns: Vec<_> = good_image().into_iter().rev().collect();
        columns.push(nested);
        let message = stream_message(Stream::Image, &columns, 0, &[]);
        let frame = image_frame(&message).unwrap();
        assert_eq!((frame.width, frame.height), (4, 3));
        let parsed = stream_columns(&message.payload).unwrap();
        assert_eq!(parsed.last().unwrap().column_id, 9);
        assert_eq!(parsed.last().unwrap().value.len(), 4);
    }

    #[test]
    fn head_pose_checks_and_malformed_messages() {
        assert!(image_frame(&stream_message(Stream::Image, &image_columns(7, 4, 3, None), 0, &[])).is_err());
        assert!(
            image_frame(&stream_message(Stream::Image, &image_columns(8, 4, 3, Some(vec![0; 11])), 0, &[])).is_err()
        );
        let good = good_image();
        let duplicate: Vec<_> = good.iter().cloned().chain([good[0].clone()]).collect();
        let mistyped: Vec<_> =
            [column(1, vec![uint32_field(1)])].into_iter().chain(good[1..].iter().cloned()).collect();
        let mut wrong_kind = stream_message(Stream::Image, &good, 0, &[]);
        wrong_kind.kind = REQUEST_KIND_FOR_TESTS;
        let cases = [
            stream_message(Stream::PrimaryCameraImage, &good, 0, &[]),
            stream_message(Stream::Image, &good, 1, &[]),
            stream_message(Stream::Image, &good[..4], 0, &[]),
            stream_message(Stream::Image, &duplicate, 0, &[]),
            stream_message(Stream::Image, &good, 0, &[uint32_field(0)]),
            stream_message(Stream::Image, &mistyped, 0, &[]),
            wrong_kind,
        ];
        for message in cases {
            assert!(image_frame(&message).is_err(), "{message:?}");
        }
    }

    #[test]
    fn truncated_column_set_and_blob_size() {
        let message = stream_message(Stream::Image, &good_image(), 0, &[]);
        let header = struct_header(6, FRAME_STRUCT).unwrap().encode().unwrap();
        let payload = [&b"\0\0"[..], &header, &message.payload[2 + header.len()..]].concat();
        assert!(stream_columns(&payload).is_err());
        let bad = Field::new(wire::BLOB, [&3u32.to_be_bytes()[..], b"\0\0"].concat());
        assert!(blob_value(&bad).is_err());
    }

    #[test]
    fn primary_camera_frame_decodes() {
        let mut columns = vec![
            column(1, vec![int64_field(-1)]),
            column(2, vec![uint32_field(8)]),
            column(3, vec![uint32_field(2)]),
            column(4, vec![uint32_field(2)]),
            column(5, vec![uint32_field(3)]),
            column(6, vec![blob_field(b"abcdef")]),
        ];
        let frame = primary_camera_frame(&stream_message(Stream::PrimaryCameraImage, &columns, 0, &[])).unwrap();
        assert_eq!(
            frame,
            PrimaryCameraFrame { timestamp: -1, word2: 8, width: 2, stride: 2, height: 3, pixels: b"abcdef".to_vec() }
        );
        columns[3] = column(4, vec![uint32_field(3)]);
        columns[5] = column(6, vec![blob_field(b"abcdefghi")]);
        assert!(primary_camera_frame(&stream_message(Stream::PrimaryCameraImage, &columns, 0, &[])).is_err());
    }
}
