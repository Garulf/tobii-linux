"""PRP service tests: a simulated bridge drives the service with the client-side codecs.

The checks mirror what the game DLL requires (docs/service-transport.md) and what
the real service sent on albedo (docs/prp-bridge.md). No pipes or Wine are involved.
REAL_* vectors were captured live from the real ET5 service with
tools/prp_real_service_probe.py; nobody was in front of the tracker, so the pose is invalid.
"""
import asyncio
import struct
import unittest

from tobii_linux import gaze
from tobii_linux.head_pose_pipeline import HeadPoseSample
from tobii_linux.prp import HEAD_POSE, decode_envelope, decode_gaze_body, decode_head_body, encode_envelope
from tobii_linux.prp_service import (
    FRAME_BRIDGE_HELLO, FRAME_DISPLAY_ID, FRAME_MESSAGE, FRAME_PIPE_OPEN, FRAME_PIPES, FRAME_REPLY_PIPE,
    GAZE_SERVICE_STREAMS, DisplayArea, GazeHub, HeadPoseHub, PrpService, UserPresence,
    parse_display_area, presence_subscribe_time,
    decode_names, encode_frame, encode_pipe_open, error_response, gaze_package, property_error,
    read_frame, request_header, stream_package, synthetic_gaze_frame)
from tobii_linux.prp_session import (
    COMMAND_PROPERTY_UNSUBSCRIBE, PROPERTY_DISPLAY_AREA,
    PROPERTY_DISPLAY_ID, decode_display_area, decode_property_response,
    decode_property_subscribe_response, decode_string_value, encode_property_request,
    encode_property_response, encode_property_subscribe_request,
    encode_property_subscribe_response,
    CommandResponse, HandshakeInfo, encode_command_response, encode_handshake_info,
    COMMAND_CHANNEL, COMMAND_CONNECT, COMMAND_STREAM_START, COMMAND_STREAM_STOP,
    COMMAND_TIMESYNC, CONNECT_RESPONSE_BLOCK_EXTENDED, SUBSCRIPTION_CHANNEL, TIMESYNC_CHANNEL,
    ClientHello, ClientSession, ConnectRequest, check_handshake_ack, decode_command_response,
    decode_handshake_info, encode_client_hello, encode_client_pid, encode_connect_request,
    encode_stream_request, encode_timesync_request, pipe_chunks, split_packets)

REAL_STREAM_PACKAGE = bytes.fromhex(
    "54505250580000000c5052500c00000000000000010000000400000008000000020000000400000034000000"
    "cb000000063ea7e8000000000000000000000000000000000000000000000000000000000000000000000000"
    "000000000400000000000000")
REAL_STREAM_START_RESPONSE = bytes.fromhex(
    "5450525030000000645052500c000000020000000a000000040000000c0000000000000003000000"
    "0400000008000000020000000400000000000000")
REAL_STREAM_STOP_RESPONSE = bytes.fromhex(
    "5450525030000000645052500c000000030000000a000000040000000c0000000000000004000000"
    "0400000008000000020000000400000000000000")
REAL_TIMESYNC_RESPONSE = bytes.fromhex(
    "5450525034000000605052500c000000010000000a000000040000000c0000000000000016000000"
    "040000000c000000cb000000c01fa7e80400000000000000")
REAL_DISPLAY_AREA_RESPONSE = bytes.fromhex(
    "5450525050000000045052500c0000000200000006000000040000000c000000000000000100000004000000"
    "280000005f4f94c38500a34333a3df42a13096438500a34333a3df425f4f94c3fc462441bb6646c0"
    "0400000000000000")
REAL_DISPLAY_ID = "DISPLAY\\GSM5B7F\\5&5028A6B&0&UID41221##eId:1E6D7F5B0B220100061E#or:1#v:1.0"
REAL_DISPLAY_ID_RESPONSE = bytes.fromhex(
    "545052507c000000285052500c0000000300000006000000040000000c000000000000001200000004000000"
    "080000000400000050000000444953504c41595c47534d354237465c35263530323841364226302655494434"
    "3132323123236549643a3145364437463542304232323031303030363145236f723a3123763a312e30000000"
    "00000000")
REAL_MODEL_RESPONSE = bytes.fromhex(
    "54505250480000001c5052500c0000000900000006000000040000000c000000000000000800000004000000"
    "08000000040000001c0000004953355f4c617267655f457965747261636b65725f35003600000000")
REAL_PROPERTY_4_RESPONSE = bytes.fromhex(
    "5450525030000000645052500c0000000400000006000000040000000c000000000000000400000004000000"
    "08000000000000000400000000000000")
