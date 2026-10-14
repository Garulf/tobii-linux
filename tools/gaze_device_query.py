#!/usr/bin/env python3
"""Read-only live query of the Eye Tracker 5 calibration state, as JSON lines.

Runs the normal startup, then reads the device status (calibration id, key 7),
the calibration blob (1100), the calibration stimulus points (1120) and the
persistent file list (3060). With --local it compares the device calibration
with a calibration.setpm copy the way start_synchronize_calibrations does,
without applying anything. Blobs are logged by length and SHA-256 only.
"""
import argparse
import hashlib
import json
from pathlib import Path
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[1]/'src'))
from tobii_linux import driver  # noqa: E402
from tobii_linux import gaze_device as gd  # noqa: E402
from tobii_linux import ttp_commands as tc  # noqa: E402
from tobii_linux.usb import USBContext  # noqa: E402
from tobii_linux.usb_channel import USBChannel  # noqa: E402


def emit(kind, **values):
    print(json.dumps({'kind': kind, 'time': time.time(), **values}, default=repr), flush=True)


def digest(data):
    return {'length': len(data), 'sha256': hashlib.sha256(data).hexdigest(), 'head': data[:96].hex()}


def describe(values):
    return [digest(bytes(v)) if isinstance(v, (bytes, bytearray)) and len(v) > 256 else v for v in values]


def query(session, name, build):
    try:
        values = session.reply_values(session.call(build))
        emit('reply', command=name, values=describe(values))
        return values
    except driver.DeviceError as error:
        emit('failed', command=name, error=str(error))
        return None


def run(args):
    local = gd.parse_calibration_setting(Path(args.local).read_bytes()) if args.local else None
    if local:
        emit('local_calibration', calibration_id=local.calibration_id, blob=digest(local.blob))
    with USBContext() as context:
        device = context.probe()[args.index]
        with context.open_transport(bus=device['bus'], address=device['address']) as transport:
            with USBChannel(transport, args.channel) as channel:
                session = driver.TrackerSession(channel)
                session.initialize()
                session.camera_calibration()
                status = query(session, 'GET_DEVICE_STATUS',
                               lambda rid: tc.header_only_request(rid, tc.Command.GET_DEVICE_STATUS))
                device_id = gd.device_calibration_id(status) if status else None
                emit('device_calibration_id', calibration_id=device_id)
                blob = query(session, 'CALIBRATION_RETRIEVE',
                             lambda rid: gd.header_only(rid, gd.GazeCommand.CALIBRATION_RETRIEVE))
                query(session, 'GET_CALIBRATION_STIMULUS_POINTS',
                      lambda rid: gd.header_only(rid, gd.GazeCommand.GET_CALIBRATION_STIMULUS_POINTS))
                query(session, 'PERSISTENT_FILE_ENUMERATE',
                      lambda rid: gd.header_only(rid, gd.GazeCommand.PERSISTENT_FILE_ENUMERATE))
                if local:
                    retrieved = next((bytes(v) for v in blob or () if isinstance(v, (bytes, bytearray))), None)
                    emit('comparison', ids_equal=device_id == local.calibration_id,
                         blob_equal=retrieved == local.blob,
                         synchronize_decision=gd.synchronize_calibration(session, local).name)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--local', help='calibration.setpm to compare with (read only)')
    parser.add_argument('--channel', type=int, default=0)
    parser.add_argument('--index', type=int, default=0)
    args = parser.parse_args()
    try:
        run(args)
        return 0
    except Exception as error:
        emit('error', type=type(error).__name__, message=str(error))
        return 1


if __name__ == '__main__':
    raise SystemExit(main())
