import argparse
import contextlib
import importlib.util
import io
import json
from pathlib import Path
import struct
import tempfile
import threading
import time
import unittest
import urllib.request

from tobii_linux import calibration as cal
from tobii_linux import calibration_page
from tobii_linux import gaze
from tobii_linux import gaze_device as gd
from tobii_linux import traffic_log
from tobii_linux import ttp_commands as tc
from tobii_linux.driver import DeviceError, TrackerSession
from tobii_linux.usb import USBError
from tobii_linux.usb_channel import USBChannel
from tobii_linux.gaze_device import GazeCommand
from tobii_linux.ttp import Decoder, Message, fields
from tobii_linux.ttp_fields import encode_payload, struct_field, triple_fixed42_fields, uint32_field, value

# A scripted device that follows the calibration command semantics recovered
# from the runtime and the Tobii client (docs/gaze-calibration.md), NOT device traffic.
STATE_ERROR = 0x20000508
OPERATION_FAILED = 0x20000407
TOOLS = Path(__file__).resolve().parents[1]/'tools'


def make_blob(points, calibration_id):
    header = struct.pack('<5I', 110, 84, 1, 0, calibration_id) + b'human'.ljust(64, b'\0')
    table = b''.join(gd.CALIBRATION_RECORD.pack(x, y, x + 0.01, y, 1, x, y - 0.01, 1) for x, y in points)
    trailer_offset = 4 + len(header)
    return struct.pack('<I', trailer_offset) + header + struct.pack('<Ii', 0x5A5A5A5A, len(points)) + table


def stimulus(points):
    return [gd.StimulusPoint((x, y, 0.0), gd.EyeQuality(2, 0.0125, 0.03125), gd.EyeQuality(2, 0.015625, 0.0625))
            for x, y in points]


class CalibrationDevice:
    def __init__(self, calibration_id=0xBC381A37, points=((0.5, 0.5),), fail_compute_at=None,
                 fail_collect_at=None, ignore_apply=False, gaze_during_collect=None):
        self.calibration_id = calibration_id
        self.blob = make_blob(points, calibration_id)
        self.calibrating = False
        self.collected = []
        self.computes = 0
        self.collects = 0
        self.fail_compute_at = fail_compute_at
        self.fail_collect_at = fail_collect_at
        self.ignore_apply = ignore_apply
        self.gaze_during_collect = gaze_during_collect or (lambda number, point: [])
        self.requests = Decoder()
        self.outgoing = []
        self.log = []

    def reply(self, request, items=(), word4=0):
        payload = encode_payload(list(items)) if items else b''
        self.outgoing.append(Message(tc.RESPONSE_KIND, request.request_id, 0, request.command, word4, payload).encode())

    def point_of(self, request):
        values = [value(f) for f in fields(request.payload)]
        return tuple(values[:2]), values[2]

    def write(self, data):
        for request in self.requests.feed(data):
            self.log.append(request)
            self.handle(request)

    def handle(self, request):
        command = request.command
        if command == tc.Command.GET_DEVICE_STATUS:
            self.reply(request, (struct_field(2, gd.STATUS_STRUCT), uint32_field(gd.STATUS_CALIBRATION_ID),
                                 tc.string_field(str(self.calibration_id))))
        elif command == tc.Command.GET_DISPLAY_AREA:
            self.reply(request, triple_fixed42_fields((-296.625, 326.0, 111.8125))
                       + triple_fixed42_fields((300.375, 326.0, 111.8125))
                       + triple_fixed42_fields((-296.625, 10.25, -3.125)))
        elif command == GazeCommand.CALIBRATION_RETRIEVE:
            self.reply(request, (tc.blob_field(self.blob),))
        elif command == GazeCommand.GET_CALIBRATION_STIMULUS_POINTS:
            points = [r.point for r in gd.calibration_points(self.blob)]
            self.reply(request, gd.stimulus_points_payload(stimulus(points)))
        elif command == GazeCommand.CALIBRATION_START:
            self.reply(request, word4=STATE_ERROR if self.calibrating else 0)
            self.calibrating = True
        elif command == GazeCommand.CALIBRATION_STOP:
            self.reply(request, word4=0 if self.calibrating else STATE_ERROR)
            self.calibrating = False
        elif command == GazeCommand.CALIBRATION_CLEAR:
            self.collected = []
            self.reply(request)
        elif command == GazeCommand.CALIBRATION_COLLECT_2D:
            self.collect(request)
        elif command == GazeCommand.CALIBRATION_DISCARD_2D:
            point, _ = self.point_of(request)
            self.collected.remove(point)
            self.reply(request)
        elif command == GazeCommand.CALIBRATION_COMPUTE_AND_APPLY:
            self.computes += 1
            if self.computes == self.fail_compute_at or not self.calibrating:
                self.reply(request, word4=OPERATION_FAILED)
                return
            self.calibration_id = (self.calibration_id * 31 + len(self.collected)) & 0xFFFFFFFF
            self.blob = make_blob(self.collected, self.calibration_id)
            self.reply(request)
        elif command == GazeCommand.CALIBRATION_APPLY:
            blob = value(fields(request.payload)[0])
            if self.calibrating:
                self.collected += [r.point for r in gd.calibration_points(blob)]
            elif not self.ignore_apply:
                self.blob, self.calibration_id = blob, gd.blob_calibration_id(blob)
            self.reply(request)
        else:
            self.reply(request)

    def collect(self, request):
        self.collects += 1
        if not self.calibrating or self.collects == self.fail_collect_at:
            self.reply(request, word4=STATE_ERROR if not self.calibrating else OPERATION_FAILED)
            return
        point, eye = self.point_of(request)
        self.collected.append(point)
        for message in self.gaze_during_collect(self.collects, point):
            self.outgoing.append(message.encode())
        self.reply(request)

    def read(self, timeout_ms):
        return self.outgoing.pop(0) if self.outgoing else b''

    def commands(self):
        return [m.command for m in self.log]