REAL_PROPERTY_REFUSALS = {
    (0xd, 0x0):
        "545052502c000000785052500c0000000d00000006000000040000000c000000020000000000000004000000"
        "040000000400000000000000",
    (0x9, 0x3):
        "5450525030000000645052500c0000000900000006000000040000000c000000020000000300000004000000"
        "08000000000000000400000000000000",
    (0xa, 0xa):
        "5450525044000000105052500c0000000a00000006000000040000000c000000020000000a00000004000000"
        "1c0000000000000000000000000000000000000000000000000000000400000000000000",
    (0x2, 0xb):
        "54505250380000006c5052500c0000000200000006000000040000000c000000020000000b00000004000000"
        "100000000000000000000000000000000400000000000000",
    (0x7, 0xd):
        "5450525034000000605052500c0000000700000006000000040000000c000000020000000d00000004000000"
        "0c00000000000000000000000400000000000000",
    (0x8, 0xe):
        "5450525030000000645052500c0000000800000006000000040000000c000000020000000e00000004000000"
        "08000000000000000400000000000000",
    (0x3, 0x14):
        "54505250380000006c5052500c0000000300000006000000040000000c000000020000001400000004000000"
        "100000000000000000000000000000000400000000000000",
    (0xc, 0x17):
        "545052505c000000085052500c0000000c00000006000000040000000c000000020000001700000004000000"
        "3400000000000000000000000000000000000000000000000000000000000000000000000000000000000000"
        "00000000000000000400000000000000",
    (0x4, 0x18):
        "54505250380000006c5052500c0000000400000006000000040000000c000000020000001800000004000000"
        "100000000000000000000000000000000400000000000000",
}
REAL_SUBSCRIBE_RESPONSES = {
    (0x3, 0x1):
        "545052505c000000085052500c000000030000000a000000040000000c000000000000000500000004000000"
        "080000000100000004000000280000005f4f94c38500a34333a3df42a13096438500a34333a3df425f4f94c3"
        "fc462441bb6646c00400000000000000",
    (0x4, 0xb):
        "5450525044000000105052500c000000040000000a000000040000000c000000000000000500000004000000"
        "080000000b0000000400000010000000cd0000005d058c72020000000400000000000000",
    (0x5, 0x2):
        "545052503c000000685052500c000000050000000a000000040000000c000000000000000500000004000000"
        "08000000020000000400000008000000000000000400000000000000",
    (0x6, 0x4):
        "545052503c000000685052500c000000060000000a000000040000000c000000000000000500000004000000"
        "08000000040000000400000008000000000000000400000000000000",
}
# Exclusive mode (5) and presence (0x14), live (Windows), evidence/private/robust-live/prop5.jsonl.
REAL_EXCLUSIVE_MODE_GET = bytes.fromhex(
    "5450525030000000645052500c0000000200000006000000040000000c000000000000000500000004000000"
    "08000000000000000400000000000000")
REAL_SUBSCRIBE_RESPONSES.update({
    (0x4, 0x5):
        "545052503c000000685052500c000000040000000a000000040000000c000000000000000500000004000000"
        "08000000050000000400000008000000000000000400000000000000",
    (0x6, 0x14):
        "5450525044000000105052500c000000060000000a000000040000000c000000110000000500000004000000"
        "080000001400000004000000100000000000000000000000000000000400000000000000",
})
# The real service left memory uninitialised in these answers: the display id string's
# padding, and the refused 0x13 block (it held display id text).
UNINITIALISED_SUBSCRIBE_BYTES = {(5, 0x12), (5, 0x13)}
REAL_UNSUBSCRIBE_LICENSED = {
    (0x9, 0x14):
        "5450525030000000645052500c000000090000000a000000040000000c000000110000000600000004000000"
        "08000000140000000400000000000000",
}
REAL_UNSUBSCRIBE_RESPONSES = {
    (0x7, 0x1):
        "5450525030000000645052500c000000070000000a000000040000000c000000000000000600000004000000"
        "08000000010000000400000000000000",
    (0x8, 0xb):
        "5450525030000000645052500c000000080000000a000000040000000c000000000000000600000004000000"
        "080000000b0000000400000000000000",
}
REAL_PRESENCE_SINCE_US = 0xCD_728C_055D
# Requests as the game DLL sent them to this service under Wine (PRP_DEBUG=1 log).
GAME_DISPLAY_AREA_REQUEST = bytes.fromhex(
    "5450525020000000745052500c00000002000000050000000400000008000000010000000400000000000000")
GAME_PRESENCE_SUBSCRIBE_REQUEST = bytes.fromhex(
    "545052502c000000785052500c00000004000000090000000400000008000000050000000400000008000000"
    "0b0000000400000000000000")
# Gaze session against the real service (gaze-host/gaze-probe-fou1.jsonl): stream start
# and stop responses by tag, after connect with tag 1, for streams 3, 29, 4, 6, 5, 33, 2.
REAL_GAZE_STREAM_RESPONSES = {
    (2, COMMAND_STREAM_START, 3): "0000000003000000040000000800000003000000",
    (3, COMMAND_STREAM_START, 29): "000000000300000004000000080000001d000000",
    (4, COMMAND_STREAM_START, 4): "0000000003000000040000000800000004000000",
    (5, COMMAND_STREAM_START, 6): "1100000003000000040000000800000006000000",
    (6, COMMAND_STREAM_START, 5): "0200000003000000040000000800000005000000",
    (7, COMMAND_STREAM_START, 33): "1100000003000000040000000800000021000000",
    (8, COMMAND_STREAM_STOP, 3): "0000000004000000040000000800000003000000",
}
REAL_SUBSCRIPTION_INFO = bytes.fromhex("080000000000")
REAL_TIMESYNC_INFO = bytes.fromhex("000000000000")
CLIENT_PID = 0x1234
FIXED_CLOCK_US = 0x0000_00CB_E8A7_1FC0


