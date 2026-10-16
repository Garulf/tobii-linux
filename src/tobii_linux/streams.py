"""Several subscribed TTP streams read from one TrackerSession and split by stream.

TrackerSession.frames() keeps only image frames. StreamReader subscribes any
set of streams and decodes images, gaze (dropping a message exactly where the
runtime does) and presence; other stream messages pass through undecoded. The
reader, its silence and suspend checks and the device clock watch are
crates/tobii-device/src/streams.rs. GazeAndImageStreams adds the threading and
clock mapping a live service needs, and serve_tracker runs one whole service
session on it.

The tracker stops sending for good (stream data and replies) when its endpoint
is not read for about a second while streams are subscribed, so every reader
here keeps reading on its own thread whatever its consumer does, and a session
that falls silent is treated as dead rather than waited on.
"""
from collections import Counter, deque
from dataclasses import dataclass, replace
import logging
import threading

from . import gaze
from ._native import device as _device
from .driver import ClockMapper, DeviceError, head_pose_processor, newest_frames
from .frames import ImageFrame, image_frame
from .ttp_commands import Stream, wire_stream_id

log = logging.getLogger('tobii_linux.streams')
READ_SLICE_MS = 100
READER_EXIT_TIMEOUT_S = 2.0
SILENCE_TIMEOUT_S = 3.0
SUSPEND_GAP_S = 2.0
DEVICE_CLOCK_REWIND_US = 1_000_000
DRAIN_DEPTH = 256

suspended_time_s = _device.suspended_time_s
# SuspendWatch(suspended_time=suspended_time_s, threshold_s=SUSPEND_GAP_S).resumed() is true
# when the host was suspended since the last check (the tracker loses power then).
SuspendWatch = _device.SuspendWatch
# DeviceClockWatch(rewind_us).observe(device_us) raises DeviceError when the tracker restarted.
DeviceClockWatch = _device.DeviceClockWatch
# A threading.Event the native reading loops check without calling into Python.
StopEvent = _device.StopEvent


@dataclass(frozen=True)
class Presence:
    """One presence stream (0x504) record: device timestamp and the device's presence value."""
    timestamp_us: int
    value: int


def gaze_frame(message):
    device = gaze.gaze_message(message)
    return gaze.gaze_frame(device) if device is not None else None


def presence(message):
    record = gaze.presence_message(message.payload)
    return Presence(*record) if record is not None else None


DECODERS = {Stream.IMAGE: image_frame, Stream.GAZE: gaze_frame, Stream.PRESENCE: presence}


@dataclass(frozen=True)
class StreamItem:
    stream: int
    value: object
    message: object = None


def _value(kind, value, message):
    if kind == 'image':
        return ImageFrame(*value)
    if kind == 'gaze':
        return gaze.frame_from_native(value)
    if kind == 'presence':
        return Presence(*value)
    return message


def _counter(counts):
    return Counter({Stream(stream): count for stream, count in counts})


class StreamReader:
    """Reads stream data from a started session; stop() may be called from another thread.

Streams listed in `optional` may refuse the subscription; they are then left
out and listed in `refused`. With `suspend_watch` (SuspendWatch), items()
ends the session with DeviceError when the host has been suspended, since
the tracker has to be started again afterwards. Setting `stop_event`
(threading.Event) has the same effect as stop().
"""
    def __init__(self, session, streams=(Stream.GAZE, Stream.IMAGE), optional=(), suspend_watch=None,
                 stop_event=None):
        self.session = session
        self.streams = tuple(Stream(s) for s in streams)
        self.optional = frozenset(Stream(s) for s in optional)
        self.by_wire_id = {wire_stream_id(s): s for s in self.streams}
        self.native = _device.StreamReader([int(s) for s in self.streams], [int(s) for s in self.optional])
        self.stop_event = stop_event if stop_event is not None else StopEvent()
        self.suspend_watch = suspend_watch

    @property
    def subscribed(self):
        return [Stream(stream) for stream in self.native.subscribed]

    @property
    def refused(self):
        return {Stream(stream): error for stream, error in self.native.refused}

    @property
    def received(self):
        return _counter(self.native.received)

    @property
    def dropped(self):
        return _counter(self.native.dropped)

    @property
    def decode_errors(self):
        return _counter(self.native.decode_errors)

    @property
    def unexpected(self):
        return self.native.unexpected

    @property
    def last_error(self):
        return self.native.last_error

    def subscribe(self):
        try:
            self.native.subscribe(self.session.native)
        finally:
            self.session._collect()

    def unsubscribe(self):
        """Unsubscribe every subscribed stream; errors are collected so each stream gets its request.

A dead session is not asked: its device no longer answers.
"""
        try:
            self.native.unsubscribe(self.session.native)
        except (DeviceError, OSError, RuntimeError):
            self.session.pending_streams = []
            raise
        finally:
            self.session.native.take_pending()
        self.session.pending_streams = []

    @property
    def stopping(self):
        return self.stop_event.is_set()

    def stop(self):
        self.stop_event.set()

    def __enter__(self):
        self.subscribe()
        return self

    def __exit__(self, exc_type, *_):
        self.unsubscribe_after(exc_type)

    def unsubscribe_after(self, exc_type):
        """Unsubscribe; when an error is already propagating, a failed unsubscribe does not replace it."""
        try:
            self.unsubscribe()
        except (DeviceError, OSError, RuntimeError):
            if exc_type is None:
                raise

    def _item(self, item):
        stream, kind, value, message = item
        return StreamItem(Stream(stream), _value(kind, value, message), message)

    def decode(self, message):
        """The StreamItem for one stream message, or None when it is not ours, dropped or malformed."""
        item = self.native.decode(message)
        return self._item(item) if item is not None else None

    def items(self, silence_timeout_s=SILENCE_TIMEOUT_S):
        """Decoded items as they arrive until stop() is called.

Raises DeviceError, marking the session dead, when no stream data arrives for
`silence_timeout_s` or the host was suspended.
"""
        session = self.session
        pending, session.pending_streams = session.pending_streams, []
        pump = _device.ItemPump(pending, silence_timeout_s)
        if session.native_loops:
            while (item := pump.next(self.native, session.native, self.stop_event, self.suspend_watch)) is not None:
                yield self._item(item)
            return
        while not self.stopping:
            item = pump.pending_item(self.native, self.stop_event)
            if item == 'stopped':
                return
            if item is not None:
                yield self._item(item)
                continue
            data = session.read(READ_SLICE_MS)
            resumed = self.suspend_watch is not None and self.suspend_watch.resumed()
            pump.received(session.native, data, resumed)


