#!/usr/bin/env python3
"""Bounded live Linux session probe for the Eye Tracker 5, logging JSON lines.

Runs the reconstructed startup one step at a time and stops at --until:
upgrade, unlock, calibration or frames. Every request and reply is logged with
raw bytes so a failed step can be analysed offline. Frames are saved as raw
grayscale files under --frames-dir. No firmware, calibration or settings
commands are sent.
"""
import argparse
import json
from pathlib import Path
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[1]/'src'))
from tobii_linux import driver  # noqa: E402
from tobii_linux.usb import USBContext  # noqa: E402
from tobii_linux.usb_channel import USBChannel  # noqa: E402

STEPS = ('upgrade', 'unlock', 'calibration', 'frames')


def emit(kind, **values):
    print(json.dumps({'kind': kind, 'time': time.time(), **values}, default=repr), flush=True)


class LoggingChannel:
    def __init__(self, channel):
        self.channel = channel

    def write(self, data):
        emit('write', bytes=data.hex())
        self.channel.write(data)

    def read(self, timeout_ms):
        data = self.channel.read(timeout_ms=timeout_ms)
        if data:
            emit('read', bytes=data[:4096].hex(), length=len(data))
        return data


def run(args):
    with USBContext() as context:
        devices = context.probe()
        emit('devices', devices=devices)
        if not devices:
            raise RuntimeError('No Eye Tracker 5 visible to libusb')
        device = devices[args.index]
        with context.open_transport(bus=device['bus'], address=device['address']) as transport:
            emit('transport', bus=device['bus'], address=device['address'], channel=args.channel)
            with USBChannel(transport, args.channel) as channel:
                session = driver.TrackerSession(LoggingChannel(channel))
                session.call(driver.protocol_upgrade, timeout_s=args.timeout)
                emit('step', name='upgrade', ok=True)
                if args.until == 'upgrade':
                    return
                session.initialize()
                emit('step', name='unlock', ok=True)
                if args.until == 'unlock':
                    return
                calibration = session.camera_calibration()
                emit('step', name='calibration', ok=True, translation=calibration.translation,
                     angles=calibration.angles)
                if args.until == 'calibration':
                    return
                session.subscribe_images()
                emit('step', name='subscribe', ok=True)
                directory = Path(args.frames_dir)
                directory.mkdir(parents=True, exist_ok=True)
                for count, frame in enumerate(session.frames(timeout_s=args.timeout)):
                    path = directory/f'frame-{count:04d}-{frame.width}x{frame.height}.gray'
                    path.write_bytes(frame.pixels)
                    emit('frame', timestamp=frame.timestamp, width=frame.width, height=frame.height, path=str(path))
                    if count + 1 >= args.frames:
                        break
                session.call(lambda rid: driver.unsubscribe_stream(rid, driver.Stream.IMAGE))
                emit('step', name='unsubscribe', ok=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--until', choices=STEPS, default='upgrade')
    parser.add_argument('--channel', type=int, default=0)
    parser.add_argument('--index', type=int, default=0)
    parser.add_argument('--timeout', type=float, default=3.0)
    parser.add_argument('--frames', type=int, default=30)
    parser.add_argument('--frames-dir', default='frames')
    args = parser.parse_args()
    try:
        run(args)
        return 0
    except Exception as error:
        emit('error', type=type(error).__name__, message=str(error))
        return 1


if __name__ == '__main__':
    raise SystemExit(main())
