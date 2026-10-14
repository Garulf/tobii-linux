import io
import json
import socket
import threading
import time
import unittest

from tobii_linux.tet_tcp import (READ_COMMANDS, UNSUPPORTED_OPERATION, RawLog, Router,
                                 TetTcpServer, subscribed_stream)
from tobii_linux.ttp import Decoder, Message, protocol_upgrade
from tobii_linux.ttp_commands import (Command, RESPONSE_KIND, STREAM_DATA_KIND, request,
                                      set_output_frequency, subscribe_stream, uint32_field,
                                      unsubscribe_stream)

GAZE_STREAM = 0x500


class FakeTracker:
    """Answers every request with status 0 and streams gaze while subscribed."""
    def __init__(self):
        self.decoder = Decoder()
        self.received = []
        self.on_message = None
        self.streaming = threading.Event()
        self.stopping = threading.Event()
        threading.Thread(target=self._stream, daemon=True).start()

    def send(self, data):
        for message in self.decoder.feed(data):
            self.received.append(message)
            if message.command == Command.SUBSCRIBE_STREAM:
                self.streaming.set()
            if message.command == Command.UNSUBSCRIBE_STREAM:
                self.streaming.clear()
            payload = b'\0\0' + uint32_field(message.command).encode()
            self.on_message(Message(RESPONSE_KIND, message.request_id, 0, message.command, 0, payload))

    def _stream(self):
        while not self.stopping.is_set():
            if self.streaming.wait(0.05):
                self.on_message(Message(STREAM_DATA_KIND, 0, 0, GAZE_STREAM, 0, b'\0\0'))
                time.sleep(0.01)


class Client:
    def __init__(self, address):
        self.sock = socket.create_connection(address, timeout=2)
        self.decoder = Decoder()
        self.messages = []

    def send(self, message):
        self.sock.sendall(message.encode())

    def receive_until(self, predicate, timeout_s=2.0):
        deadline = time.monotonic() + timeout_s
        while time.monotonic() < deadline:
            found = [m for m in self.messages if predicate(m)]
            if found:
                return found
            self.sock.settimeout(max(0.01, deadline - time.monotonic()))
            try:
                data = self.sock.recv(65536)
            except socket.timeout:
                break
            if not data:
                break
            self.messages.extend(self.decoder.feed(data))
        return [m for m in self.messages if predicate(m)]

    def close(self):
        self.sock.close()


class TetTcpProxyTest(unittest.TestCase):
    def setUp(self):
        self.output = io.StringIO()
        self.log = RawLog(stream=self.output)
        self.tracker = FakeTracker()
        self.router = Router(self.tracker, self.log)
        self.tracker.on_message = self.router.from_device
        self.server = TetTcpServer(self.router, self.log, ports={0: 4455}, udp_ports=()).start()
        self.client = Client(self.server.addresses[0])

    def tearDown(self):
        self.client.close()
        self.server.stop()
        self.tracker.stopping.set()

    def records(self):
        return [json.loads(line) for line in self.output.getvalue().splitlines()]

    def test_request_is_forwarded_with_a_proxy_id_and_answered_with_the_original_id(self):
        self.client.send(protocol_upgrade(7))
        replies = self.client.receive_until(lambda m: m.kind == RESPONSE_KIND)
        self.assertEqual([(r.request_id, r.command, r.word4) for r in replies], [(7, 1000, 0)])
        self.assertNotEqual(self.tracker.received[0].request_id, 7)
        self.assertEqual(self.tracker.received[0].payload, protocol_upgrade(7).payload)

    def test_write_requests_never_reach_the_tracker(self):
        self.client.send(set_output_frequency(3, 33.0))
        replies = self.client.receive_until(lambda m: m.kind == RESPONSE_KIND)
        self.assertEqual([(r.request_id, r.command, r.word4) for r in replies],
                         [(3, Command.SET_OUTPUT_FREQUENCY, UNSUPPORTED_OPERATION)])
        self.assertEqual(self.tracker.received, [])
        self.assertIn('blocked', [r.get('note') for r in self.records()])

    def test_subscribed_stream_reaches_the_client_and_is_unsubscribed_when_it_disconnects(self):
        self.client.send(subscribe_stream(4, 1))
        data = self.client.receive_until(lambda m: m.kind == STREAM_DATA_KIND)
        self.assertTrue(data)
        self.assertEqual(data[0].command, GAZE_STREAM)
        self.client.close()
        deadline = time.monotonic() + 2
        while time.monotonic() < deadline and self.tracker.streaming.is_set():
            time.sleep(0.02)
        self.assertFalse(self.tracker.streaming.is_set())
        last = self.tracker.received[-1]
        self.assertEqual((last.command, subscribed_stream(last)), (Command.UNSUBSCRIBE_STREAM, GAZE_STREAM))

    def test_a_second_client_shares_the_subscription_and_keeps_it_when_the_first_leaves(self):
        second = Client(self.server.addresses[0])
        try:
            self.client.send(subscribe_stream(1, 1))
            self.assertTrue(self.client.receive_until(lambda m: m.kind == STREAM_DATA_KIND))
            second.send(subscribe_stream(1, 1))
            self.assertTrue(second.receive_until(lambda m: m.kind == STREAM_DATA_KIND))
            self.client.send(unsubscribe_stream(2, 1))
            self.assertTrue(self.client.receive_until(
                lambda m: m.kind == RESPONSE_KIND and m.command == Command.UNSUBSCRIBE_STREAM))
            subscriptions = [m.command for m in self.tracker.received]
            self.assertEqual(subscriptions, [Command.SUBSCRIBE_STREAM])
            self.assertTrue(self.tracker.streaming.is_set())
        finally:
            second.close()

    def test_unlock_mode_answers_the_dll_upgrade_from_the_cached_reply(self):
        router = Router(self.tracker, self.log, unlock=True)
        self.tracker.on_message = router.from_device
        router.prepare()
        commands = [m.command for m in self.tracker.received]
        self.assertEqual(commands, [Command.PROTOCOL_UPGRADE, Command.UNLOCK_PPU])
        client = Client(TetTcpServer(router, self.log, ports={0: 4455}, udp_ports=()).start().addresses[0])
        client.send(protocol_upgrade(9))
        replies = client.receive_until(lambda m: m.kind == RESPONSE_KIND)
        client.close()
        self.assertEqual([(r.request_id, r.command) for r in replies], [(9, Command.PROTOCOL_UPGRADE)])
        self.assertEqual(len(self.tracker.received), 2)


class PolicyTest(unittest.TestCase):
    def test_policy_contains_no_known_write_command(self):
        writes = {Command.SET_OUTPUT_FREQUENCY, Command.SET_FREQUENCIES, Command.SET_ILLUMINATION_MODE,
                  Command.POWER_SAVE_ACTIVATE, Command.POWER_SAVE_DEACTIVATE, Command.PAUSE_DEVICE}
        self.assertFalse(writes & READ_COMMANDS)

    def test_subscribed_stream_reads_the_wire_id(self):
        self.assertEqual(subscribed_stream(unsubscribe_stream(1, 1)), GAZE_STREAM)
        self.assertIsNone(subscribed_stream(request(1, Command.SUBSCRIBE_STREAM)))


if __name__ == '__main__':
    unittest.main()
