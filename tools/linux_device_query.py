#!/usr/bin/env python3
"""Read-only live query of Eye Tracker 5 settings over TTP, as JSON lines.

Runs the normal startup, then sends parameterless get/enumerate requests and
logs each reply's decoded values and raw payload. Only getters are sent.
"""
import argparse
import json
from pathlib import Path
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[1]/'src'))
from tobii_linux import driver  # noqa: E402
from tobii_linux.ttp_commands import Command, header_only_request  # noqa: E402
from tobii_linux.usb import USBContext  # noqa: E402
from tobii_linux.usb_channel import USBChannel  # noqa: E402

QUERIES = (
    Command.GET_DEVICE_INFO, Command.GET_DEVICE_NAME, Command.GET_DEVICE_STATUS,
    Command.GET_DISPLAY_AREA, Command.GET_TRACK_BOX, Command.GET_GEOMETRY_MOUNTING,
    Command.GET_ENABLED_EYE, Command.GET_LOW_BLINK_MODE, Command.GET_ILLUMINATION_MODE,
    Command.ENUMERATE_ILLUMINATION_MODES, Command.GET_OUTPUT_FREQUENCY,
    Command.ENUMERATE_OUTPUT_FREQUENCIES, Command.GET_FREQUENCIES, Command.ENUMERATE_FREQUENCIES,
    Command.GET_CAPABILITIES, Command.GET_CAPABILITIES_ALL, Command.GET_DEVICE_PROPERTIES,
    Command.ENUMERATE_EXTENSIONS, Command.ENUMERATE_ENABLED_EXTENSIONS, Command.ENUMERATE_STREAM_TYPES,
)


def emit(kind, **values):
    print(json.dumps({'kind': kind, 'time': time.time(), **values}, default=repr), flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--channel', type=int, default=0)
    parser.add_argument('--index', type=int, default=0)
    args = parser.parse_args()
    with USBContext() as context:
        device = context.probe()[args.index]
        with context.open_transport(bus=device['bus'], address=device['address']) as transport:
            with USBChannel(transport, args.channel) as channel:
                session = driver.TrackerSession(channel)
                session.initialize()
                session.camera_calibration()
                for command in QUERIES:
                    try:
                        reply = session.call(lambda rid: header_only_request(rid, command))
                        emit('reply', command=command.name, values=session.reply_values(reply),
                             payload=reply.payload.hex())
                    except driver.DeviceError as error:
                        emit('failed', command=command.name, error=str(error))
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
