"""SESP codec and service: vectors from the original builders, and the service over a relay.

ORIGINAL_* vectors are outputs of the original code run in emulation by
tools/sesp_oracle.py: service messages from ses_windows.dll's builders, requests
from the game DLL's builders. LEGACY_* positions come from the game DLL's own
legacy-module transform (0x18004ac80, 0x180048d30) in the same tool.
"""
import asyncio
import struct
import unittest

from tobii_linux import sesp
from tobii_linux.head_pose_pipeline import DisplayArea, HeadPoseSample
from tobii_linux.prp_service import (FRAME_BRIDGE_HELLO, FRAME_MESSAGE, FRAME_PIPE_OPEN,
                                     FRAME_REPLY_PIPE, HeadPoseHub, PrpService, decode_names,
                                     encode_frame, encode_pipe_open, read_frame)
from tobii_linux.qpc_clock import CounterMapping, QpcSample, ServiceClock
from tobii_linux.sesp_service import LegacyPositionTransform, head_pose_message

ORIGINAL_SERVICE_MESSAGES = {
    "initialize, head pose": (
        lambda: sesp.encode_initialize_response(1, 0, {1}),
        "736573702e0000005d65737004000000e0ffffff010000000800000003000000f8ffffff0100000008000500"
        "000004000a000d0004000c000800"),
    "initialize, head pose and stream status": (
        lambda: sesp.encode_initialize_response(2, 0, {1, 3}),
        "73657370320000004165737004000000dcffffff020000000800000003000000f8ffffff010100000c000600"
        "00000400000005000a000d0004000c000800"),
    "initialize, every flag": (
        lambda: sesp.encode_initialize_response(7, 0, set(range(1, 16))),
        "73657370560000002565737004000000b8ffffff070000000800000003000000ecffffff0101010101010101"
        "01010101010101002400130000000400050006000700080009000a000b000c000d000e000f00100011001200"
        "0a000d0004000c000800"),
    "initialize with error": (
        lambda: sesp.encode_initialize_response(0x01020304, 5, {1, 15}),
        "736573704e0000003d65737004000000c0ffffff040302010800000003000000f4ffffff0500000001010000"
        "24000a0004000800000000000000000000000000000000000000000000000000000009000a000d0004000c00"
        "0800"),
    "initialize, error above 7": (
        lambda: sesp.encode_initialize_response(3, 9, set()),
        "736573702c0000005f65737004000000e2ffffff030000000800000003000000f8ffffff0100000006000800"
        "04000a000d0004000c000800"),
    "initialize, sequence 0": (
        lambda: sesp.encode_initialize_response(0, 0, {2, 9}),
        "736573703a0000004965737004000000d4ffffff0800000003000000f8ffffff010100001800060000000000"
        "040000000000000000000000000005000a000900000008000400"),
    "head-pose start": (
        lambda: sesp.encode_error_response(3, sesp.RESPONSE_HEADPOSE_START, 0),
        "73657370260000005565737004000000e8ffffff030000000800000005000000fcffffff040004000a000d00"
        "04000c000800"),
    "head-pose stop with error": (
        lambda: sesp.encode_error_response(4, sesp.RESPONSE_HEADPOSE_STOP, 6),
        "736573702c0000005f65737004000000e2ffffff040000000800000007000000f8ffffff0600000006000800"
        "04000a000d0004000c000800"),
    "statistics, largest sequence": (
        lambda: sesp.encode_error_response(0xFFFFFFFF, sesp.RESPONSE_SEND_STATISTICS, 12),
        "736573702c0000005f65737004000000e2ffffffffffffff0800000033000000f8ffffff0100000006000800"
        "04000a000d0004000c000800"),
    "invalid operation, sequence 0": (
        lambda: sesp.encode_error_response(0, sesp.SIGNAL_INVALID_OPERATION, 7),
        "73657370280000005b65737004000000e6ffffff0800000001000000f8ffffff070000000600080004000a00"
        "0900000008000400"),
    "stream status": (
        lambda: sesp.encode_stream_status_response(99),
        "73657370260000005565737004000000e8ffffff63000000080000001b000000fcffffff040004000a000d00"
        "04000c000800"),
    "head pose": (
        lambda: sesp.encode_head_pose(1, sesp.HeadPose(0x112345678, (1.5, -2.0, 600.0),
                                                       (0.1, -0.2, 0.3))),
        "73657370560000002565737004000000b8ffffff010000000800000008000000dcffffff0100000078563412"
        "0000c03f000000c000001644cdcccc3dcdcc4cbe9a99993e14002400040008000c001000140018001c002000"
        "0a000d0004000c000800"),
    "head pose, mostly zero": (
        lambda: sesp.encode_head_pose(2, sesp.HeadPose(123456789012, (0, 0, 0), (0, 0, 0))),
        "73657370320000004165737004000000dcffffff020000000800000008000000f4ffffff1c000000141a99be"
        "08000c00040008000a000d0004000c000800"),
    "head pose, invalid and negative time": (
        lambda: sesp.encode_head_pose(3, sesp.HeadPose(-5, (12.25, 160.5, 540.0), (-0.5, 0.0, 3.0),
                                                       0, (0, 1, 0))),
        "73657370660000001565737004000000a8ffffff030000000800000008000000d4fffffffffffffffbffffff"
        "000044410080204300000744000000bf000040400000000000000000000000001c002c00040008000c001000"
        "1400180000001c0020002400000028000a000d0004000c000800"),
}
ORIGINAL_INITIALIZE_REQUEST = bytes.fromhex(
    "736573704a0000003965737004000000c4ffffff020000000800000002000000dcffffff0800000003000000"
    "130000007465742d7463703a2f2f3132372e302e302e310008000c00040008000a000d0004000c000800")
