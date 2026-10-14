"""Production tet-tcp endpoint: device replies, stream fan-out and timestamps on the game's clock."""
import asyncio
import unittest

from tobii_linux import gaze
from tobii_linux.tet_tcp import UNSUPPORTED_OPERATION
from tobii_linux.tet_tcp_service import (GAZE_STREAM, PRESENCE_STREAM, DeviceReplies,
                                         TetTcpService, restamp)
from tobii_linux.ttp import Decoder, Message, ProtocolError, protocol_upgrade
from tobii_linux.ttp_commands import (Command, REQUEST_KIND, RESPONSE_KIND, STREAM_DATA_KIND,
                                      header_only_request, request, subscribe_stream,
                                      unsubscribe_stream, Stream)

# Real ET5 replies (evidence/private/gaze-capture/device-query.jsonl, live).
DEVICE_INFO_PAYLOAD = bytes.fromhex(
    "000014000000160000001249533546462d313030323034313431343633140000001a000000164953355f4c61"
    "7267655f457965747261636b65725f35140000000700000003495335140000000e0000000a30326131613661"
    "393737")
DISPLAY_AREA_PAYLOAD = bytes.fromhex(
    '0000050000000400031f410400000008fffb5d85080000000400000008000518042800000004000000080001bf4666000000'
    '050000000400031f4104000000080004b185080000000400000008000518042800000004000000080001bf46660000000500'
    '00000400031f410400000008fffb5d8508000000040000000800002911bf0000000400000008fffff3999450000005000000'
    '0400010100020000000400003039')
UPGRADE_PAYLOAD = bytes.fromhex("0000020000000400010008")


def reply(command, payload=b"", status=0):
    return Message(RESPONSE_KIND, 0, 1, command, status, payload)


def gaze_message(timestamp):
    return gaze.gaze_stream_message({1: gaze.int64_column(timestamp), 27: gaze.uint32_column(1),
                                     28: gaze.vec2_column((0.25, 0.75))})


class FakeSession:
    """Answers getters like a tracker; 1460 is never answered."""

    def __init__(self):
        self.startup_replies = {Command.PROTOCOL_UPGRADE: reply(1000, UPGRADE_PAYLOAD)}
        self.requests = []

    def exchange(self, build, timeout_s):
        message = build(len(self.requests) + 1)
        self.requests.append(message)
        if message.command == 1460:
            return None
        if message.command == Command.GET_DEVICE_INFO:
            return Message(RESPONSE_KIND, message.request_id, 1, message.command, 0, DEVICE_INFO_PAYLOAD)
        return Message(RESPONSE_KIND, message.request_id, 1, message.command, 0x20000501)


class ReplyCaptureTests(unittest.TestCase):
    def test_capture_keeps_startup_replies_and_every_answer(self):
        session = FakeSession()
        device = DeviceReplies.capture(session)
        self.assertTrue(all(m.kind == REQUEST_KIND and not m.payload for m in session.requests))
        self.assertIn(Command.GET_DEVICE_INFO, device.replies)
        self.assertEqual(device.replies[Command.GET_DEVICE_PROPERTIES].word4, 0x20000501)
        self.assertNotIn(1460, device.replies)
        self.assertEqual(device.replies[Command.PROTOCOL_UPGRADE].payload, UPGRADE_PAYLOAD)
        self.assertIsNone(device.display_area())

    def test_display_area_comes_from_the_1430_reply(self):
        area = DeviceReplies({Command.GET_DISPLAY_AREA: reply(1430, DISPLAY_AREA_PAYLOAD)}).display_area()
        self.assertAlmostEqual(area.top_left[0], -296.62008666992, places=4)
        self.assertAlmostEqual(area.bottom_left[2], -3.1000201702, places=4)