class StallingDevice(CalibrationDevice):
    """Streams gaze while subscribed and goes silent for good once it is left unread too long.

Models the live run of 2026-10-14: after the UI paused reading for 2.1 s the
tracker sent nothing more in that USB session, neither stream data nor replies.
"""
    def __init__(self, unread_limit_s=0.3, **kwargs):
        super().__init__(**kwargs)
        self.unread_limit_s = unread_limit_s
        self.subscribed = False
        self.wedged = False
        self.last_read = time.monotonic()
        self.last_gaze = 0.0

    def handle(self, request):
        if request.command == SUBSCRIBE:
            self.subscribed = True
        elif request.command == UNSUBSCRIBE:
            self.subscribed = False
        if not self.wedged:
            super().handle(request)

    def read(self, timeout_ms):
        now = time.monotonic()
        if self.subscribed and now - self.last_read > self.unread_limit_s:
            self.wedged = True
        self.last_read = now
        if self.wedged:
            time.sleep(timeout_ms / 1000)
            return b''
        if self.subscribed and now - self.last_gaze > 0.03:
            self.last_gaze = now
            self.outgoing += [m.encode() for m in gaze_at([now * 1000], (0.5, 0.5))]
        if not self.outgoing:
            time.sleep(min(timeout_ms, 10) / 1000)
        return super().read(timeout_ms)


class SlowUI(cal.CalibrationUI):
    def __init__(self, seconds):
        self.seconds = seconds

    def show_point(self, index, x, y):
        time.sleep(self.seconds)


class RecordingUI(cal.CalibrationUI):
    def __init__(self, interrupt_at=None):
        self.events = []
        self.interrupt_at = interrupt_at

    def show_point(self, index, x, y):
        self.events.append(('show', index))
        if index == self.interrupt_at:
            raise KeyboardInterrupt

    def point_done(self, index, accepted):
        self.events.append(('done', index, accepted))

    def stage_done(self, stage):
        self.events.append(('stage', stage))