ORIGINAL_INITIALIZE_REQUEST_WITHOUT_SUBSCRIPTION = bytes.fromhex(
    "73657370440000003765737004000000caffffff010000000800000002000000e0ffffff0400000013000000"
    "7465742d7463703a2f2f3132372e302e302e31000600080004000a000d0004000c000800")
ORIGINAL_HEADPOSE_START_REQUEST = bytes.fromhex(
    "73657370260000005565737004000000e8ffffff040000000800000004000000fcffffff040004000a000d00"
    "04000c000800")
ORIGINAL_STREAM_STATUS_REQUEST = bytes.fromhex(
    "736573703e0000004d65737004000000d0ffffff05000000080000001a000000e8ffffff0c00000004000000"
    "00000000010000000d00000008000c00040008000a000d0004000c000800")
ALBEDO_AREA = DisplayArea((-296.6200866699219, 326.0040588378906, 111.8187484741211),
                          (300.3799133300781, 326.0040588378906, 111.8187484741211),
                          (-296.6200866699219, 10.267330169677734, -3.100020170211792))
LEGACY_TRANSFORMED = {
    (0.0, 0.0, 600.0): (-1.879913330078125, 37.076385498046875, 509.4561767578125),
    (25.0, -40.0, 550.0): (23.120086669921875, -17.612335205078125, 476.15234375),
    (-120.5, 80.25, 720.0): (-122.37991333007812, 153.5291290283203, 594.7721557617188),
}
CLIENT_PID = 0x114


def one_message(data: bytes) -> sesp.Message:
    buffers = sesp.FrameDecoder().feed(data)
    assert len(buffers) == 1
    return sesp.decode(buffers[0])


