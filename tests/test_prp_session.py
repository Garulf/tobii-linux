"""PRP session codec tests.

Hex vectors below were produced by emulating the game DLL's original serializer
(tools/prp_session_oracle.py); they are not captured live traffic.
"""
import struct
import unittest

from tobii_linux.prp import GazePoint, HeadPose, ProtocolError, encode_stream_packet
from tobii_linux.prp_session import (
    COMMAND_CHANNEL, COMMAND_CONNECT, COMMAND_STREAM_START, COMMAND_STREAM_STOP,
    COMMAND_TIMESYNC, PIPE_CHUNK, ClientHello, ClientSession, CommandResponse,
    ConnectRequest, ConnectResponse, HandshakeInfo, check_handshake_ack, client_pipe_name,
    decode_client_hello, decode_client_pid, decode_command_request, decode_command_response,
    decode_handshake_info, encode_client_hello, encode_client_pid, encode_command_response,
    encode_connect_request, encode_handshake_ack, encode_handshake_info,
    encode_stream_request, encode_timesync_request, minimal_connect_response, pipe_chunks,
    server_pipe_names, split_packets)

STREAM_START_HEAD = bytes.fromhex(
    "545052502c000000785052500c00000009000000090000000400000008000000"
    "030000000400000008000000020000000400000000000000")
TIMESYNC_REQUEST = bytes.fromhex(
    "54505250280000007c5052500c00000005000000090000000400000008000000"
    "1600000004000000040000000400000000000000")
STREAM_STOP_RESPONSE = bytes.fromhex(
    "5450525030000000645052500c000000050000000a000000040000000c000000"
    "05000000040000000400000008000000020000000400000000000000")
TIMESYNC_RESPONSE = bytes.fromhex(
    "5450525034000000605052500c000000060000000a000000040000000c000000"
    "0000000016000000040000000c00000003020100070605040400000000000000")
EMPTY_CONNECT_REQUEST = bytes.fromhex(
    "5450525064000000305052500c000000ffffff7f090000000400000008000000"
    "010000000400000040000000f9ffffff00000000040000000000000000000000"
    "0400000011000000000000000600000000000000000000000000000000000000"
    "01000000020000000400000000000000")
HANDSHAKE_INFO = (struct.pack("<I", 7) + b"tobii-prp://linux-bridge_subscription\0"
                  + b"tobii-prp://linux-bridge_timesync\0")


class HandshakeTests(unittest.TestCase):
    def test_pipe_names(self):
        self.assertEqual(server_pipe_names("tobii-prp://ET5"), ("TOBIIPRP-ET5", "ET5"))
        self.assertEqual(client_pipe_name("TOBIIPRP-ET5", 0x1A2B, 3),
                         "client_TOBIIPRP-ET5_00001a2b_0000000000000003")
        for bad in ("tobii-prp://", "tprp-tcp://x", "ET5"):
            with self.assertRaises(ProtocolError):
                server_pipe_names(bad)

    def test_client_hello_round_trip_and_rejection(self):
        hello = ClientHello(COMMAND_CHANNEL, "client_TOBIIPRP-ET5_00000010_0000000000000001")
        wire = encode_client_hello(hello)
        self.assertEqual(wire[:8], struct.pack("<II", 1, 0x4E21))
        self.assertEqual(decode_client_hello(wire), hello)
        for bad in (wire[:8], wire[:-1], struct.pack("<II", 0, 0x4E21) + b"x\0"):
            with self.assertRaises(ProtocolError):
                decode_client_hello(bad)

    def test_ack_pid_and_info(self):
        check_handshake_ack(encode_handshake_ack())
        for bad in (b"\x01\0\0\0\0", b"\x02\0\0\0", b""):
            with self.assertRaises(ProtocolError):
                check_handshake_ack(bad)
        self.assertEqual(decode_client_pid(encode_client_pid(4321)), 4321)
        info = HandshakeInfo(7, "tobii-prp://linux-bridge_subscription",
                             "tobii-prp://linux-bridge_timesync")
        self.assertEqual(encode_handshake_info(info), HANDSHAKE_INFO)
        self.assertEqual(decode_handshake_info(HANDSHAKE_INFO), info)
        with self.assertRaises(ProtocolError):
            decode_handshake_info(HANDSHAKE_INFO[:-1])
        with self.assertRaises(ProtocolError):
            encode_handshake_info(HandshakeInfo(1, "a" * 200, "b" * 60))