START, STOP, CLEAR = GazeCommand.CALIBRATION_START, GazeCommand.CALIBRATION_STOP, GazeCommand.CALIBRATION_CLEAR
COLLECT, DISCARD = GazeCommand.CALIBRATION_COLLECT_2D, GazeCommand.CALIBRATION_DISCARD_2D
COMPUTE, APPLY = GazeCommand.CALIBRATION_COMPUTE_AND_APPLY, GazeCommand.CALIBRATION_APPLY
RETRIEVE, STIMULUS = GazeCommand.CALIBRATION_RETRIEVE, GazeCommand.GET_CALIBRATION_STIMULUS_POINTS
STATUS = tc.Command.GET_DEVICE_STATUS
SUBSCRIBE, UNSUBSCRIBE = tc.Command.SUBSCRIBE_STREAM, tc.Command.UNSUBSCRIBE_STREAM
WRITES = {START, STOP, CLEAR, COLLECT, DISCARD, COMPUTE, APPLY, GazeCommand.CALIBRATION_START_EYE,
          GazeCommand.CALIBRATION_COLLECT_3D, GazeCommand.CALIBRATION_COLLECT_PER_EYE_2D,
          GazeCommand.CALIBRATION_COMPUTE_AND_APPLY_PER_EYE, GazeCommand.CALIBRATION_DISCARD_3D,
          GazeCommand.SET_DISPLAY_AREA, GazeCommand.PERSISTENT_FILE_WRITE, GazeCommand.PERSISTENT_FILE_ERASE,
          GazeCommand.SET_ENABLED_EYE, SUBSCRIBE, UNSUBSCRIBE}
FULL_SEQUENCE = [SUBSCRIBE, START, CLEAR, COLLECT, COMPUTE, COLLECT, COLLECT, COLLECT, COMPUTE,
                 COLLECT, COLLECT, COLLECT, COMPUTE, STOP, UNSUBSCRIBE, RETRIEVE, STATUS, STIMULUS]


def gaze_at(timestamps_ms, point):
    return [gaze.gaze_stream_message({1: gaze.int64_column(int(t * 1000)), 7: gaze.uint32_column(0),
                                      13: gaze.uint32_column(0), 5: gaze.vec2_column(point),
                                      11: gaze.vec2_column(point)}) for t in timestamps_ms]


class BackupRestoreTests(unittest.TestCase):
    def test_backup_is_a_calibration_setting_and_verifies(self):
        device = CalibrationDevice(points=((0.5, 0.5), (0.1, 0.9)))
        session = TrackerSession(device)
        backup = cal.backup_calibration(session)
        self.assertEqual(device.commands(), [STATUS, RETRIEVE, STATUS])
        with tempfile.TemporaryDirectory() as directory:
            path = cal.save_backup(Path(directory)/'calibration.setpm', backup)
            data = path.read_bytes()
            self.assertEqual(struct.unpack_from('<4I', data), (4, 1, len(data) - 12, 0xBC381A37))
            self.assertEqual(data[16:], device.blob)
            self.assertTrue(cal.verify_backup(session, cal.load_backup(path)).ok)
            with self.assertRaises(FileExistsError):
                cal.save_backup(path, backup)
        self.assertFalse(set(device.commands()) & WRITES)

    def test_verify_reports_a_different_device_calibration(self):
        device = CalibrationDevice()
        old = cal.backup_calibration(TrackerSession(device))
        device.blob, device.calibration_id = make_blob([(0.9, 0.1)], 7), 7
        check = cal.verify_backup(TrackerSession(device), old)
        self.assertEqual((check.ids_match, check.blob_matches, check.ok), (False, False, False))

    def test_restore_applies_and_checks_the_id(self):
        device = CalibrationDevice(calibration_id=5)
        saved = gd.LocalCalibration(0xBC381A37, make_blob([(0.5, 0.5)], 0xBC381A37))
        self.assertEqual(cal.restore_calibration(TrackerSession(device), saved), 0xBC381A37)
        self.assertEqual(device.commands(), [APPLY, STATUS])
        self.assertEqual(device.blob, saved.blob)

    def test_restore_fails_when_the_device_keeps_its_id(self):
        device = CalibrationDevice(calibration_id=5, ignore_apply=True)
        saved = gd.LocalCalibration(0xBC381A37, make_blob([(0.5, 0.5)], 0xBC381A37))
        with self.assertRaises(cal.CalibrationError):
            cal.restore_calibration(TrackerSession(device), saved)