class CodecTests(unittest.TestCase):
    def test_service_messages_match_the_original_builders(self):
        for name, (build, expected) in ORIGINAL_SERVICE_MESSAGES.items():
            with self.subTest(name):
                self.assertEqual(build().hex(), expected)

    def test_game_requests_decode(self):
        request = sesp.initialize_request(one_message(ORIGINAL_INITIALIZE_REQUEST))
        self.assertEqual((request.device_url, request.subscription_id), ("tet-tcp://127.0.0.1", 3))
        bare = one_message(ORIGINAL_INITIALIZE_REQUEST_WITHOUT_SUBSCRIPTION)
        self.assertEqual((bare.sequence, sesp.initialize_request(bare).subscription_id), (1, 0))
        start = one_message(ORIGINAL_HEADPOSE_START_REQUEST)
        self.assertEqual((start.sequence, start.kind), (4, sesp.REQUEST_HEADPOSE_START))
        status = one_message(ORIGINAL_STREAM_STATUS_REQUEST)
        self.assertEqual((status.sequence, status.kind), (5, sesp.REQUEST_NOTIFY_STREAM_STATUS))

    def test_head_pose_round_trip_keeps_defaults_and_negative_time(self):
        pose = sesp.HeadPose(-5, (12.25, 160.5, 540.0), (-0.5, 0.0, 3.0), 0, (0, 1, 0))
        self.assertEqual(sesp.head_pose(one_message(sesp.encode_head_pose(3, pose))), pose)
        valid = sesp.HeadPose(7, (0.0, 0.0, 0.0), (0.0, 0.0, 0.0))
        self.assertEqual(sesp.head_pose(one_message(sesp.encode_head_pose(1, valid))), valid)

    def test_capabilities_read_back(self):
        self.assertEqual(sesp.capabilities(one_message(sesp.encode_initialize_response(1, 0, {1, 3}))),
                         (0, {1, 3}))

    def test_decoder_reassembles_split_and_joined_messages(self):
        first = sesp.encode_stream_status_response(1)
        second = sesp.encode_head_pose(2, sesp.HeadPose(9, (1.0, 2.0, 3.0), (0.0, 0.0, 0.0)))
        decoder = sesp.FrameDecoder()
        joined = first + second
        buffers = decoder.feed(joined[:5]) + decoder.feed(joined[5:40]) + decoder.feed(joined[40:])
        self.assertEqual([sesp.decode(b).kind for b in buffers],
                         [sesp.RESPONSE_NOTIFY_STREAM_STATUS, sesp.SUBSCRIPTION_HEADPOSE])

    def test_malformed_input_is_rejected(self):
        with self.assertRaises(sesp.SespError):
            sesp.FrameDecoder().feed(b"sesq" + bytes(8))
        header = struct.pack("<III", sesp.MAGIC, 4, 5)
        with self.assertRaises(sesp.SespError):
            sesp.FrameDecoder().feed(header + bytes(4))
        with self.assertRaises(sesp.SespError):
            sesp.decode(struct.pack("<I", 200))
        with self.assertRaises(sesp.SespError):
            sesp.decode_hello(struct.pack("<I", 0x4E21) + b"client\0")
        with self.assertRaises(sesp.SespError):
            sesp.decode_hello(struct.pack("<I", sesp.COMMAND_CHANNEL) + b"client")

    def test_hello(self):
        hello = sesp.decode_hello(sesp.encode_hello(sesp.SUBSCRIPTION_CHANNEL, "client_x"))
        self.assertEqual(hello, sesp.Hello(sesp.SUBSCRIPTION_CHANNEL, "client_x"))


class LegacyPositionTests(unittest.TestCase):
    def test_model_matches_the_game_dll_transform(self):
        transform = LegacyPositionTransform.from_display_area(ALBEDO_AREA)
        for position, expected in LEGACY_TRANSFORMED.items():
            for got, want in zip(transform.apply(position), expected):
                self.assertAlmostEqual(got, want, delta=1e-3)

    def test_inverse_restores_prp_positions(self):
        transform = LegacyPositionTransform.from_display_area(ALBEDO_AREA)
        for position in LEGACY_TRANSFORMED:
            for got, want in zip(transform.apply(transform.invert(position)), position):
                self.assertAlmostEqual(got, want, delta=1e-3)

    def test_head_pose_message_carries_the_inverse(self):
        sample = HeadPoseSample(1000, True, (0.0, 0.0, 600.0), (0.1, 0.2, 0.3))
        transform = LegacyPositionTransform.from_display_area(ALBEDO_AREA)
        pose = sesp.head_pose(one_message(head_pose_message(4, sample, 77, transform)))
        self.assertEqual(pose.timestamp_us, 77)
        for got, want in zip(transform.apply(pose.position), (0.0, 0.0, 600.0)):
            self.assertAlmostEqual(got, want, delta=1e-3)
        unchanged = sesp.head_pose(one_message(head_pose_message(4, sample, 77)))
        self.assertEqual(unchanged.position, (0.0, 0.0, 600.0))
        invalid = sesp.head_pose(one_message(head_pose_message(
            5, HeadPoseSample(1, False, (0.0, 0.0, 0.0), (0.0, 0.0, 0.0)), 1)))
        self.assertEqual((invalid.position_validity, invalid.rotation_validity), (0, (0, 0, 0)))


