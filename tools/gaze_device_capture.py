#!/usr/bin/env python3
"""Timed live gaze capture from an Eye Tracker 5 for device-side comparison with Windows.

Runs the reconstructed startup (upgrade, unlock_ppu, realm 0x2712 with hardware
info, as the runtime's head-pose start does), logs every notification the
device sends, reads a few settings with getters, then records stream 0x500
while a person follows a fixed target schedule. The schedule runs twice: once
with gaze and presence only, once with the image stream subscribed as well,
the way the Windows runtime keeps it subscribed for head pose. Nothing is
written to the device; see docs/gaze-device.md.

Built-in schedule per phase (seconds after the 'phase' record): 0-2 settle,
then centre, top-left, top-right, bottom-right, bottom-left for --dwell seconds
each (17 s per phase at the default 3 s). With --seconds the phase records
continuously instead, for alignment with an external target log by wall time.
"""
import argparse
import json
from pathlib import Path
import statistics
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[1]/'src'))
from tobii_linux import driver  # noqa: E402
from tobii_linux import ttp_commands as tc  # noqa: E402
from tobii_linux.frames import stream_columns  # noqa: E402
from tobii_linux.ttp_fields import value  # noqa: E402
from tobii_linux.usb import USBContext  # noqa: E402
from tobii_linux.usb_channel import USBChannel  # noqa: E402

TARGETS = ('centre', 'top-left', 'top-right', 'bottom-right', 'bottom-left')
SETTLE_S = 2.0
GETTERS = (tc.Command.GET_DISPLAY_AREA, tc.Command.GET_FREQUENCIES, tc.Command.GET_ENABLED_EYE,
           tc.Command.GET_DEVICE_STATUS)
LEFT_VALIDITY, RIGHT_VALIDITY, COMBINED_VALIDITY = 7, 13, 27
LEFT_ORIGIN, RIGHT_ORIGIN, COMBINED_POINT = 2, 8, 28
PHASE_STREAMS = {'gaze': (tc.Stream.GAZE, tc.Stream.PRESENCE),
                 'gaze+image': (tc.Stream.GAZE, tc.Stream.PRESENCE, tc.Stream.IMAGE)}
PHASES = tuple(PHASE_STREAMS)
EYE_VALID = 0
COMBINED_VALID = 1


def emit(kind, **values):
    print(json.dumps({'kind': kind, 'time': time.time(), **values}, default=repr), flush=True)


def log_notification(message):
    """Session observer: logs notifications wherever they arrive, during calls or reads."""
    if message.kind == tc.NOTIFICATION_KIND:
        emit('notification', word1=message.request_id, word2=message.word2,
             command=message.command, word4=message.word4, payload=message.payload.hex())


def gaze_summary(record):
    columns = {c.column_id: [value(f) for f in c.value] for c in stream_columns(record)}

    def scalar(column):
        return columns[column][0] if column in columns else None

    def vector(column):
        return tuple(columns[column][1:]) if column in columns else None
    return {'left_valid': scalar(LEFT_VALIDITY) == EYE_VALID,
            'right_valid': scalar(RIGHT_VALIDITY) == EYE_VALID,
            'combined_valid': scalar(COMBINED_VALIDITY) == COMBINED_VALID,
            'left_origin': vector(LEFT_ORIGIN), 'right_origin': vector(RIGHT_ORIGIN),
            'combined_point': vector(COMBINED_POINT)}


def median_point(points):
    points = [p for p in points if p]
    return [round(statistics.median(p[i] for p in points), 1) for i in range(len(points[0]))] if points else None


def segment_report(phase, target, samples):
    count = len(samples)
    both = sum(s['left_valid'] and s['right_valid'] for s in samples)
    return {'phase': phase, 'target': target, 'records': count,
            'left_valid': sum(s['left_valid'] for s in samples),
            'right_valid': sum(s['right_valid'] for s in samples),
            'both_valid': both, 'combined_valid': sum(s['combined_valid'] for s in samples),
            'left_origin_median': median_point([s['left_origin'] for s in samples if s['left_valid']]),
            'right_origin_median': median_point([s['right_origin'] for s in samples if s['right_valid']]),
            'combined_point_median': median_point([s['combined_point'] for s in samples if s['combined_valid']])}


def drain(session, until, on_gaze, keep_images=False):
    pending, session.pending_streams = session.pending_streams, []
    while True:
        for message in pending:
            if message.kind != tc.STREAM_DATA_KIND:
                continue
            image = message.command in (tc.wire_stream_id(tc.Stream.IMAGE),)
            emit('message', command=message.command, word2=message.word2, word4=message.word4,
                 length=len(message.payload), payload=None if image and not keep_images else message.payload.hex())
            if message.command == tc.wire_stream_id(tc.Stream.GAZE) and message.word4 == 0:
                on_gaze(gaze_summary(message.payload))
        if time.monotonic() >= until:
            return
        data = session.channel.read(timeout_ms=100)
        pending = session.decoder.feed(data) if data else []


def run_phase(session, phase, streams, dwell, reports, seconds=None):
    subscribed = []
    for stream in streams:
        try:
            session.call(lambda rid: tc.subscribe_stream(rid, stream))
            subscribed.append(stream)
        except driver.DeviceError as error:
            emit('subscribe_failed', phase=phase, stream=int(stream), error=str(error))
    emit('phase', phase=phase, streams=[int(s) for s in subscribed])
    start = time.monotonic()
    if seconds is not None:
        samples = []
        drain(session, start + seconds, samples.append)
        reports.append(segment_report(phase, 'continuous', samples))
        emit('segment', **reports[-1])
        for stream in subscribed:
            session.call(lambda rid: tc.unsubscribe_stream(rid, stream))
        return
    drain(session, start + SETTLE_S, lambda sample: None)
    for index, target in enumerate(TARGETS):
        emit('target', phase=phase, target=target, seconds=dwell,
             offset_s=round(time.monotonic() - start, 3))
        samples = []
        drain(session, start + SETTLE_S + (index + 1)*dwell, samples.append)
        report = segment_report(phase, target, samples)
        reports.append(report)
        emit('segment', **report)
    for stream in subscribed:
        session.call(lambda rid: tc.unsubscribe_stream(rid, stream))


def run(args):
    with USBContext() as context:
        devices = context.probe()
        if not devices:
            raise RuntimeError('No Eye Tracker 5 visible to libusb')
        device = devices[args.index]
        with context.open_transport(bus=device['bus'], address=device['address']) as transport:
            with USBChannel(transport, args.channel) as channel:
                session = driver.TrackerSession(channel)
                session.observe(log_notification)
                session.initialize()
                session.camera_calibration()
                emit('step', name='started')
                for command in GETTERS:
                    try:
                        reply = session.call(lambda rid: tc.header_only_request(rid, command))
                        emit('getter', command=command.name, values=session.reply_values(reply))
                    except driver.DeviceError as error:
                        emit('getter_failed', command=command.name, error=str(error))
                reports = []
                for phase in PHASES if args.phase == 'both' else (args.phase,):
                    run_phase(session, phase, PHASE_STREAMS[phase], args.dwell, reports, args.seconds)
                emit('summary', segments=reports)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--dwell', type=float, default=3.0, help='seconds per target')
    parser.add_argument('--phase', choices=PHASES + ('both',), default='both')
    parser.add_argument('--seconds', type=float,
                        help='record continuously this long per phase instead of the built-in schedule')
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