def drained(items, depth=DRAIN_DEPTH):
    """Iterate `items` on its own thread so a slow consumer never delays reading.

When the consumer falls `depth` items behind, the oldest waiting items are
dropped. Errors of the iterator reach the consumer once the items before them
have been taken.
"""
    condition = threading.Condition()
    waiting = deque(maxlen=depth)
    state = {'done': False, 'error': None}

    def read():
        try:
            for item in items:
                with condition:
                    waiting.append(item)
                    condition.notify()
        except Exception as error:
            state['error'] = error
        finally:
            with condition:
                state['done'] = True
                condition.notify()

    threading.Thread(target=read, name='tracker-drain', daemon=True).start()
    while True:
        with condition:
            condition.wait_for(lambda: waiting or state['done'])
            item = waiting.popleft() if waiting else None
            finished = state['done'] and not waiting
        if item is not None:
            yield item
        elif finished:
            if state['error'] is not None:
                raise state['error']
            return


class GazeAndImageStreams:
    """Gaze, image and optionally presence streams of one session, for a live service.

Every gaze frame goes to `on_gaze(frame)` and every presence record to
`on_presence(timestamp_us, platmod_value)` as it arrives, restamped onto
`host_clock` (microseconds), with the device value mapped as
receive_presence_data does. Both run on the reader thread, so they must be
thread safe and quick. When the presence subscription succeeds, an unknown
record stamped 30 ms before the current time comes first, as the runtime's
presence subscribe emits one until the device reports (0x140276dd0).
newest_frames() yields only the newest image frame, dropping frames that
arrive while the consumer is busy; image_time maps their device timestamps
onto `host_clock`. Gaze and images use separate clock mappers, so head pose
timing is the same as without gaze. A device clock that jumps back ends the
session with DeviceError. `on_device_stream(message, host_us)`, when given,
also receives each gaze and presence message as the device sent it, with its
time on `host_clock`.
"""
    def __init__(self, session, host_clock, on_gaze, on_presence=None,
                 silence_timeout_s=SILENCE_TIMEOUT_S, suspend_watch=None, stop_event=None,
                 device_clock=None, on_device_stream=None):
        streams = (Stream.GAZE, Stream.IMAGE) + ((Stream.PRESENCE,) if on_presence is not None else ())
        self.reader = StreamReader(session, streams, optional=(Stream.PRESENCE,),
                                   suspend_watch=suspend_watch, stop_event=stop_event)
        self.host_clock = host_clock
        self.on_gaze, self.on_presence = on_gaze, on_presence
        self.on_device_stream = on_device_stream
        self.image_time = ClockMapper(host_clock)
        self.gaze_time = ClockMapper(host_clock)
        self.device_clock = device_clock if device_clock is not None else DeviceClockWatch()
        self.silence_timeout_s = silence_timeout_s
        self.reading = threading.Event()
        self.finished = threading.Event()

    def __enter__(self):
        from .prp_service import presence_subscribe_time
        self.reader.subscribe()
        if Stream.PRESENCE in self.reader.subscribed:
            self.on_presence(presence_subscribe_time(self.host_clock()), gaze.PLATMOD_UNKNOWN)
        return self

    def __exit__(self, exc_type, *_):
        self.reader.stop()
        if self.reading.is_set():
            self.finished.wait(READER_EXIT_TIMEOUT_S)
        self.reader.unsubscribe_after(exc_type)

    def _device_time(self, device_us):
        self.gaze_time.observe(device_us)
        return self.gaze_time(device_us)

    def _images(self):
        self.reading.set()
        try:
            for item in self.reader.items(self.silence_timeout_s):
                if item.stream == Stream.IMAGE:
                    self.device_clock.observe(item.value.timestamp)
                    yield item.value
                elif item.stream == Stream.GAZE:
                    self.device_clock.observe(item.value.timestamp_us)
                    host_us = self._device_time(item.value.timestamp_us)
                    self.on_gaze(item.value.with_timestamp(host_us))
                    self._forward(item.message, host_us)
                elif item.stream == Stream.PRESENCE:
                    host_us = self._device_time(item.value.timestamp_us)
                    self.on_presence(host_us, gaze.platmod_presence(item.value.value))
                    self._forward(item.message, host_us)
        except DeviceError:
            self.reader.session.dead = True
            raise
        finally:
            self.finished.set()

    def _forward(self, message, host_us):
        if self.on_device_stream is not None:
            self.on_device_stream(message, host_us)

    @property
    def wire_streams(self):
        return frozenset(wire_stream_id(stream) for stream in self.reader.subscribed)

    def newest_frames(self):
        return newest_frames(self._images(), on_receive=lambda frame: self.image_time.observe(frame.timestamp))