class CounterMappingTests(unittest.TestCase):
    def clock(self, values):
        return ServiceClock("raw", read_us=lambda name: values[name])

    def test_without_reports_the_source_is_the_counter(self):
        self.assertEqual(CounterMapping(self.clock({"raw": 0}))(5_000_000), 5_000_000)

    def test_matching_clock_keeps_the_value(self):
        mapping = CounterMapping(self.clock({"raw": 0}))
        mapping.observe(QpcSample(10_000_000, {"raw": 10_000_400, "boottime": 12_000_000}))
        self.assertEqual(mapping(20_000_000), 20_000_000)

    def test_unanchored_counter_uses_the_smallest_delay(self):
        mapping = CounterMapping(self.clock({"raw": 0}))
        mapping.observe(QpcSample(1_000_000, {"raw": 501_000_300}))
        mapping.observe(QpcSample(3_000_000, {"raw": 503_000_200}))
        self.assertEqual(mapping(600_000_000), 99_999_800)


class RelayedPipe:
    """One streamengineservices connection as the bridge relays it."""

    def __init__(self, reader, writer):
        self.reader, self.writer = reader, writer
        self.decoder = sesp.FrameDecoder()
        self.reply_pipe = None
        self.pending = []

    @classmethod
    async def open(cls, port, client_pid=CLIENT_PID):
        reader, writer = await asyncio.open_connection("127.0.0.1", port)
        writer.write(encode_frame(FRAME_PIPE_OPEN, encode_pipe_open(client_pid, sesp.PIPE_NAME)))
        return cls(reader, writer)

    async def raw(self, timeout=2.0):
        while True:
            kind, payload = await asyncio.wait_for(read_frame(self.reader), timeout)
            if kind == FRAME_REPLY_PIPE:
                self.reply_pipe = decode_names(payload)[0]
                continue
            assert kind == FRAME_MESSAGE
            return payload

    async def hello(self, channel, reply_pipe):
        self.writer.write(encode_frame(FRAME_MESSAGE, sesp.encode_hello(channel, reply_pipe)))
        slot = await self.raw()
        assert self.reply_pipe == reply_pipe and len(slot) == 4
        return struct.unpack("<I", slot)[0]

    def send(self, data: bytes, chunk: int = 4096):
        for start in range(0, len(data), chunk):
            self.writer.write(encode_frame(FRAME_MESSAGE, data[start:start + chunk]))

    async def message(self, timeout=2.0) -> sesp.Message:
        while not self.pending:
            self.pending += self.decoder.feed(await self.raw(timeout))
        return sesp.decode(self.pending.pop(0))

    def close(self):
        self.writer.close()


def game_request(sequence: int, kind: int, payload=(), strings=()) -> bytes:
    return sesp.message(sequence, kind, payload, strings)


class SespServiceTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        self.hub = HeadPoseHub()
        self.clock = ServiceClock("raw", read_us=lambda name: 0)
        self.service = PrpService(self.hub, clock=self.clock, display_area=ALBEDO_AREA,
                                  tet_tcp=True, tet_tcp_port=0)
        self.service.attach_tracker()
        self.server = await self.service.serve("127.0.0.1", 0)
        self.port = self.server.sockets[0].getsockname()[1]
        self.pipes = []

    async def asyncTearDown(self):
        for pipe in self.pipes:
            pipe.close()
        await self.service.close(self.server)

    async def pipe(self, **kwargs):
        pipe = await RelayedPipe.open(self.port, **kwargs)
        self.pipes.append(pipe)
        return pipe

    async def connect(self):
        command = await self.pipe()
        await command.hello(sesp.COMMAND_CHANNEL, "client_streamengineservices_00000114_1")
        subscription = await self.pipe()
        slot = await subscription.hello(sesp.SUBSCRIPTION_CHANNEL,
                                        "client_streamengineservices_00000114_2")
        command.send(game_request(1, sesp.REQUEST_INITIALIZE,
                                  [sesp.Scalar(1, "I", slot)], [(0, b"tet-tcp://127.0.0.1")]))
        return command, subscription, await command.message()

    async def test_bridge_serves_only_the_sesp_pipe(self):
        reader, writer = await asyncio.open_connection("127.0.0.1", self.port)
        writer.write(encode_frame(FRAME_BRIDGE_HELLO, struct.pack("<I", 1)))
        _, payload = await read_frame(reader)
        writer.close()
        self.assertEqual(decode_names(payload), [sesp.PIPE_NAME])

    async def test_initialize_advertises_head_pose(self):
        _, _, response = await self.connect()
        self.assertEqual(response.sequence, 1)
        self.assertEqual(sesp.capabilities(response),
                         (0, {sesp.CAPABILITY_HEAD_POSE, sesp.CAPABILITY_STREAM_STATUS}))

    async def test_initialize_with_unknown_subscription_fails(self):
        command = await self.pipe()
        await command.hello(sesp.COMMAND_CHANNEL, "client_a")
        command.send(game_request(1, sesp.REQUEST_INITIALIZE, [sesp.Scalar(1, "I", 999)],
                                  [(0, b"tet-tcp://127.0.0.1")]))
        error, flags = sesp.capabilities(await command.message())
        self.assertEqual((error, flags), (sesp.ERROR_NOT_FOUND, set()))

    async def test_head_pose_flows_on_the_subscription_channel_until_stopped(self):
        command, subscription, _ = await self.connect()
        command.send(ORIGINAL_HEADPOSE_START_REQUEST)
        started = await command.message()
        self.assertEqual((started.sequence, started.kind), (4, sesp.RESPONSE_HEADPOSE_START))
        await asyncio.sleep(0.05)
        self.hub.publish(HeadPoseSample(1_000_000, True, (0.0, 0.0, 600.0), (0.1, -0.2, 0.0)))
        self.hub.publish(HeadPoseSample(1_030_000, False, (0.0, 0.0, 0.0), (0.0, 0.0, 0.0)))
        first, second = await subscription.message(), await subscription.message()
        self.assertEqual([first.sequence, second.sequence], [1, 2])
        pose = sesp.head_pose(first)
        self.assertEqual(pose.timestamp_us, 1_000_000)
        transform = LegacyPositionTransform.from_display_area(ALBEDO_AREA)
        for got, want in zip(transform.apply(pose.position), (0.0, 0.0, 600.0)):
            self.assertAlmostEqual(got, want, delta=1e-3)
        self.assertEqual(sesp.head_pose(second).position_validity, sesp.INVALID)
        command.send(game_request(5, sesp.REQUEST_HEADPOSE_STOP))
        stopped = await command.message()
        self.assertEqual((stopped.sequence, stopped.kind), (5, sesp.RESPONSE_HEADPOSE_STOP))
        await asyncio.sleep(0.05)
        self.hub.publish(HeadPoseSample(1_060_000, True, (0.0, 0.0, 600.0), (0.0, 0.0, 0.0)))
        with self.assertRaises(asyncio.TimeoutError):
            await subscription.message(timeout=0.2)

    async def test_other_requests(self):
        command, _, _ = await self.connect()
        command.send(ORIGINAL_STREAM_STATUS_REQUEST)
        self.assertEqual((await command.message()).kind, sesp.RESPONSE_NOTIFY_STREAM_STATUS)
        command.send(game_request(6, sesp.REQUEST_SEND_STATISTICS))
        statistics = await command.message()
        self.assertEqual((statistics.sequence, statistics.kind), (6, sesp.RESPONSE_SEND_STATISTICS))
        command.send(game_request(7, sesp.REQUEST_GET_DISPLAY_INFO))
        refused = await command.message()
        self.assertEqual((refused.sequence, refused.kind), (7, sesp.SIGNAL_INVALID_OPERATION))
        self.assertEqual(refused.payload.scalar(0, "I"), sesp.ERROR_NOT_SUPPORTED)

    async def test_detaching_the_tracker_closes_sesp_connections(self):
        command, _, _ = await self.connect()
        self.service.detach_tracker()
        with self.assertRaises((asyncio.IncompleteReadError, ConnectionError)):
            await command.raw(timeout=1.0)


if __name__ == "__main__":
    unittest.main()