class CalibratorTests(unittest.TestCase):
    def test_sequence_targets_and_outcome(self):
        device = CalibrationDevice()
        ui = RecordingUI()
        outcome = cal.Calibrator(TrackerSession(device), ui).run()
        self.assertEqual(device.commands(), FULL_SEQUENCE)
        collects = [device.point_of(m) for m in device.log if m.command == COLLECT]
        expected = [tuple(struct.unpack('<f', struct.pack('<f', c))[0] for c in p) for p in cal.INTERACTIVE_POINTS]
        self.assertEqual([p for p, _ in collects], expected)
        self.assertEqual({eye for _, eye in collects}, {3})
        self.assertEqual(outcome.calibration, gd.LocalCalibration(device.calibration_id, device.blob))
        self.assertEqual(len(outcome.stimulus_points), 7)
        self.assertEqual([p.point for p in outcome.points], expected)
        self.assertEqual([e for e in ui.events if e[0] == 'stage'], [('stage', 0), ('stage', 1), ('stage', 2)])
        self.assertFalse(device.calibrating)

    def test_improve_reapplies_the_previous_calibration_after_clear(self):
        device = CalibrationDevice(points=[(0.5, 0.5)] * 7)
        previous = cal.backup_calibration(TrackerSession(device))
        device.log.clear()
        outcome = cal.Calibrator(TrackerSession(device), previous=previous).run()
        self.assertEqual(device.commands()[:4], [SUBSCRIBE, START, CLEAR, APPLY])
        self.assertEqual(len(outcome.points), 14)

    def test_compute_failure_stops_without_apply(self):
        device = CalibrationDevice(fail_compute_at=2)
        before = device.blob
        with self.assertRaises(cal.CalibrationFailed) as failure:
            cal.Calibrator(TrackerSession(device), RecordingUI(), restarts=0).run()
        self.assertEqual(failure.exception.step, 'compute stage 1 (1070)')
        self.assertEqual(device.commands(), FULL_SEQUENCE[:9] + [STOP, UNSUBSCRIBE])
        self.assertNotIn(APPLY, device.commands())
        self.assertFalse(device.calibrating)
        self.assertNotEqual(device.blob, before)

    def test_refused_compute_restarts_from_the_first_target(self):
        device = CalibrationDevice(fail_compute_at=1)
        ui = RecordingUI()
        calibrator = cal.Calibrator(TrackerSession(device), ui)
        outcome = calibrator.run()
        restart = [SUBSCRIBE, START, CLEAR, COLLECT, COMPUTE, STIMULUS, STOP, START, CLEAR]
        self.assertEqual(device.commands()[:9], restart)
        self.assertEqual(device.commands()[9:], FULL_SEQUENCE[3:])
        self.assertEqual([(stage, attempt) for stage, attempt, _ in calibrator.compute_failures], [(0, 0)])
        self.assertIn(('show', 0), ui.events[1:])
        self.assertEqual(len(outcome.points), 7)

    def test_compute_refused_every_time_gives_up(self):
        device = CalibrationDevice()
        original = device.handle

        def refuse(request):
            if request.command == COMPUTE:
                device.reply(request, word4=OPERATION_FAILED)
            else:
                original(request)
        device.handle = refuse
        with self.assertRaises(cal.CalibrationFailed):
            cal.Calibrator(TrackerSession(device), restarts=1).run()
        self.assertEqual(device.commands().count(COMPUTE), 2)
        self.assertEqual(device.commands()[-2:], [STOP, UNSUBSCRIBE])
        self.assertNotIn(APPLY, device.commands())

    def test_collect_failure_stops(self):
        device = CalibrationDevice(fail_collect_at=3)
        with self.assertRaises(cal.CalibrationFailed) as failure:
            cal.Calibrator(TrackerSession(device)).run()
        self.assertEqual(failure.exception.step, 'target 2')
        self.assertEqual(device.commands()[-3:], [COLLECT, STOP, UNSUBSCRIBE])
        self.assertNotIn(APPLY, device.commands())

    def test_start_failure_sends_no_stop(self):
        device = CalibrationDevice()
        device.calibrating = True
        with self.assertRaises(cal.CalibrationFailed):
            cal.Calibrator(TrackerSession(device)).run()
        self.assertEqual(device.commands(), [SUBSCRIBE, START, UNSUBSCRIBE])

    def test_interrupt_stops_and_propagates(self):
        device = CalibrationDevice()
        with self.assertRaises(KeyboardInterrupt):
            cal.Calibrator(TrackerSession(device), RecordingUI(interrupt_at=4)).run()
        self.assertEqual(device.commands()[-2:], [STOP, UNSUBSCRIBE])
        self.assertNotIn(APPLY, device.commands())

    def test_focus_change_discards_and_collects_again(self):
        targets = cal.INTERACTIVE_POINTS

        def gaze_during_collect(number, point):
            looking_at = targets[3] if number == 3 else point
            return gaze_at(range(1000 * number, 1000 * number + 600, 30), looking_at)
        device = CalibrationDevice(gaze_during_collect=gaze_during_collect)
        ui = RecordingUI()
        outcome = cal.Calibrator(TrackerSession(device), ui, focus=cal.FocusCheck((2560, 1440))).run()
        commands = device.commands()
        self.assertEqual(commands[:9], [SUBSCRIBE, START, CLEAR, COLLECT, COMPUTE, COLLECT, COLLECT, DISCARD, COLLECT])
        self.assertEqual(device.point_of(device.log[7])[0], device.point_of(device.log[6])[0])
        self.assertIn(('done', 2, False), ui.events)
        self.assertEqual(outcome.discarded, ((2, 1),))
        self.assertEqual(len(outcome.points), 7)

    def test_focus_lost_on_every_attempt_fails(self):
        device = CalibrationDevice(gaze_during_collect=lambda number, point: gaze_at((0, 600), point)
                                   if number > 1 else [])
        with self.assertRaises(cal.CalibrationFailed):
            cal.Calibrator(TrackerSession(device), focus=cal.FocusCheck(), max_attempts=2).run()
        self.assertEqual(device.commands()[5:10], [COLLECT, DISCARD, COLLECT, DISCARD, STOP])


