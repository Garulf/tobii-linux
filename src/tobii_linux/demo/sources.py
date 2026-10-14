"""Where the demo's data comes from: a live tracker over USB, or recorded logs replayed at their own pace."""
from dataclasses import replace
import json
from pathlib import Path
import statistics
import threading
import time

from ..supervisor import Backoff
from ..driver import CameraCalibration, NoTrackerError, usb_session
from .. import gaze
from ..frames import ImageFrame
from ..ttp import ProtocolError
from ..ttp_commands import Command, Stream, header_only_request

DEVICE_INFO_FIELDS = ('serial_number', 'model', 'generation', 'firmware_version')
NO_TRACKER_HINT = ("On WSL attach it with 'usbipd attach --wsl --busid <id>'; "
                   'on Linux check the cable and the udev rule.')
MAX_REPLAY_GAP_S = 0.5


def printable(item):
    if isinstance(item, (bytes, bytearray)):
        return bytes(item).decode('utf-8', 'replace')
    return item if isinstance(item, (int, float, str)) else repr(item)


def device_info(session):
    """GET_DEVICE_INFO (1420) reply values, named in the order the reply carries them."""
    values = [printable(v) for v in session.reply_values(
        session.call(lambda rid: header_only_request(rid, Command.GET_DEVICE_INFO)))]
    info = dict(zip(DEVICE_INFO_FIELDS, values))
    info.update({f'value_{index}': v for index, v in enumerate(values[len(DEVICE_INFO_FIELDS):],
                                                                len(DEVICE_INFO_FIELDS))})
    return info


class SourceThread:
    """Runs `run(stop_event)` on a thread; stop() asks it to finish and join() waits for it."""
    name = 'source'

    def __init__(self):
        self.stop_event = threading.Event()
        self.thread = None

    def start(self):
        self.thread = threading.Thread(target=self.run, args=(self.stop_event,), name=f'demo-{self.name}',
                                       daemon=True)
        self.thread.start()

    def stop(self):
        self.stop_event.set()

    def join(self, timeout_s=None):
        if self.thread is not None:
            self.thread.join(timeout_s)
        return self.thread is None or not self.thread.is_alive()


class LiveSource(SourceThread):
    """One USB session subscribed to gaze, images and presence; starts a new one after any failure.

The endpoint is read on its own thread (streams.drained) so slow browser
publishing never stalls the tracker; a silent session, a restarted tracker
or a host resume ends the session, and the next attempt follows a growing
delay (supervisor.Backoff).
"""
    name = 'live'

    def __init__(self, pipeline, index=0, channel=0, backoff=None):
        super().__init__()
        self.pipeline = pipeline
        self.index, self.channel = index, channel
        self.reader = None
        self.backoff = backoff or Backoff()

    def stop(self):
        super().stop()
        reader = self.reader
        if reader is not None:
            reader.stop()

    def run(self, stop):
        while not stop.is_set():
            started = time.monotonic()
            try:
                self.pipeline.set_connection('connecting', 'Starting the tracker')
                with usb_session(self.index, self.channel) as session:
                    self.session(session, stop)
            except NoTrackerError as error:
                self.pipeline.set_connection('no-device', f'{error}. {NO_TRACKER_HINT}')
            except Exception as error:
                message = f'{type(error).__name__}: {error}'
                self.pipeline.report_error(message)
                self.pipeline.set_connection('error', message)
                self.pipeline.on_session_lost()
            if not stop.is_set():
                stop.wait(self.backoff.delay(time.monotonic() - started))
        self.pipeline.set_connection('stopped', 'Stopped')

    def session(self, session, stop):
        from ..streams import StreamReader, SuspendWatch, drained
        info = device_info(session)
        calibration = session.camera_calibration()
        device = session.usb_device
        self.pipeline.on_session(calibration, info, {
            'bus': device.get('bus'), 'address': device.get('address'), 'channel': self.channel,
            'camera_translation': list(calibration.translation),
            'camera_angles': list(calibration.angles)})
        reader = StreamReader(session, (Stream.GAZE, Stream.IMAGE, Stream.PRESENCE),
                              optional=(Stream.PRESENCE,), suspend_watch=SuspendWatch())
        self.reader = self.pipeline.reader = reader
        if stop.is_set():
            return
        with reader:
            if Stream.PRESENCE in reader.subscribed:
                self.pipeline.on_presence_subscribed()
            self.pipeline.set_connection('streaming', 'Streaming gaze and images')
            self.dispatch(drained(reader.items()))
        self.pipeline.set_connection('stopping', 'Unsubscribed the tracker streams')

    def dispatch(self, items):
        handlers = {Stream.IMAGE: self.pipeline.on_frame, Stream.GAZE: self.pipeline.on_gaze,
                    Stream.PRESENCE: self.pipeline.on_presence}
        for item in items:
            handlers[item.stream](item.value)