class FramingTests(unittest.TestCase):
    def test_chunking_and_reassembly(self):
        head = encode_stream_packet(HeadPose(5, True, (1, 2, 3), (True, True, True), (0, 0, 0)),
                                    message_tag=0)
        gaze = encode_stream_packet(GazePoint(6, True, 0.5, 0.5), message_tag=0)
        stream = head + gaze
        packets, tail = split_packets(stream[:-3])
        self.assertEqual((packets, tail), ([head], gaze[:-3]))
        self.assertEqual(split_packets(stream), ([head, gaze], b""))
        big = bytes(PIPE_CHUNK + 5)
        self.assertEqual([len(c) for c in pipe_chunks(big)], [PIPE_CHUNK, 5])
        with self.assertRaises(ProtocolError):
            split_packets(b"XPRP" + stream[4:])


class CommandTests(unittest.TestCase):
    def test_emulated_request_vectors(self):
        self.assertEqual(encode_stream_request(9, COMMAND_STREAM_START, 2), STREAM_START_HEAD)
        self.assertEqual(encode_timesync_request(5), TIMESYNC_REQUEST)
        empty = ConnectRequest(-7, (), b"", (4, 17, 0, 6), 1, b"", b"", 2)
        self.assertEqual(encode_connect_request(0x7FFFFFFF, empty), EMPTY_CONNECT_REQUEST)
        decoded = decode_command_request(STREAM_START_HEAD)
        self.assertEqual((decoded.tag, decoded.command, decoded.stream), (9, 3, 2))
        self.assertEqual(decode_command_request(EMPTY_CONNECT_REQUEST).connect, empty)

    def test_connect_request_round_trip(self):
        request = ConnectRequest(3, (b"license-key-one", b"", b"k2"), bytes(range(1, 71)),
                                 (4, 17, 0, 6), 0x1234, "C:\\Games\\".encode("utf-16-le"),
                                 "Game".encode("utf-16-le"), 1)
        for extended in (True, False):
            wire = encode_connect_request(1, request, extended=extended)
            expected = request if extended else ConnectRequest(**{**request.__dict__,
                                                                  "field_of_use": 0})
            self.assertEqual(decode_command_request(wire, extended=extended).connect, expected)

    def test_emulated_response_vectors(self):
        self.assertEqual(encode_command_response(
            CommandResponse(5, COMMAND_STREAM_STOP, 5, 2)), STREAM_STOP_RESPONSE)
        self.assertEqual(encode_command_response(
            CommandResponse(6, COMMAND_TIMESYNC, 0, 0x0001020304050607)), TIMESYNC_RESPONSE)
        self.assertEqual(decode_command_response(TIMESYNC_RESPONSE).value, 0x0001020304050607)
        self.assertEqual(decode_command_response(STREAM_STOP_RESPONSE).status, 5)

    def test_connect_response_round_trip_and_limits(self):
        response = ConnectResponse(
            "1.2.3.4-test", False, (1, 2), (3,), (), (-4, 5, 6), (2, 3, 29), (0, 0x20, 7),
            ((0, (1, 2, 3)), (4, ())), ("a", "b", "c", "d", "e", "x", "y"), tuple(range(9)),
            (0, 1, 10), 3, (8, 9), ("e0", "e1", "", "e3"))
        message = CommandResponse(1, COMMAND_CONNECT, 0, connect=response)
        self.assertEqual(decode_command_response(encode_command_response(message)), message)
        minimal = minimal_connect_response("bridge", 2)
        decoded = decode_command_response(encode_command_response(
            CommandResponse(2, COMMAND_CONNECT, 0, connect=minimal))).connect
        self.assertEqual((decoded.streams, decoded.license_results,
                          decoded.firmware_upgrade_in_progress), ((2, 3, 29), (0, 0), False))
        for bad in (ConnectResponse(streams=tuple(range(0x21))),
                    ConnectResponse(supported_commands=(0x21,)),
                    ConnectResponse(groups=((5, ()),)),
                    ConnectResponse(build_version="x" * 0x100)):
            with self.assertRaises(ProtocolError):
                encode_command_response(CommandResponse(1, COMMAND_CONNECT, 0, connect=bad))

    def test_malformed_messages(self):
        with self.assertRaises(ProtocolError):
            decode_command_request(TIMESYNC_RESPONSE)
        with self.assertRaises(ProtocolError):
            decode_command_response(STREAM_START_HEAD)
        truncated = bytearray(STREAM_START_HEAD)
        struct.pack_into("<I", truncated, 12 + 24, 0x40)
        with self.assertRaises(ProtocolError):
            decode_command_request(bytes(truncated))
        with self.assertRaises(ProtocolError):
            encode_stream_request(1, COMMAND_TIMESYNC, 2)

    def test_session_tags(self):
        session = ClientSession()
        self.assertEqual([session.next_command_tag() for _ in range(2)], [1, 2])
        self.assertEqual(session.next_timesync_tag(), 1)


if __name__ == "__main__":
    unittest.main()