class UnreadStreamTests(unittest.TestCase):
    def test_pausing_reads_while_gaze_is_subscribed_silences_the_device(self):
        device = StallingDevice()
        session = TrackerSession(device)
        session.call(lambda rid: tc.subscribe_stream(rid, tc.Stream.GAZE))
        time.sleep(0.5)
        with self.assertRaises(DeviceError):
            session.call(lambda rid: tc.header_only_request(rid, STATUS), timeout_s=0.3)

    def test_calibrator_keeps_reading_while_the_ui_blocks(self):
        device = StallingDevice(points=[(0.5, 0.5)])
        outcome = cal.Calibrator(TrackerSession(device), SlowUI(0.5), stages=((0,), (1,))).run()
        self.assertFalse(device.wedged)
        self.assertEqual(device.commands(), [SUBSCRIBE, START, CLEAR, COLLECT, COMPUTE, COLLECT, COMPUTE, STOP,
                                             UNSUBSCRIBE, RETRIEVE, STATUS, STIMULUS])
        self.assertEqual(len(outcome.points), 2)

    def test_collect_waits_for_a_still_binocular_gaze(self):
        device = StallingDevice(points=[(0.5, 0.5)])
        log = []
        cal.Calibrator(TrackerSession(device), stages=((0,),), wait_for_fixation=True,
                       log=lambda kind, **values: log.append((kind, values))).run()
        fixations = [values for kind, values in log if kind == 'fixation']
        self.assertEqual(len(fixations), 1)
        self.assertTrue(fixations[0]['stable'])
        self.assertGreaterEqual(fixations[0]['samples'], cal.FIXATION_MIN_SAMPLES)

    def test_ui_errors_are_raised_after_the_worker_ends(self):
        device = StallingDevice()
        with self.assertRaises(KeyboardInterrupt):
            cal.Calibrator(TrackerSession(device), RecordingUI(interrupt_at=0)).run()
        self.assertEqual(device.commands()[-2:], [STOP, UNSUBSCRIBE])
        self.assertFalse(device.wedged)