class RestampTests(unittest.TestCase):
    def test_only_the_timestamp_changes(self):
        original = gaze_message(908279389020)
        payload = restamp(original.payload, 43716142484)
        self.assertEqual(len(payload), len(original.payload))
        decoded = gaze.decode_gaze_message(payload)
        self.assertEqual(decoded.timestamp, 43716142484)
        self.assertEqual(decoded.combined_point, (0.25, 0.75))
        self.assertEqual(restamp(payload, 908279389020), original.payload)

    def test_presence_and_negative_times(self):
        payload = gaze.columns_payload({1: gaze.int64_column(5), 2: gaze.int32_column(2)})
        self.assertEqual(gaze.presence_message(restamp(payload, -7)), (-7, 2))

    def test_message_without_timestamp_is_rejected(self):
        with self.assertRaises(ProtocolError):
            restamp(gaze.columns_payload({27: gaze.uint32_column(1)}), 1)


class ServiceTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        self.service = TetTcpService(game_time=lambda us: us - 1000, port=0)
        replies = {Command.PROTOCOL_UPGRADE: reply(1000, UPGRADE_PAYLOAD),
                   Command.GET_DEVICE_INFO: reply(1420, DEVICE_INFO_PAYLOAD),
                   Command.GET_DEVICE_PROPERTIES: reply(1330, status=0x20000501)}
        await self.service.attach(DeviceReplies(replies, frozenset({GAZE_STREAM, PRESENCE_STREAM,
                                                                     0x501})))
        self.reader, self.writer = await asyncio.open_connection("127.0.0.1", self.service.port_bound)
        self.decoder = Decoder()
        self.pending = []

    async def asyncTearDown(self):
        self.writer.close()
        await self.service.detach()

    async def ask(self, message):
        self.writer.write(message.encode())
        return await self.receive()

    async def receive(self, timeout=2.0):
        while not self.pending:
            self.pending += self.decoder.feed(await asyncio.wait_for(self.reader.read(65536), timeout))
        return self.pending.pop(0)

    async def test_startup_and_getters_come_from_the_device_replies(self):
        upgrade = await self.ask(protocol_upgrade(7))
        self.assertEqual((upgrade.kind, upgrade.request_id, upgrade.payload), (RESPONSE_KIND, 7, UPGRADE_PAYLOAD))
        info = await self.ask(header_only_request(8, Command.GET_DEVICE_INFO))
        self.assertEqual((info.request_id, info.word4, info.payload), (8, 0, DEVICE_INFO_PAYLOAD))
        properties = await self.ask(header_only_request(9, Command.GET_DEVICE_PROPERTIES))
        self.assertEqual((properties.request_id, properties.word4), (9, 0x20000501))

    async def test_writes_and_unknown_commands_are_refused(self):
        for command in (1440, Command.SET_FREQUENCIES, 3000, Command.GET_TRACK_BOX):
            answer = await self.ask(request(11, command))
            self.assertEqual((answer.command, answer.word4), (command, UNSUPPORTED_OPERATION))

    async def test_subscribed_connections_get_restamped_streams(self):
        answer = await self.ask(subscribe_stream(12, Stream.GAZE))
        self.assertEqual((answer.request_id, answer.word2, answer.word4), (12, 1, 0))
        image = await self.ask(subscribe_stream(13, Stream.IMAGE))
        self.assertEqual(image.word4, UNSUPPORTED_OPERATION)
        self.service.publish(gaze_message(908279389020), 5_000_000)
        streamed = await self.receive()
        self.assertEqual((streamed.kind, streamed.command), (STREAM_DATA_KIND, GAZE_STREAM))
        self.assertEqual(gaze.decode_gaze_message(streamed.payload).timestamp, 4_999_000)
        await self.ask(unsubscribe_stream(14, Stream.GAZE))
        self.service.publish(gaze_message(908279389021), 5_030_000)
        with self.assertRaises(asyncio.TimeoutError):
            await self.receive(timeout=0.2)

    async def test_detach_closes_the_port_and_connections(self):
        port = self.service.port_bound
        await self.service.detach()
        self.assertEqual(await asyncio.wait_for(self.reader.read(10), 1.0), b"")
        with self.assertRaises(OSError):
            await asyncio.open_connection("127.0.0.1", port)


if __name__ == "__main__":
    unittest.main()
