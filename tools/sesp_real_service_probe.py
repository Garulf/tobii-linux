#!/usr/bin/env python3
"""Bounded, read-only SESP client probe against the real Tobii software on Windows.

Reports whether `\\\\.\\pipe\\streamengineservices` exists and which process owns
it, then acts as the game DLL's sesp_services client (docs/sesp.md): hello on
the command (0x2711) and subscription (0x2712) channels, one initialize request
per --device-url (the subscription slot as subscription id), head-pose start,
--seconds of subscription traffic, head-pose stop, close. Every pipe message is
recorded with local QueryPerformanceCounter time. It sends nothing that changes
the service: no profile, firmware-upgrade or statistics requests.

Output is JSONL on stdout. Requires 64-bit Windows Python with src/ on sys.path.
"""
import argparse
import ctypes as C
import ctypes.wintypes as W
import os
from pathlib import Path
import struct
import subprocess
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parent))
sys.path.insert(0, str(Path(__file__).resolve().parent.parent/"src"))
import prp_real_service_probe as pipes  # noqa: E402
from tobii_linux import sesp  # noqa: E402

DEFAULT_URLS = ("tet-tcp://127.0.0.1", "")


def pipe_owner(handle):
    if not pipes.kernel32:
        return None
    query = pipes.bind("GetNamedPipeServerProcessId", [W.HANDLE, C.POINTER(W.ULONG)])
    pid = W.ULONG()
    if not query(handle, C.byref(pid)):
        return None
    listing = subprocess.run(["tasklist", "/fo", "csv", "/nh", "/fi", f"PID eq {pid.value}"],
                             capture_output=True, text=True).stdout.strip()
    return {"pid": pid.value, "process": listing}


class SespChannel(pipes.Channel):
    def handshake(self):
        """Hello, the service opens the reply pipe, then one u32: its slot index."""
        self.write(sesp.encode_hello(self.channel, self.reply_name))
        ok, error, _ = pipes.wait_overlapped(self.reply, self.connect_overlapped, self.timeout_ms,
                                             "reply pipe connect")
        if not ok and error != pipes.ERROR_PIPE_CONNECTED:
            raise pipes.PipeError(f"reply pipe connect error {error}")
        slot = self.read_message(self.timeout_ms)
        pipes.emit("slot", label=self.label, size=len(slot),
                   value=struct.unpack("<I", slot)[0] if len(slot) == 4 else None)
        return struct.unpack("<I", slot)[0]


class MessageReader:
    def __init__(self, channel):
        self.channel, self.decoder = channel, sesp.FrameDecoder()

    def messages(self, timeout_s):
        deadline = time.monotonic() + timeout_s
        while time.monotonic() < deadline:
            try:
                received_at, data = self.channel.messages.get(timeout=0.05)
            except Exception:
                continue
            for buffer in self.decoder.feed(data):
                yield received_at, buffer

    def response(self, sequence, timeout_s=3.0):
        for _, buffer in self.messages(timeout_s):
            message = sesp.decode(buffer)
            describe(self.channel.label, message, buffer)
            if message.sequence == sequence:
                return message
        raise pipes.PipeError(f"no response {sequence} on {self.channel.label}")


def describe(label, message, buffer, received_at=None):
    values = {"sequence": message.sequence, "type": message.kind, "hex": buffer.hex()}
    if message.kind == sesp.RESPONSE_INITIALIZE:
        error, flags = sesp.capabilities(message)
        values.update(error=error, capabilities=sorted(flags))
    elif message.kind == sesp.SUBSCRIPTION_HEADPOSE:
        pose = sesp.head_pose(message)
        values.update(timestamp_us=pose.timestamp_us, position=pose.position,
                      rotation=pose.rotation, position_validity=pose.position_validity,
                      rotation_validity=pose.rotation_validity, received_qpc_us=received_at)
    elif message.payload is not None:
        values.update(error=message.payload.scalar(0, "I"))
    pipes.emit("sesp", label=label, **values)


def prp_urls():
    return [f"tobii-prp://{name[len('TOBIIPRP-'):]}" for name in pipes.list_prp_pipes()]


def run(args):
    listed = sorted(n for n in os.listdir(pipes.PIPE_ROOT) if n.lower() == sesp.PIPE_NAME)
    pipes.emit("pipe_listing", present=bool(listed), prp_urls=prp_urls())
    command = SespChannel("command", sesp.URL, sesp.COMMAND_CHANNEL, 3000)
    subscription = SespChannel("subscription", sesp.URL, sesp.SUBSCRIPTION_CHANNEL, 3000)
    channels = []
    try:
        for channel in (command, subscription):
            channel.open()
            channels.append(channel)
            pipes.emit("owner", label=channel.label, **(pipe_owner(channel.server) or {}))
        command.handshake()
        slot = subscription.handshake()
        command.start_reader()
        subscription.start_reader()
        replies = MessageReader(command)
        sequences = iter(range(1, 1 << 31))
        for url in args.device_url:
            sequence = next(sequences)
            command.write(sesp.message(sequence, sesp.REQUEST_INITIALIZE,
                                       [sesp.Scalar(1, "I", slot)] if slot else [],
                                       [(0, url.encode())]))
            replies.response(sequence)
        sequence = next(sequences)
        command.write(sesp.message(sequence, sesp.REQUEST_HEADPOSE_START, []))
        replies.response(sequence)
        for received_at, buffer in MessageReader(subscription).messages(args.seconds):
            describe("subscription", sesp.decode(buffer), buffer, received_at)
        sequence = next(sequences)
        command.write(sesp.message(sequence, sesp.REQUEST_HEADPOSE_STOP, []))
        replies.response(sequence)
    finally:
        for channel in channels:
            channel.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--device-url", action="append",
                        help="URL sent in an initialize request (repeatable); default: "
                             f"{', '.join(repr(u) for u in DEFAULT_URLS)} and every listed "
                             "tobii-prp:// device")
    parser.add_argument("--seconds", type=float, default=3.0)
    args = parser.parse_args()
    args.device_url = args.device_url or list(DEFAULT_URLS) + prp_urls()
    try:
        run(args)
    except (pipes.PipeError, sesp.SespError) as error:
        pipes.emit("error", error=str(error))
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