class RelayedChannel:
    """One pipe connection as the bridge relays it, seen from the client's side."""

    def __init__(self, reader, writer):
        self.reader, self.writer = reader, writer
        self.reply_pipe = None
        self.tail = b""

    @classmethod
    async def open(cls, port, pipe, client_pid=CLIENT_PID):
        reader, writer = await asyncio.open_connection("127.0.0.1", port)
        writer.write(encode_frame(FRAME_PIPE_OPEN, encode_pipe_open(client_pid, pipe)))
        return cls(reader, writer)

    def send(self, message):
        self.writer.write(encode_frame(FRAME_MESSAGE, message))

    def send_packet(self, packet):
        for chunk in pipe_chunks(packet):
            self.send(chunk)

    async def receive(self, timeout=2.0):
        while True:
            kind, payload = await asyncio.wait_for(read_frame(self.reader), timeout)
            if kind == FRAME_REPLY_PIPE:
                self.reply_pipe = decode_names(payload)[0]
                continue
            if kind != FRAME_MESSAGE:
                raise AssertionError(f"unexpected frame {kind}")
            return payload

    async def packet(self, timeout=2.0):
        while True:
            packets, self.tail = split_packets(self.tail)
            if packets:
                self.tail = b"".join(packets[1:]) + self.tail
                return packets[0]
            self.tail += await self.receive(timeout)

    async def handshake(self, channel, reply_pipe, pid=CLIENT_PID):
        self.send(encode_client_hello(ClientHello(channel, reply_pipe)))
        check_handshake_ack(await self.receive())
        self.send(encode_client_pid(pid))
        return decode_handshake_info(await self.receive())

    def close(self):
        self.writer.close()


class RealServiceVectorTests(unittest.TestCase):
    """The service's encoders reproduce real-service bytes exactly."""

    def test_stream_package(self):
        timestamp = 0xCB_E8A7_3E06
        sample = HeadPoseSample(timestamp, False, (0.0, 0.0, 0.0), (0.0, 0.0, 0.0))
        self.assertEqual(stream_package(sample), REAL_STREAM_PACKAGE)

    def test_command_responses(self):
        self.assertEqual(encode_command_response(CommandResponse(
            2, COMMAND_STREAM_START, 0, HEAD_POSE)), REAL_STREAM_START_RESPONSE)
        self.assertEqual(encode_command_response(CommandResponse(
            3, COMMAND_STREAM_STOP, 0, HEAD_POSE)), REAL_STREAM_STOP_RESPONSE)
        self.assertEqual(encode_command_response(CommandResponse(
            1, COMMAND_TIMESYNC, 0, 0xCB_E8A7_1FC0)), REAL_TIMESYNC_RESPONSE)

    def test_handshake_info_without_urls(self):
        self.assertEqual(encode_handshake_info(HandshakeInfo(8, "", "")), REAL_SUBSCRIPTION_INFO)
        self.assertEqual(encode_handshake_info(HandshakeInfo(0, "", "")), REAL_TIMESYNC_INFO)


def real_display_area() -> DisplayArea:
    floats = struct.unpack_from("<9f", REAL_DISPLAY_AREA_RESPONSE, 48)
    return DisplayArea(floats[0:3], floats[3:6], floats[6:9])


def presence_service(platmod_value: int) -> PrpService:
    service = PrpService(HeadPoseHub(), "ET5-TEST", clock=lambda: FIXED_CLOCK_US)
    service.publish_presence(REAL_PRESENCE_SINCE_US, platmod_value)
    return service


# Emulated game DLL serializer 0x18004f730 on a queued presence record (tools/presence_oracle.py):
# timestamp 0x112233445566, value 1.
ORIGINAL_PRESENCE_NOTIFICATION = bytes.fromhex(
    "5450525034000000605052500c000000000000000200000004000000080000000b000000040000001000000022"
    "11000066554433010000000400000000000000")