class DryRunTests(unittest.TestCase):
    def test_dry_run_reads_but_sends_no_write(self):
        device = CalibrationDevice()
        before = device.blob
        reports = []
        session = cal.DryRunSession(TrackerSession(device), report=reports.append)
        outcome = cal.Calibrator(session, RecordingUI()).run()
        self.assertEqual(device.commands(), [RETRIEVE, STATUS, STIMULUS])
        self.assertEqual([m.command for m in session.blocked], FULL_SEQUENCE[:15])
        self.assertEqual([r['command'] for r in reports], FULL_SEQUENCE[:15])
        self.assertEqual(reports[3]['fields'][2], 3)
        self.assertEqual(outcome.calibration.blob, before)

    def test_restore_dry_run_reports_the_blob(self):
        device = CalibrationDevice()
        reports = []
        saved = gd.LocalCalibration(9, make_blob([(0.5, 0.5)] * 3, 9))
        cal.DryRunSession(TrackerSession(device), report=reports.append).call(
            lambda rid: gd.calibration_apply(rid, saved.blob))
        self.assertEqual(device.log, [])
        self.assertEqual(reports[0]['fields'][0]['bytes'], len(saved.blob))

    def test_tool_calibrate_dry_run(self):
        spec = importlib.util.spec_from_file_location('gaze_calibrate', TOOLS/'gaze_calibrate.py')
        tool = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(tool)
        device = CalibrationDevice()
        args = argparse.Namespace(dry_run=True, ui='none', improve=False, before='unused-before',
                                  after='unused-after', overwrite=False)
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            status = tool.calibrate(TrackerSession(device), args)
        self.assertEqual(status, 0)
        self.assertFalse(set(device.commands()) & WRITES)
        records = [json.loads(line) for line in output.getvalue().splitlines()]
        self.assertEqual([r['command'] for r in records if r['kind'] == 'would_send'], FULL_SEQUENCE[:15])
        self.assertTrue(next(r for r in records if r['kind'] == 'dry_run')['device_unchanged'])
        self.assertFalse(Path('unused-before').exists())


class FixationTests(unittest.TestCase):
    def samples(self, points, step_ms=30):
        return [cal.GazeSample(i * step_ms * 1000, p) for i, p in enumerate(points)]

    def test_still_gaze_is_a_fixation(self):
        self.assertTrue(cal.fixation(self.samples([(0.2 + 0.001 * i, 0.3) for i in range(12)])))

    def test_saccade_in_the_window_is_not(self):
        points = [(0.2, 0.3)] * 8 + [(0.5, 0.3)] * 4
        self.assertFalse(cal.fixation(self.samples(points)))
        self.assertTrue(cal.fixation(self.samples(points + [(0.5, 0.3)] * 8)))

    def test_one_eye_missing_is_not(self):
        points = [(0.2, 0.3)] * 11 + [None]
        self.assertFalse(cal.fixation(self.samples(points)))

    def test_too_few_samples_is_not(self):
        self.assertFalse(cal.fixation(self.samples([(0.2, 0.3)] * 3)))


class FocusCheckTests(unittest.TestCase):
    targets = cal.INTERACTIVE_POINTS[1:4]

    def samples(self, entries):
        return [cal.GazeSample(int(t * 1000), p) for t, p in entries]

    def test_steady_gaze_keeps_focus(self):
        samples = self.samples((t, self.targets[0]) for t in range(0, 600, 30))
        self.assertFalse(cal.FocusCheck().focus_changed(samples, self.targets, 0))

    def test_short_glance_elsewhere_keeps_focus(self):
        entries = [(t, self.targets[0]) for t in range(0, 300, 30)] + [(300, self.targets[1]), (330, self.targets[1])]
        entries += [(t, self.targets[0]) for t in range(360, 600, 30)]
        self.assertFalse(cal.FocusCheck().focus_changed(self.samples(entries), self.targets, 0))

    def test_gaze_on_another_target_changes_focus(self):
        entries = [(0, self.targets[0])] + [(t, self.targets[2]) for t in range(30, 600, 30)]
        self.assertTrue(cal.FocusCheck().focus_changed(self.samples(entries), self.targets, 0))

    def test_missing_gaze_counts_as_leaving(self):
        entries = [(0, self.targets[0]), (100, None), (400, None), (430, self.targets[0])]
        self.assertTrue(cal.FocusCheck().focus_changed(self.samples(entries), self.targets, 0))

    def test_gaze_between_targets_is_ignored(self):
        entries = [(t, (0.3, 0.5)) for t in range(0, 200, 30)] + [(200, self.targets[0])]
        self.assertFalse(cal.FocusCheck().focus_changed(self.samples(entries), self.targets, 0))

    def test_no_samples_is_not_a_change(self):
        self.assertFalse(cal.FocusCheck().focus_changed([], self.targets, 0))

    def test_gaze_sample_averages_valid_eyes(self):
        message = gaze.gaze_stream_message({1: gaze.int64_column(5), 7: gaze.uint32_column(0),
                                            13: gaze.uint32_column(4), 5: gaze.vec2_column((0.25, 0.5)),
                                            11: gaze.vec2_column((0.75, 0.5))})
        self.assertEqual(cal.gaze_sample(message), cal.GazeSample(5, (0.25, 0.5)))