def head_pose_and_gaze(session, model_directory, display_area, host_clock, on_gaze, on_presence=None, camera=0):
    """driver.head_pose_samples with a host clock, plus live gaze (and presence) through the callbacks."""
    process = head_pose_processor(session, model_directory, display_area, camera)
    with GazeAndImageStreams(session, host_clock, on_gaze, on_presence) as streams:
        for frame in streams.newest_frames():
            sample = process(frame)
            yield replace(sample, timestamp_us=streams.image_time(sample.timestamp_us))


@dataclass
class ServiceFeed:
    """Where one service session delivers its output; every callback must be thread safe.

`started(calibration_id)` runs once the streams are subscribed, before the first
item, with the device's calibration id (None when it reports none). With
`device(replies)` the session first reads the device replies tet-tcp clients
get (tet_tcp_service.DeviceReplies), passed with the subscribed streams before
`started`; `device_stream(message, host_us)` then receives every raw gaze and
presence message.
"""
    sample: object
    gaze: object = None
    presence: object = None
    started: object = None
    device: object = None
    device_stream: object = None


def serve_tracker(session, processor, host_clock, feed, stop, suspend_watch=None):
    """One service session on a started tracker: calibration, streams, then samples until it ends.

`processor` is a driver.HeadPoseProcessor (or any object with start(calibration)
and a call from frame to sample). Returns when `stop` (threading.Event) is set;
raises DeviceError and the like when the tracker fails or goes silent.
"""
    processor.start(session.camera_calibration())
    calibration_id = device_calibration_id(session)
    replies = None
    if feed.device is not None:
        from .tet_tcp_service import DeviceReplies
        replies = DeviceReplies.capture(session)
    gaze_callback = feed.gaze if feed.gaze is not None else (lambda frame: None)
    with GazeAndImageStreams(session, host_clock, gaze_callback, feed.presence,
                             suspend_watch=suspend_watch, stop_event=StopEvent(),
                             on_device_stream=feed.device_stream) as streams:
        stopper = StopRelay(stop, streams.reader.stop)
        if replies is not None:
            feed.device(replace(replies, streams=streams.wire_streams))
        if feed.started is not None:
            feed.started(calibration_id)
        try:
            for frame in streams.newest_frames():
                if stop.is_set():
                    break
                sample = processor(frame)
                feed.sample(replace(sample, timestamp_us=streams.image_time(sample.timestamp_us)))
        finally:
            stopper.release()


def device_calibration_id(session):
    """The calibration id in the device status (1490), as the runtime reads it at startup."""
    from .gaze_device import device_calibration_id as from_status
    from .ttp_commands import Command, header_only_request
    reply = session.call(lambda rid: header_only_request(rid, Command.GET_DEVICE_STATUS))
    return from_status(session.reply_values(reply))


class StopRelay:
    """Calls `action` once when `stop` is set, until release(); for stop events shared by sessions."""
    def __init__(self, stop, action):
        self.released = threading.Event()
        self.thread = threading.Thread(target=self._wait, args=(stop, action), daemon=True,
                                       name='tracker-stop')
        self.thread.start()

    def _wait(self, stop, action):
        while not self.released.is_set():
            if stop.wait(0.2):
                action()
                return

    def release(self):
        self.released.set()