class PropertyVectorTests(unittest.TestCase):
    """Property replies of the service against the real service's bytes (live, Windows)."""

    def setUp(self):
        self.service = PrpService(HeadPoseHub(), "ET5-TEST", display_area=real_display_area(),
                                  display_id=REAL_DISPLAY_ID)

    def reply(self, tag, property_id):
        return encode_property_response(self.service.answer_property(tag, property_id))

    def test_requests_match_the_game_dll(self):
        self.assertEqual(encode_property_request(2, PROPERTY_DISPLAY_AREA),
                         GAME_DISPLAY_AREA_REQUEST)
        self.assertEqual(encode_property_subscribe_request(4, 0xB),
                         GAME_PRESENCE_SUBSCRIBE_REQUEST)

    def test_display_area_and_id(self):
        self.assertEqual(self.reply(2, PROPERTY_DISPLAY_AREA), REAL_DISPLAY_AREA_RESPONSE)
        self.assertEqual(self.reply(3, PROPERTY_DISPLAY_ID), REAL_DISPLAY_ID_RESPONSE)
        area = decode_display_area(decode_property_response(REAL_DISPLAY_AREA_RESPONSE))
        self.assertAlmostEqual(area[3] - area[0], 597.0, places=3)
        self.assertEqual(decode_string_value(decode_property_response(REAL_DISPLAY_ID_RESPONSE)),
                         REAL_DISPLAY_ID)

    def test_model_name_differs_only_in_padding(self):
        reply = self.reply(9, 8)
        self.assertEqual(len(reply), len(REAL_MODEL_RESPONSE))
        self.assertEqual(reply[:-6], REAL_MODEL_RESPONSE[:-6])
        self.assertEqual(decode_string_value(decode_property_response(reply)),
                         decode_string_value(decode_property_response(REAL_MODEL_RESPONSE)))

    def test_property_4(self):
        self.assertEqual(self.reply(4, 4), REAL_PROPERTY_4_RESPONSE)

    def test_refusals(self):
        for (tag, property_id), expected in REAL_PROPERTY_REFUSALS.items():
            with self.subTest(property_id=property_id):
                self.assertEqual(self.reply(tag, property_id), bytes.fromhex(expected))

    def test_unconfigured_display_is_refused(self):
        service = PrpService(HeadPoseHub(), "ET5-TEST")
        for property_id in (PROPERTY_DISPLAY_AREA, PROPERTY_DISPLAY_ID):
            with self.subTest(property_id=property_id):
                self.assertEqual(service.answer_property(5, property_id),
                                 property_error(5, property_id))

    def test_subscribe_responses(self):
        service = presence_service(gaze.PLATMOD_AWAY)
        service.display_area = real_display_area()
        for (tag, property_id), expected in REAL_SUBSCRIBE_RESPONSES.items():
            with self.subTest(property_id=property_id):
                response = service.answer_property(tag, property_id, subscribe=True)
                self.assertEqual(encode_property_subscribe_response(response),
                                 bytes.fromhex(expected))

    def test_presence_follows_the_device(self):
        for platmod, wire in ((gaze.PLATMOD_UNKNOWN, 1), (gaze.PLATMOD_AWAY, 2),
                              (gaze.PLATMOD_PRESENT, 3)):
            with self.subTest(platmod=platmod):
                response = presence_service(platmod).answer_property(1, 0xB, subscribe=True)
                self.assertEqual(struct.unpack("<IIII", response.block),
                                 (0x10, REAL_PRESENCE_SINCE_US >> 32,
                                  REAL_PRESENCE_SINCE_US & 0xFFFFFFFF, wire))

    def test_presence_is_refused_before_any_record(self):
        service = PrpService(HeadPoseHub(), "ET5-TEST")
        self.assertEqual(service.answer_property(1, 0xB, subscribe=True), property_error(1, 0xB))

    def test_every_record_after_the_first_is_notified(self):
        presence = UserPresence()
        notified = [presence.record(time_us, value)
                    for time_us, value in ((10, 0), (20, 1), (30, 1), (40, 2))]
        self.assertEqual(notified, [False, True, True, True])
        self.assertEqual((presence.value, presence.since_us, presence.present), (3, 40, True))
        presence.clear()
        self.assertFalse(presence.record(50, 2))

    def test_notification_matches_the_original_serializer(self):
        presence = UserPresence()
        presence.record(0x112233445566, gaze.PLATMOD_UNKNOWN)
        self.assertEqual(presence.notification(), ORIGINAL_PRESENCE_NOTIFICATION)

    def test_subscribe_time_leads_the_tracker_by_30_ms(self):
        self.assertEqual(presence_subscribe_time(900_000_000_000), 899_999_970_000)
        self.assertEqual(presence_subscribe_time(12_345), 12_345)

    def test_exclusive_mode_get(self):
        self.assertEqual(self.reply(2, 5), REAL_EXCLUSIVE_MODE_GET)

    def test_unsubscribe_responses(self):
        for (tag, property_id), expected in REAL_UNSUBSCRIBE_RESPONSES.items():
            with self.subTest(property_id=property_id):
                self.assertEqual(encode_command_response(CommandResponse(
                    tag, COMMAND_PROPERTY_UNSUBSCRIBE, 0, property_id)), bytes.fromhex(expected))

    def test_display_area_option(self):
        area = parse_display_area("-296.62,326.0,111.8,300.38,326.0,111.8,-296.62,10.27,-3.1")
        self.assertEqual(area.top_right, (300.38, 326.0, 111.8))
        with self.assertRaises(ValueError):
            parse_display_area("1,2,3")


class ServiceTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        self.hub = HeadPoseHub()
        self.service = PrpService(self.hub, "ET5-TEST", clock=lambda: FIXED_CLOCK_US)
        self.attach()
        self.server = await self.service.serve("127.0.0.1", 0)
        self.port = self.server.sockets[0].getsockname()[1]
        self.channels = []

    def attach(self):
        self.service.attach_tracker()
        self.service.publish_presence(REAL_PRESENCE_SINCE_US, gaze.PLATMOD_AWAY)

    async def asyncTearDown(self):
        for channel in self.channels:
            channel.close()
        await self.service.close(self.server)

    async def channel(self, pipe, **kwargs):
        channel = await RelayedChannel.open(self.port, pipe, **kwargs)
        self.channels.append(channel)
        return channel

    async def session(self, licenses=()):
        command = await self.channel("ET5-TEST")
        info = await command.handshake(COMMAND_CHANNEL, "client_TOBIIPRP-ET5-TEST_00001234_1")
        subscription_pipe = info.subscription_url.removeprefix("prp-internal://")
        subscription = await self.channel(subscription_pipe)
        sub_info = await subscription.handshake(SUBSCRIPTION_CHANNEL, "client_sub")
        tags = ClientSession()
        tag = tags.next_command_tag()
        command.send_packet(encode_connect_request(tag, ConnectRequest(
            sub_info.client_id, licenses, process_id=CLIENT_PID,
            image_directory="\\Games\\".encode("utf-16-le"),
            image_name="StarCitizen".encode("utf-16-le"))))
        packet = await command.packet()
        return command, subscription, info, tags, tag, packet

    async def test_bridge_reported_display_id_is_used_unless_configured(self):
        reader, writer = await asyncio.open_connection("127.0.0.1", self.port)
        writer.write(encode_frame(FRAME_BRIDGE_HELLO, struct.pack("<I", 1)))
        await read_frame(reader)
        writer.write(encode_frame(FRAME_DISPLAY_ID, "DISPLAY\\DEFAULT_MONITOR\\0000&0000".encode()))
        await writer.drain()
        for _ in range(100):
            if self.service.reported_display_id:
                break
            await asyncio.sleep(0.01)
        writer.close()
        reply = self.service.answer_property(3, PROPERTY_DISPLAY_ID)
        self.assertEqual(decode_string_value(reply), "DISPLAY\\DEFAULT_MONITOR\\0000&0000")
        configured = PrpService(HeadPoseHub(), "ET5-TEST", display_id=REAL_DISPLAY_ID)
        configured.reported_display_id = "DISPLAY\\DEFAULT_MONITOR\\0000&0000"
        self.assertEqual(decode_string_value(configured.answer_property(3, PROPERTY_DISPLAY_ID)),
                         REAL_DISPLAY_ID)

    async def test_bridge_learns_pipe_names(self):
        reader, writer = await asyncio.open_connection("127.0.0.1", self.port)
        writer.write(encode_frame(FRAME_BRIDGE_HELLO, struct.pack("<I", 1)))
        kind, payload = await read_frame(reader)
        writer.close()
        names = decode_names(payload)
        self.assertEqual(kind, FRAME_PIPES)
        self.assertEqual(names[:2], ["TOBIIPRP-ET5-TEST", "ET5-TEST"])
        self.assertEqual(names[2:], [self.service.subscription_pipe, self.service.timesync_pipe])

    async def test_handshake_matches_real_service_shape(self):
        command = await self.channel("TOBIIPRP-ET5-TEST")
        info = await command.handshake(COMMAND_CHANNEL, "client_TOBIIPRP-ET5-TEST_00001234_1")
        self.assertEqual(command.reply_pipe, "client_TOBIIPRP-ET5-TEST_00001234_1")
        self.assertGreater(info.client_id, 0)
        self.assertTrue(info.subscription_url.startswith("prp-internal://"))
        self.assertTrue(info.timesync_url.startswith("prp-internal://"))
        self.assertLess(max(map(len, (info.subscription_url, info.timesync_url))), 128)
        subscription = await self.channel(self.service.subscription_pipe)
        sub_info = await subscription.handshake(SUBSCRIPTION_CHANNEL, "client_sub")
        self.assertEqual((sub_info.subscription_url, sub_info.timesync_url), ("", ""))
        self.assertNotEqual(sub_info.client_id, -1)
        timesync = await self.channel(self.service.timesync_pipe)
        self.assertEqual((await timesync.handshake(TIMESYNC_CHANNEL, "client_ts")).client_id, 0)

    async def test_connect_satisfies_device_create(self):
        for licenses in ((), (b"license-one", b"license-two")):
            with self.subTest(licenses=len(licenses)):
                *_, tag, packet = await self.session(licenses)
                payload = decode_envelope(packet)
                response = decode_command_response(packet)
                self.assertEqual((response.tag, response.command, response.status),
                                 (tag, COMMAND_CONNECT, 0))
                self.assertEqual(struct.unpack_from("<I", payload, 32)[0],
                                 CONNECT_RESPONSE_BLOCK_EXTENDED)
                connect = response.connect
                self.assertEqual(connect.license_results, (0,) * len(licenses))
                self.assertFalse(connect.firmware_upgrade_in_progress)
                self.assertIn(HEAD_POSE, connect.streams)
                self.assertIn(COMMAND_TIMESYNC, connect.supported_commands)

    async def test_connect_with_unknown_subscription_fails(self):
        command = await self.channel("ET5-TEST")
        await command.handshake(COMMAND_CHANNEL, "client_cmd")
        command.send_packet(encode_connect_request(1, ConnectRequest(-1)))
        response = decode_command_response(await command.packet())
        self.assertNotEqual(response.status, 0)

    async def test_stream_start_pushes_samples_until_stop(self):
        command, subscription, _, tags, _, _ = await self.session()
        tag = tags.next_command_tag()
        command.send_packet(encode_stream_request(tag, COMMAND_STREAM_START, HEAD_POSE))
        response = decode_command_response(await command.packet())
        self.assertEqual((response.tag, response.status, response.value), (tag, 0, HEAD_POSE))
        sample = HeadPoseSample(FIXED_CLOCK_US + 5, True, (1.0, 2.0, 600.0), (0.1, -0.2, 0.3))
        self.hub.publish(sample)
        package = await subscription.packet()
        self.assertEqual(package, stream_package(sample))
        payload = decode_envelope(package)
        self.assertEqual(struct.unpack_from("<7I", payload), (12, 0, 1, 4, 8, HEAD_POSE, 4))
        pose = decode_head_body(payload[28:80])
        self.assertEqual(pose.timestamp, sample.timestamp_us)
        self.assertEqual(pose.rotation_valid, (True, True, True))
        self.assertAlmostEqual(pose.rotation[1], -0.2, places=6)
        tag = tags.next_command_tag()
        command.send_packet(encode_stream_request(tag, COMMAND_STREAM_STOP, HEAD_POSE))
        response = decode_command_response(await command.packet())
        self.assertEqual((response.status, response.value), (0, HEAD_POSE))
        self.assertEqual(self.hub.queues, set())
        self.hub.publish(sample)
        with self.assertRaises(asyncio.TimeoutError):
            await subscription.packet(timeout=0.2)

    async def test_client_disconnect_releases_stream(self):
        command, subscription, _, tags, _, _ = await self.session()
        command.send_packet(encode_stream_request(tags.next_command_tag(), COMMAND_STREAM_START,
                                                  HEAD_POSE))
        await command.packet()
        self.assertEqual(len(self.hub.queues), 1)
        command.close()
        subscription.close()
        for _ in range(50):
            if not self.hub.queues and not self.service.subscriptions:
                break
            await asyncio.sleep(0.02)
        self.assertEqual((self.hub.queues, self.service.subscriptions), (set(), {}))

    async def test_invalid_sample_is_flagged_invalid(self):
        package = stream_package(HeadPoseSample(7, False, (0.0, 0.0, 0.0), (0.0, 0.0, 0.0)))
        pose = decode_head_body(decode_envelope(package)[28:80])
        self.assertFalse(pose.position_valid)
        self.assertEqual(pose.rotation_valid, (False, False, False))

    async def test_stream_requests_for_other_streams_and_before_start(self):
        command, _, _, tags, _, _ = await self.session()
        tag = tags.next_command_tag()
        command.send_packet(encode_stream_request(tag, COMMAND_STREAM_START, 3))
        self.assertNotEqual(decode_command_response(await command.packet()).status, 0)
        tag = tags.next_command_tag()
        command.send_packet(encode_stream_request(tag, COMMAND_STREAM_STOP, HEAD_POSE))
        self.assertNotEqual(decode_command_response(await command.packet()).status, 0)

    async def test_timesync_on_its_channel(self):
        timesync = await self.channel(self.service.timesync_pipe)
        await timesync.handshake(TIMESYNC_CHANNEL, "client_ts")
        tags = ClientSession()
        tag = tags.next_timesync_tag()
        timesync.send_packet(encode_timesync_request(tag))
        packet = await timesync.packet()
        response = decode_command_response(packet)
        self.assertEqual((response.tag, response.command, response.status, response.value),
                         (tag, COMMAND_TIMESYNC, 0, FIXED_CLOCK_US))
        self.assertEqual(packet, REAL_TIMESYNC_RESPONSE)

    async def test_unknown_command_gets_nonzero_status_with_same_tag(self):
        command, *_ = await self.session()
        header = struct.pack("<IIII", 12, 9, 9, 4)
        request = encode_envelope(header + struct.pack("<IIII", 8, 2, 4, 0x10)
                                  + bytes(12) + struct.pack("<II", 4, 0))
        self.assertEqual((request_header(request).tag, request_header(request).command), (9, 2))
        command.send_packet(request)
        packet = await command.packet()
        self.assertEqual(packet, error_response(9, 2, 2))
        payload = decode_envelope(packet)
        self.assertEqual(struct.unpack_from("<IIIIIiI", payload), (12, 9, 10, 4, 12, 2, 2))

    async def test_property_get_and_subscribe_keep_the_session(self):
        self.service.display_area = real_display_area()
        command, subscription, _, tags, _, _ = await self.session()
        tag = tags.next_command_tag()
        command.send_packet(encode_property_request(tag, PROPERTY_DISPLAY_AREA))
        response = decode_property_response(await command.packet())
        self.assertEqual((response.tag, response.status, response.property_id),
                         (tag, 0, PROPERTY_DISPLAY_AREA))
        tag = tags.next_command_tag()
        command.send_packet(encode_property_request(tag, PROPERTY_DISPLAY_ID))
        response = decode_property_response(await command.packet())
        self.assertEqual((response.tag, response.status), (tag, 2))
        tag = tags.next_command_tag()
        command.send_packet(encode_property_subscribe_request(tag, 0xB))
        response = decode_property_subscribe_response(await command.packet())
        self.assertEqual((response.tag, response.status, response.property_id), (tag, 0, 0xB))
        tag = tags.next_command_tag()
        command.send_packet(encode_property_subscribe_request(tag, 0xB,
                                                              COMMAND_PROPERTY_UNSUBSCRIBE))
        response = decode_command_response(await command.packet())
        self.assertEqual((response.tag, response.command, response.status, response.value),
                         (tag, COMMAND_PROPERTY_UNSUBSCRIBE, 0, 0xB))
        tag = tags.next_command_tag()
        command.send_packet(encode_stream_request(tag, COMMAND_STREAM_START, HEAD_POSE))
        self.assertEqual(decode_command_response(await command.packet()).status, 0)
        self.hub.publish(HeadPoseSample(FIXED_CLOCK_US, True, (0.0,) * 3, (0.0,) * 3))
        self.assertTrue(await subscription.packet())

    async def test_unsupported_property_subscribe_carries_a_value_block(self):
        command, _, _, tags, _, _ = await self.session()
        tag = tags.next_command_tag()
        command.send_packet(encode_property_subscribe_request(tag, 3))
        response = decode_property_subscribe_response(await command.packet())
        self.assertEqual((response.tag, response.status, response.property_id), (tag, 2, 3))
        self.assertEqual(response.block, struct.pack("<II", 8, 0))

    async def test_every_property_subscription_matches_the_real_service(self):
        """Commands 5 and 6 for properties 1 to 24 against the real service's answers.

The real run (field_of_use 2) is in tests/data/real-property-subscriptions.json.
"""
        import json
        from pathlib import Path
        real = json.loads((Path(__file__).resolve().parent/'data/real-property-subscriptions.json')
                          .read_text())
        self.service.display_area = real_display_area()
        self.service.display_id = REAL_DISPLAY_ID
        self.service.calibration_id = 0xBC381A37
        self.service.presence.clear()
        self.service.publish_presence(0xD4_41C5_AAA6, gaze.PLATMOD_AWAY)
        command, subscription, _, tags, _, _ = await self.session_with_field_of_use(2)
        for command_id in (5, 6):
            key = 'subscribe' if command_id == 5 else 'unsubscribe'
            for property_id, vector in sorted(real[key].items(), key=lambda item: int(item[0])):
                with self.subTest(command=command_id, property_id=int(property_id)):
                    command.send_packet(encode_property_subscribe_request(
                        vector['tag'], int(property_id), command_id))
                    packet, expected = await command.packet(), bytes.fromhex(vector['hex'])
                    if (command_id, int(property_id)) in UNINITIALISED_SUBSCRIBE_BYTES:
                        self.assertEqual(len(packet), len(expected))
                        self.assertEqual(packet[:52], expected[:52])
                    else:
                        self.assertEqual(packet, expected)

    async def session_with_field_of_use(self, field_of_use):
        command = await self.channel("ET5-TEST")
        info = await command.handshake(COMMAND_CHANNEL, "client_TOBIIPRP-ET5-TEST_00001234_1")
        subscription = await self.channel(info.subscription_url.removeprefix("prp-internal://"))
        sub_info = await subscription.handshake(SUBSCRIPTION_CHANNEL, "client_sub")
        tags = ClientSession()
        tag = tags.next_command_tag()
        command.send_packet(encode_connect_request(tag, ConnectRequest(
            sub_info.client_id, process_id=CLIENT_PID, field_of_use=field_of_use)))
        packet = await command.packet()
        return command, subscription, info, tags, tag, packet

    async def test_licensed_presence_unsubscribe_matches_the_real_service(self):
        command, *_ = await self.session()
        for (tag, property_id), expected in REAL_UNSUBSCRIBE_LICENSED.items():
            command.send_packet(encode_property_subscribe_request(tag, property_id,
                                                                  COMMAND_PROPERTY_UNSUBSCRIBE))
            self.assertEqual(await command.packet(), bytes.fromhex(expected))

    async def closed(self, channel):
        """The service closed `channel`; messages it sent before closing are skipped."""
        with self.assertRaises((asyncio.IncompleteReadError, ConnectionError)):
            for _ in range(1000):
                await channel.receive()

    async def subscribe_presence(self, command, tags):
        tag = tags.next_command_tag()
        command.send_packet(encode_property_subscribe_request(tag, 0xB))
        return decode_property_subscribe_response(await command.packet())

    async def test_presence_changes_reach_only_subscribed_clients(self):
        watching, watching_sub, _, watching_tags, _, _ = await self.session()
        other, other_sub, _, other_tags, _, _ = await self.session()
        response = await self.subscribe_presence(watching, watching_tags)
        self.assertEqual(struct.unpack("<IIII", response.block)[3], 2)
        self.service.publish_presence(FIXED_CLOCK_US + 10, gaze.PLATMOD_PRESENT)
        notification = await watching_sub.packet()
        self.assertEqual(notification, self.service.presence.notification())
        self.assertEqual(struct.unpack_from("<3I", decode_envelope(notification)), (12, 0, 2))
        with self.assertRaises(asyncio.TimeoutError):
            await other_sub.packet(timeout=0.2)
        tag = watching_tags.next_command_tag()
        watching.send_packet(encode_property_subscribe_request(tag, 0xB,
                                                               COMMAND_PROPERTY_UNSUBSCRIBE))
        await watching.packet()
        self.service.publish_presence(FIXED_CLOCK_US + 20, gaze.PLATMOD_AWAY)
        with self.assertRaises(asyncio.TimeoutError):
            await watching_sub.packet(timeout=0.2)
        self.assertEqual(self.service.sent_notifications, 1)

    async def test_two_clients_receive_every_sample(self):
        clients = [await self.session() for _ in range(2)]
        for command, _, _, tags, _, _ in clients:
            command.send_packet(encode_stream_request(tags.next_command_tag(),
                                                      COMMAND_STREAM_START, HEAD_POSE))
            self.assertEqual(decode_command_response(await command.packet()).status, 0)
        samples = [HeadPoseSample(FIXED_CLOCK_US + i, True, (float(i), 0.0, 600.0), (0.0,) * 3)
                   for i in range(3)]
        for sample in samples:
            self.hub.publish(sample)
            await asyncio.sleep(0.01)
        for _, subscription, *_ in clients:
            self.assertEqual([await subscription.packet() for _ in samples],
                             [stream_package(sample) for sample in samples])
        first_command, first_sub, _, first_tags, _, _ = clients[0]
        first_command.send_packet(encode_stream_request(first_tags.next_command_tag(),
                                                        COMMAND_STREAM_STOP, HEAD_POSE))
        await first_command.packet()
        self.hub.publish(samples[0])
        self.assertEqual(await clients[1][1].packet(), stream_package(samples[0]))
        with self.assertRaises(asyncio.TimeoutError):
            await first_sub.packet(timeout=0.2)

    async def test_closing_the_command_channel_closes_its_subscription(self):
        command, subscription, *_ = await self.session()
        command.close()
        await self.closed(subscription)
        for _ in range(50):
            if not self.service.subscriptions:
                break
            await asyncio.sleep(0.02)
        self.assertEqual(self.service.subscriptions, {})

    async def test_tracker_loss_closes_clients_until_it_is_back(self):
        command, subscription, _, tags, _, _ = await self.session()
        command.send_packet(encode_stream_request(tags.next_command_tag(), COMMAND_STREAM_START,
                                                  HEAD_POSE))
        await command.packet()
        self.service.detach_tracker()
        await self.closed(command)
        await self.closed(subscription)
        self.assertEqual(self.hub.queues, set())
        refused = await self.channel("ET5-TEST")
        refused.send(encode_client_hello(ClientHello(COMMAND_CHANNEL, "client_cmd")))
        await self.closed(refused)
        self.attach()
        *_, packet = await self.session()
        self.assertEqual(decode_command_response(packet).status, 0)

    async def test_pid_mismatch_closes_connection(self):
        command = await self.channel("ET5-TEST", client_pid=CLIENT_PID)
        command.send(encode_client_hello(ClientHello(COMMAND_CHANNEL, "client_cmd")))
        check_handshake_ack(await command.receive())
        command.send(encode_client_pid(CLIENT_PID + 1))
        with self.assertRaises(asyncio.IncompleteReadError):
            await command.receive()

    async def test_version_zero_hello_closes_connection(self):
        command = await self.channel("ET5-TEST")
        command.send(struct.pack("<II", 0, COMMAND_CHANNEL) + b"client_cmd\0")
        with self.assertRaises(asyncio.IncompleteReadError):
            await command.receive()


