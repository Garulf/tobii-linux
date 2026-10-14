#!/usr/bin/env python3
"""Bounded live capture of raw TTP stream messages from an Eye Tracker 5.

Runs startup and the head-pose realm like linux_session_probe.py, asks the
device for the column layout of each selected stream, subscribes them, and logs
every message for --seconds as JSON lines with raw payload bytes. Image
payloads are logged by length only unless --keep-images is given. No firmware,
calibration or settings commands are sent.
"""
import argparse
import json
from pathlib import Path
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[1]/'src'))
from tobii_linux import driver  # noqa: E402
from tobii_linux import ttp_commands as tc  # noqa: E402
from tobii_linux.usb import USBContext  # noqa: E402
from tobii_linux.usb_channel import USBChannel  # noqa: E402

IMAGE_STREAMS = {tc.Stream.IMAGE, tc.Stream.PRIMARY_CAMERA_IMAGE}


def emit(kind, **values):
    print(json.dumps({'kind': kind, 'time': time.time(), **values}, default=repr), flush=True)


def describe(session, stream):
    try:
        reply = session.call(lambda rid: tc.enumerate_stream_type_columns(rid, stream))
        emit('columns', stream=int(stream), values=session.reply_values(reply), payload=reply.payload.hex())
    except driver.DeviceError as error:
        emit('columns', stream=int(stream), error=str(error))


def capture(session, streams, seconds, keep_images):
    wire_ids = {tc.wire_stream_id(s): s for s in streams}
    subscribed = []
    for stream in streams:
        try:
            session.call(lambda rid: tc.subscribe_stream(rid, stream))
            subscribed.append(stream)
            emit('subscribed', stream=int(stream))
        except driver.DeviceError as error:
            emit('subscribe_failed', stream=int(stream), error=str(error))
    deadline = time.monotonic() + seconds
    pending, session.pending_streams = session.pending_streams, []
    while time.monotonic() < deadline:
        for message in pending:
            stream = wire_ids.get(message.command)
            image = stream in IMAGE_STREAMS
            emit('message', message_kind=message.kind, command=message.command, stream=stream,
                 word2=message.word2, word4=message.word4, length=len(message.payload),
                 payload=None if image and not keep_images else message.payload.hex())
        data = session.channel.read(timeout_ms=200)
        pending = session.decoder.feed(data) if data else []
    for stream in subscribed:
        session.call(lambda rid: tc.unsubscribe_stream(rid, stream))
        emit('unsubscribed', stream=int(stream))


def run(args):
    streams = [tc.Stream[name.upper()] for name in args.streams]
    with USBContext() as context:
        devices = context.probe()
        if not devices:
            raise RuntimeError('No Eye Tracker 5 visible to libusb')
        device = devices[args.index]
        with context.open_transport(bus=device['bus'], address=device['address']) as transport:
            with USBChannel(transport, args.channel) as channel:
                session = driver.TrackerSession(channel)
                session.initialize()
                session.camera_calibration()
                emit('step', name='started')
                for stream in streams:
                    describe(session, stream)
                capture(session, streams, args.seconds, args.keep_images)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--streams', nargs='+', default=['gaze', 'presence'],
                        choices=[s.name.lower() for s in tc.Stream])
    parser.add_argument('--seconds', type=float, default=10.0)
    parser.add_argument('--channel', type=int, default=0)
    parser.add_argument('--index', type=int, default=0)
    parser.add_argument('--keep-images', action='store_true')
    args = parser.parse_args()
    try:
        run(args)
        return 0
    except Exception as error:
        emit('error', type=type(error).__name__, message=str(error))
        return 1


if __name__ == '__main__':
    raise SystemExit(main())