class TargetTests(unittest.TestCase):
    def test_display_of_albedo_uses_the_whole_screen(self):
        self.assertEqual(cal.calibration_area((597.0, 336.1)), (0.0, 0.0, 1.0, 1.0))
        self.assertEqual(cal.calibration_targets((597.0, 336.1))[1], (0.10000000149011612, 0.8999999761581421))

    def test_large_display_uses_a_centred_area(self):
        x, y, w, h = cal.calibration_area((800.0, 450.0))
        self.assertAlmostEqual(w, 0.75)
        self.assertAlmostEqual(h, 340.0 / 450.0)
        self.assertAlmostEqual(x, 0.125)
        self.assertEqual(cal.calibration_targets((800.0, 450.0))[0], (0.5, 0.5))

    def test_display_size_from_display_area(self):
        device = CalibrationDevice()
        width, height = cal.read_display_size(TrackerSession(device))
        self.assertAlmostEqual(width, 597.0)
        self.assertAlmostEqual(height, ((326.0 - 10.25) ** 2 + (111.8125 + 3.125) ** 2) ** 0.5, places=4)


class DeviceFormatTests(unittest.TestCase):
    def test_calibration_setting_round_trip(self):
        local = gd.LocalCalibration(0x1234, make_blob([(0.5, 0.5)], 0x1234))
        data = gd.calibration_setting(local)
        self.assertEqual(gd.parse_calibration_setting(data), local)
        self.assertEqual(gd.blob_calibration_id(local.blob), 0x1234)

    def test_calibration_points_statuses(self):
        blob = struct.pack('<I', 4) + struct.pack('<Ii', 0, 3) + b''.join(
            gd.CALIBRATION_RECORD.pack(0.5, 0.5, 0.4, 0.6, status, 0.6, 0.4, 1) for status in (-1, 0, 7))
        points = gd.calibration_points(blob)
        self.assertEqual([p.left_status for p in points], [gd.PointStatus.FAILED_OR_INVALID,
                                                           gd.PointStatus.VALID_BUT_NOT_USED,
                                                           gd.PointStatus.FAILED_OR_INVALID])
        self.assertEqual(points[0].left_mapping, (struct.unpack('<f', struct.pack('<f', 0.4))[0],
                                                  struct.unpack('<f', struct.pack('<f', 0.6))[0]))

    def test_calibration_points_rejects_a_table_past_the_end(self):
        with self.assertRaises(gd.ProtocolError):
            gd.calibration_points(struct.pack('<I', 4) + struct.pack('<Ii', 0, 2) + b'\0' * 40)

    def test_stimulus_points_round_trip_and_status_mapping(self):
        points = [gd.StimulusPoint((0.5, 0.25, 0.0), gd.EyeQuality(2, 0.015625, 0.03125), gd.EyeQuality(5, 0.5, 0.25))]
        values = [value(f) for f in gd.stimulus_points_payload(points)]
        parsed = gd.stimulus_points(values)
        self.assertEqual(parsed[0].left, gd.EyeQuality(2, 0.015625, 0.03125))
        self.assertEqual(parsed[0].right.status, gd.PointStatus.FAILED_OR_INVALID)
        self.assertEqual(parsed[0].point, (0.5, 0.25, 0.0))