def recorded_frames(log_path, frames_dir=None):
    """Calibration and (timestamp, width, height, path) rows from a linux_session_probe.py log."""
    log_path = Path(log_path)
    frames_dir = Path(frames_dir) if frames_dir is not None else log_path.parent/'frames'
    calibration, frames = None, []
    for line in log_path.read_text().splitlines():
        if not line.strip():
            continue
        row = json.loads(line)
        if row.get('kind') == 'step' and row.get('name') == 'calibration' and 'translation' in row:
            calibration = CameraCalibration(tuple(row['translation']), tuple(row['angles']))
        elif row.get('kind') == 'frame':
            name = Path(row['path'].replace('\\', '/')).name
            frames.append((int(row['timestamp']), int(row['width']), int(row['height']), frames_dir/name))
    if not frames:
        raise ValueError(f'{log_path} has no frame rows')
    return calibration, frames


def recorded_gaze(log_path):
    """gaze.GazeFrame of every recorded gaze message the runtime would keep, and how many it drops."""
    messages = [message for _, message in gaze.recorded_gaze_messages(log_path)]
    frames = []
    for message in messages:
        try:
            device = gaze.gaze_message(message)
        except ProtocolError:
            device = None
        if device is not None:
            frames.append(gaze.gaze_frame(device))
    if not frames:
        raise ValueError(f'{log_path} has no usable gaze messages')
    return frames, len(messages) - len(frames)


def replay_offsets_s(timestamps):
    """Seconds from the first item to each item, with long recording gaps shortened."""
    offsets = [0.0]
    for previous, current in zip(timestamps, timestamps[1:]):
        offsets.append(offsets[-1] + min(max(current - previous, 0) / 1e6, MAX_REPLAY_GAP_S))
    return offsets


def typical_interval_s(timestamps):
    intervals = [b - a for a, b in zip(timestamps, timestamps[1:]) if b > a]
    return statistics.median(intervals) / 1e6 if intervals else 0.03


def play(items, timestamp_of, emit, stop, shift):
    """Emit `items` in a loop at their recorded timing, shifting timestamps so they keep increasing."""
    timestamps = [timestamp_of(item) for item in items]
    offsets = replay_offsets_s(timestamps)
    loop_s = offsets[-1] + typical_interval_s(timestamps)
    loop_shift_us = timestamps[-1] - timestamps[0] + int(typical_interval_s(timestamps) * 1e6)
    loop = 0
    while not stop.is_set():
        start = time.monotonic()
        for item, offset in zip(items, offsets):
            if stop.wait(max(0.0, start + offset - time.monotonic())):
                return
            emit(shift(item, loop * loop_shift_us))
        if stop.wait(max(0.0, start + loop_s - time.monotonic())):
            return
        loop += 1


class ReplaySource(SourceThread):
    """Recorded camera frames and gaze messages, each looped at its recorded rate."""
    name = 'replay'

    def __init__(self, pipeline, frames_log=None, frames_dir=None, gaze_log=None):
        super().__init__()
        if frames_log is None and gaze_log is None:
            raise ValueError('Replay needs a frames log, a gaze log or both')
        self.pipeline = pipeline
        self.calibration, self.frames, self.gaze, skipped = None, [], [], 0
        details = {}
        if frames_log is not None:
            self.calibration, rows = recorded_frames(frames_log, frames_dir)
            self.frames = [ImageFrame(timestamp, 8, width, height, path.read_bytes())
                           for timestamp, width, height, path in rows]
            details.update(frames_log=str(frames_log), frames=len(self.frames))
        if gaze_log is not None:
            self.gaze, skipped = recorded_gaze(gaze_log)
            details.update(gaze_log=str(gaze_log), gaze_records=len(self.gaze), gaze_skipped=skipped)
        self.details = details

    def run(self, stop):
        self.pipeline.on_session(self.calibration, {'model': 'Recorded session', 'serial_number': 'replay'},
                                 self.details)
        self.pipeline.set_connection('streaming', 'Replaying recorded data')
        players = []
        if self.frames:
            players.append(threading.Thread(target=play, name='demo-replay-frames', daemon=True, args=(
                self.frames, lambda f: f.timestamp, self.pipeline.on_frame, stop,
                lambda f, offset: replace(f, timestamp=f.timestamp + offset))))
        if self.gaze:
            players.append(threading.Thread(target=play, name='demo-replay-gaze', daemon=True, args=(
                self.gaze, lambda frame: frame.timestamp_us, self.pipeline.on_gaze, stop,
                lambda frame, offset: frame.with_timestamp(frame.timestamp_us + offset))))
        for player in players:
            player.start()
        for player in players:
            player.join()
        self.pipeline.set_connection('stopped', 'Stopped')