class WineClockTests(unittest.TestCase):
    def test_clock_follows_the_wine_counter_source(self):
        import time
        from tobii_linux.prp_service import wine_clock
        for name, clock_id in (("boottime", time.CLOCK_BOOTTIME), ("raw", time.CLOCK_MONOTONIC_RAW)):
            before = time.clock_gettime_ns(clock_id) // 1000
            value = wine_clock(name)()
            after = time.clock_gettime_ns(clock_id) // 1000
            self.assertLessEqual(before, value)
            self.assertLessEqual(value, after)

    def test_unknown_clock_is_rejected(self):
        from tobii_linux.prp_service import wine_clock
        with self.assertRaises(ValueError):
            wine_clock("realtime")


if __name__ == "__main__":
    unittest.main()


class GazeServiceTests(ServiceTests):
    async def asyncSetUp(self):
        self.hub, self.gaze_hub = HeadPoseHub(), GazeHub()
        self.service = PrpService(self.hub, "ET5-TEST", clock=lambda: FIXED_CLOCK_US,
                                  gaze_hub=self.gaze_hub)
        self.attach()
        self.server = await self.service.serve("127.0.0.1", 0)
        self.port = self.server.sockets[0].getsockname()[1]
        self.channels = []

    async def test_connect_lists_the_real_service_gaze_streams(self):
        *_, packet = await self.session()
        self.assertEqual(decode_command_response(packet).connect.streams, GAZE_SERVICE_STREAMS)

    async def test_stream_responses_match_the_real_service(self):
        command, *_ = await self.session()
        for (tag, request, stream), block in REAL_GAZE_STREAM_RESPONSES.items():
            with self.subTest(tag=tag, stream=stream):
                command.send_packet(encode_stream_request(tag, request, stream))
                packet = await command.packet()
                expected = bytes.fromhex(
                    "5450525030000000645052500c000000" + f"{tag:02x}000000"
                    + "0a000000040000000c000000" + block + "0400000000000000")
                self.assertEqual(packet, expected)

    async def test_gaze_streams_push_each_frame_until_stopped(self):
        command, subscription, _, tags, _, _ = await self.session()
        streams = (gaze.PRP_GAZE_POINT, gaze.PRP_RESPONSIVE_GAZE_POINT, gaze.PRP_GAZE_ORIGIN)
        for stream in streams:
            command.send_packet(encode_stream_request(tags.next_command_tag(),
                                                      COMMAND_STREAM_START, stream))
            self.assertEqual(decode_command_response(await command.packet()).status, 0)
        frame = synthetic_gaze_frame(FIXED_CLOCK_US + 9, FIXED_CLOCK_US)
        self.gaze_hub.publish(frame)
        packages = {struct.unpack_from("<I", decode_envelope(p), 20)[0]: p
                    for p in [await subscription.packet() for _ in streams]}
        self.assertEqual(packages, {stream: gaze_package(frame, stream) for stream in streams})
        point = decode_gaze_body(decode_envelope(packages[gaze.PRP_GAZE_POINT])[28:52])
        self.assertEqual((point.timestamp, point.valid, (point.x, point.y)),
                         (FIXED_CLOCK_US + 9, True, frame.device.combined_point))
        for stream in streams:
            command.send_packet(encode_stream_request(tags.next_command_tag(),
                                                      COMMAND_STREAM_STOP, stream))
            self.assertEqual(decode_command_response(await command.packet()).status, 0)
        self.assertEqual(self.gaze_hub.queues, set())

    async def test_head_pose_and_gaze_streams_are_independent(self):
        command, subscription, _, tags, _, _ = await self.session()
        for stream in (HEAD_POSE, gaze.PRP_GAZE_POINT):
            command.send_packet(encode_stream_request(tags.next_command_tag(),
                                                      COMMAND_STREAM_START, stream))
            await command.packet()
        command.send_packet(encode_stream_request(tags.next_command_tag(), COMMAND_STREAM_STOP,
                                                  HEAD_POSE))
        await command.packet()
        self.hub.publish(HeadPoseSample(FIXED_CLOCK_US, True, (0.0, 0.0, 600.0), (0.0, 0.0, 0.0)))
        frame = synthetic_gaze_frame(FIXED_CLOCK_US, FIXED_CLOCK_US)
        self.gaze_hub.publish(frame)
        self.assertEqual(await subscription.packet(), gaze_package(frame, gaze.PRP_GAZE_POINT))
        with self.assertRaises(asyncio.TimeoutError):
            await subscription.packet(timeout=0.2)

    async def test_gaze_disconnect_releases_streams(self):
        command, subscription, _, tags, _, _ = await self.session()
        command.send_packet(encode_stream_request(tags.next_command_tag(), COMMAND_STREAM_START,
                                                  gaze.PRP_GAZE_POINT))
        await command.packet()
        command.close()
        subscription.close()
        for _ in range(50):
            if not self.gaze_hub.queues:
                break
            await asyncio.sleep(0.02)
        self.assertEqual(self.gaze_hub.queues, set())

    async def test_stream_requests_for_other_streams_and_before_start(self):
        command, _, _, tags, _, _ = await self.session()
        command.send_packet(encode_stream_request(tags.next_command_tag(), COMMAND_STREAM_START, 7))
        self.assertEqual(decode_command_response(await command.packet()).status, 2)
        command.send_packet(encode_stream_request(tags.next_command_tag(), COMMAND_STREAM_STOP,
                                                  gaze.PRP_GAZE_POINT))
        self.assertEqual(decode_command_response(await command.packet()).status, 5)
