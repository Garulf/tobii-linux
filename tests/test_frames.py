import struct
import unittest

from tobii_linux import frames
from tobii_linux import ttp_commands as tc
from tobii_linux.ttp import Field, Message, ProtocolError

# Synthetic messages built from the statically recovered layout, NOT device traffic.


def int64_field(value):
    return Field(tc.FIELD_INT64, struct.pack('>q', value))


def column(column_id, *value):
    return [tc.struct_header(2, frames.COLUMN_STRUCT), tc.uint32_field(column_id), *value]


def stream_message(stream, columns, word4=0, trailing=()):
    items = [tc.struct_header(len(columns), frames.FRAME_STRUCT)]
    for entry in columns:
        items += entry
    payload = b'\0\0' + b''.join(item.encode() for item in [*items, *trailing])
    return Message(tc.STREAM_DATA_KIND, 0, 0, tc.wire_stream_id(stream), word4, payload)


def image_columns(pixel_format=8, width=4, height=3, pixels=None):
    pixels = bytes(range(width * height)) if pixels is None else pixels
    return [column(1, int64_field(123456789)), column(2, tc.uint32_field(pixel_format)),
            column(3, tc.uint32_field(width)), column(4, tc.uint32_field(height)),
            column(5, tc.blob_field(pixels))]


class FrameTests(unittest.TestCase):
    def test_image_frame(self):
        frame = frames.image_frame(stream_message(tc.Stream.IMAGE, image_columns()))
        self.assertEqual(frame, frames.ImageFrame(123456789, 8, 4, 3, bytes(range(12))))

    def test_column_order_and_unknown_nested_column(self):
        nested = column(9, tc.struct_header(2, 0x1234), tc.uint32_field(1),
                        tc.struct_header(1, 0x99), tc.int32_field(-5))
        columns = image_columns()[::-1] + [nested]
        frame = frames.image_frame(stream_message(tc.Stream.IMAGE, columns))
        self.assertEqual((frame.width, frame.height), (4, 3))
        parsed = frames.stream_columns(stream_message(tc.Stream.IMAGE, columns).payload)
        self.assertEqual(parsed[-1].column_id, 9)
        self.assertEqual(len(parsed[-1].value), 4)

    def test_head_pose_checks(self):
        with self.assertRaises(ProtocolError):
            frames.image_frame(stream_message(tc.Stream.IMAGE, image_columns(pixel_format=7)))
        with self.assertRaises(ProtocolError):
            frames.image_frame(stream_message(tc.Stream.IMAGE, image_columns(pixels=b'\0' * 11)))

    def test_rejects_malformed_messages(self):
        good = image_columns()
        cases = [
            stream_message(tc.Stream.PRIMARY_CAMERA_IMAGE, good),
            stream_message(tc.Stream.IMAGE, good, word4=1),
            stream_message(tc.Stream.IMAGE, good[:4]),
            stream_message(tc.Stream.IMAGE, good + [good[0]]),
            stream_message(tc.Stream.IMAGE, good, trailing=(tc.uint32_field(0),)),
            stream_message(tc.Stream.IMAGE, [column(1, tc.uint32_field(1))] + good[1:]),
            Message(tc.REQUEST_KIND, 0, 0, 0x501, 0, stream_message(tc.Stream.IMAGE, good).payload),
        ]
        for message in cases:
            with self.subTest(message=message.encode()[:40].hex()), self.assertRaises(ProtocolError):
                frames.image_frame(message)

    def test_truncated_column_set(self):
        message = stream_message(tc.Stream.IMAGE, image_columns())
        items = message.payload
        header = tc.struct_header(6, frames.FRAME_STRUCT).encode()
        payload = b'\0\0' + header + items[2 + len(header):]
        with self.assertRaises(ProtocolError):
            frames.stream_columns(payload)

    def test_blob_inner_size_must_match(self):
        bad = Field(tc.FIELD_BLOB, struct.pack('>I', 3) + b'\0\0')
        with self.assertRaises(ProtocolError):
            frames.blob_value(bad)

    def test_primary_camera_frame(self):
        columns = [column(1, int64_field(-1)), column(2, tc.uint32_field(8)),
                   column(3, tc.uint32_field(2)), column(4, tc.uint32_field(2)),
                   column(5, tc.uint32_field(3)), column(6, tc.blob_field(b'abcdef'))]
        frame = frames.primary_camera_frame(stream_message(tc.Stream.PRIMARY_CAMERA_IMAGE, columns))
        self.assertEqual(frame, frames.PrimaryCameraFrame(-1, 8, 2, 2, 3, b'abcdef'))
        columns[3] = column(4, tc.uint32_field(3))
        columns[5] = column(6, tc.blob_field(b'abcdefghi'))
        with self.assertRaises(ProtocolError):
            frames.primary_camera_frame(stream_message(tc.Stream.PRIMARY_CAMERA_IMAGE, columns))


if __name__ == '__main__':
    unittest.main()
