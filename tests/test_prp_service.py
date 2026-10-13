"""PRP service tests: a simulated bridge drives the service with the client-side codecs.

The checks mirror what the game DLL requires (docs/service-transport.md) and what
the real service sent on albedo (docs/prp-bridge.md). No pipes or Wine are involved.
REAL_* vectors were captured live from the real ET5 service with
tools/prp_real_service_probe.py; nobody was in front of the tracker, so the pose is invalid.
"""
import asyncio
import struct
import unittest

from tobii_linux.head_pose_pipeline import HeadPoseSample
from tobii_linux.prp import HEAD_POSE, decode_envelope, decode_head_body, encode_envelope
from tobii_linux.prp_service import (
    FRAME_BRIDGE_HELLO, FRAME_MESSAGE, FRAME_PIPE_OPEN, FRAME_PIPES, FRAME_REPLY_PIPE,
    HeadPoseHub, PrpService, decode_names, encode_frame, encode_pipe_open, error_response,
    read_frame, request_header, stream_package)
from tobii_linux.prp_session import (
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


class ServiceTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        self.hub = HeadPoseHub()
        self.service = PrpService(self.hub, "ET5-TEST", clock=lambda: FIXED_CLOCK_US)
        self.server = await self.service.serve("127.0.0.1", 0)
        self.port = self.server.sockets[0].getsockname()[1]
        self.channels = []

    async def asyncTearDown(self):
        for channel in self.channels:
            channel.close()
        self.server.close()
        await self.server.wait_closed()

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