class PageTests(unittest.TestCase):
    def test_page_acknowledges_targets(self):
        ui = calibration_page.PageCalibrationUI(port=0, settle_s=0).start()
        try:
            base = ui.url
            self.assertIn(b'requestFullscreen', urllib.request.urlopen(base).read())

            def post(report):
                request = urllib.request.Request(base + 'ack', data=json.dumps(report).encode(), method='POST')
                urllib.request.urlopen(request).read()

            def state():
                return json.loads(urllib.request.urlopen(base + 'state').read())

            post({'seq': state()['seq'], 'fullscreen': True, 'width': 2560, 'height': 1440})
            with self.assertRaises(TimeoutError):
                ui.wait_for_display(0.2)
            post({'seq': state()['seq'], 'fullscreen': True, 'started': True, 'width': 2560, 'height': 1440})
            self.assertEqual(ui.wait_for_display(1)['width'], 2560)
            shown = threading.Thread(target=ui.show_point, args=(3, 0.9, 0.9))
            shown.start()
            while state()['phase'] != 'point':
                pass
            current = state()
            self.assertEqual((current['index'], current['x'], current['y']), (3, 0.9, 0.9))
            post({'seq': current['seq'], 'fullscreen': True, 'width': 2560, 'height': 1440})
            shown.join(2)
            self.assertFalse(shown.is_alive())
            self.assertEqual(ui.screen_px, (2560, 1440))
            post({'seq': state()['seq'], 'aborted': True})
            with self.assertRaises(calibration_page.CalibrationAborted):
                ui.show_point(4, 0.1, 0.1)
        finally:
            ui.close()


class PageStartTests(unittest.TestCase):
    def test_a_kiosk_window_as_large_as_the_screen_counts_as_fullscreen(self):
        fills = calibration_page.fills_screen
        self.assertTrue(fills({'fullscreen': False, 'width': 1707, 'height': 960,
                               'screen_width': 1708, 'screen_height': 961}))
        self.assertFalse(fills({'fullscreen': False, 'width': 1280, 'height': 900,
                                'screen_width': 2560, 'screen_height': 1440}))
        self.assertFalse(fills(None))

    def test_page_started_by_its_countdown_without_the_fullscreen_flag(self):
        ui = calibration_page.PageCalibrationUI(port=0, settle_s=0).start()
        self.addCleanup(ui.close)
        page = urllib.request.urlopen(ui.url).read()
        self.assertIn(b'COUNTDOWN_S = 3', page)
        report = {'seq': 0, 'fullscreen': False, 'started': True, 'width': 2560, 'height': 1440,
                  'screen_width': 2560, 'screen_height': 1440}
        request = urllib.request.Request(ui.url + 'ack', data=json.dumps(report).encode(), method='POST')
        urllib.request.urlopen(request).read()
        self.assertEqual(ui.wait_for_display(1)['width'], 2560)


class FramedTransport:
    """Bulk transfers with the channel framing of usb_framing, for the traffic log."""
    def __init__(self, reads):
        self.reads = list(reads)
        self.written = []

    def control_out(self, request, value=0, timeout_ms=1000):
        return 0

    def write(self, data, timeout_ms=1000):
        self.written.append(bytes(data))
        return len(data)

    def read(self, size, timeout_ms=1000):
        item = self.reads.pop(0) if self.reads else USBError('bulk transfer', -7)
        if isinstance(item, USBError):
            raise item
        return item


class TrafficLogTests(unittest.TestCase):
    def test_log_records_transfers_and_replays_messages(self):
        reply = Message(tc.RESPONSE_KIND, 1, 0, 1490, 0, b'').encode()
        frame = struct.pack('<II', 1, len(reply) + 8) + reply
        transport = FramedTransport([USBError('bulk transfer', -7, 10, frame[:10]), frame[10:]])
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)/'raw.jsonl'
            log = traffic_log.TrafficLog(path)
            with USBChannel(traffic_log.LoggingTransport(transport, log)) as channel:
                session = TrackerSession(channel)
                session.call(lambda rid: tc.header_only_request(rid, STATUS))
            log.close()
            ops = [json.loads(line)['op'] for line in path.read_text().splitlines()]
            self.assertEqual(ops, ['start', 'control_out', 'write', 'read_error', 'read', 'control_out'])
            replayed = [(direction, m.command) for _, direction, m in traffic_log.replay_messages(path)]
            self.assertEqual(replayed, [('out', 1490), ('in', 1490)])


if __name__ == '__main__':
    unittest.main()
