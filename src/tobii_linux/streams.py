"""Several subscribed TTP streams read from one TrackerSession and split by stream.

TrackerSession.frames() keeps only image frames. StreamReader subscribes any
set of streams and decodes images (frames.image_frame), gaze (gaze.gaze_frame
of gaze.gaze_message, which drops a message exactly where the runtime does) and
presence (gaze.presence_message); other stream messages pass through undecoded.
GazeAndImageStreams adds the threading and clock mapping a live service needs,
and serve_tracker runs one whole service session on it.

The tracker stops sending for good (stream data and replies) when its endpoint
is not read for about a second while streams are subscribed, so every reader
here keeps reading on its own thread whatever its consumer does, and a session
that falls silent is treated as dead rather than waited on.
"""
from collections import Counter, deque
from dataclasses import dataclass, replace
import logging
import threading
import time

from . import gaze
from .driver import ClockMapper, DeviceError, head_pose_processor, newest_frames
from .frames import image_frame
from .ttp import ProtocolError
from .ttp_commands import STREAM_DATA_KIND, Stream, subscribe_stream, unsubscribe_stream, wire_stream_id

log = logging.getLogger('tobii_linux.streams')
READ_SLICE_MS = 100
READER_EXIT_TIMEOUT_S = 2.0
SILENCE_TIMEOUT_S = 3.0
# A suspended host keeps CLOCK_BOOTTIME running while CLOCK_MONOTONIC stops.
SUSPEND_GAP_S = 2.0
# The device clock running backwards by more than this means the tracker restarted.
DEVICE_CLOCK_REWIND_US = 1_000_000
DRAIN_DEPTH = 256


def suspended_time_s():
    return time.clock_gettime(time.CLOCK_BOOTTIME) - time.clock_gettime(time.CLOCK_MONOTONIC)


class SuspendWatch:
    """Notices that the host was suspended since the last check (the tracker loses power then)."""
    def __init__(self, suspended_time=suspended_time_s, threshold_s=SUSPEND_GAP_S):
        self.suspended_time = suspended_time
        self.threshold_s = threshold_s
        self.last = suspended_time()

    def resumed(self):
        now = self.suspended_time()
        jumped = now - self.last > self.threshold_s
        self.last = now
        return jumped


class DeviceClockWatch:
    """Raises when device timestamps jump back, which happens when the tracker restarts."""
    def __init__(self, rewind_us=DEVICE_CLOCK_REWIND_US):
        self.rewind_us = rewind_us
        self.latest = None

    def observe(self, device_us):
        if self.latest is not None and device_us < self.latest - self.rewind_us:
            raise DeviceError('The tracker clock went back; the tracker restarted')
        self.latest = device_us if self.latest is None else max(self.latest, device_us)


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
        self.subscribed = []
        self.refused = {}
        self.received = Counter()
        self.dropped = Counter()
        self.decode_errors = Counter()
        self.unexpected = 0
        self.last_error = None
        self.stop_event = stop_event if stop_event is not None else threading.Event()
        self.suspend_watch = suspend_watch

    def subscribe(self):
        for stream in self.streams:
            try:
                self.session.call(lambda rid, s=stream: subscribe_stream(rid, s))
            except DeviceError as error:
                if stream not in self.optional:
                    raise
                self.refused[stream] = str(error)
                continue
            self.subscribed.append(stream)

    def unsubscribe(self):
        """Unsubscribe every subscribed stream; errors are collected so each stream gets its request.

A dead session is not asked: its device no longer answers.
"""
        if self.session.dead:
            log.info('not unsubscribing %d stream(s) of a dead session', len(self.subscribed))
            self.subscribed.clear()
            self.session.pending_streams = []
            return
        errors, unsubscribed = [], []
        while self.subscribed:
            stream = self.subscribed.pop()
            try:
                self.session.call(lambda rid, s=stream: unsubscribe_stream(rid, s))
                unsubscribed.append(stream.name.lower())
            except (DeviceError, OSError, RuntimeError) as error:
                errors.append(error)
        self.session.pending_streams = []
        log.info('unsubscribed %s', ', '.join(unsubscribed) or 'nothing')
        if errors:
            raise errors[0]

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

    def decode(self, message):
        """The StreamItem for one stream message, or None when it is not ours, dropped or malformed."""
        if message.kind != STREAM_DATA_KIND:
            return None
        stream = self.by_wire_id.get(message.command)
        if stream is None:
            self.unexpected += 1
            return None
        self.received[stream] += 1
        decoder = DECODERS.get(stream)
        if decoder is None:
            return StreamItem(stream, message, message)
        try:
            value = decoder(message)
        except ProtocolError as error:
            self.decode_errors[stream] += 1
            self.last_error = f'{stream.name.lower()} message: {error}'
            return None
        if value is None:
            self.dropped[stream] += 1
            return None
        return StreamItem(stream, value, message)

    def items(self, silence_timeout_s=SILENCE_TIMEOUT_S):
        """Decoded items as they arrive until stop() is called.

Raises DeviceError, marking the session dead, when no stream data arrives for
`silence_timeout_s` or the host was suspended.
"""
        session = self.session
        pending, session.pending_streams = session.pending_streams, []
        last_data = time.monotonic()
        while not self.stopping:
            for message in pending:
                if self.stopping:
                    return
                item = self.decode(message)
                if item is not None:
                    yield item
            data = session.read(READ_SLICE_MS)
            pending = session.decoder.feed(data) if data else []
            if self.suspend_watch is not None and self.suspend_watch.resumed():
                session.dead = True
                raise DeviceError('The host was suspended; the tracker must be started again')
            if pending:
                last_data = time.monotonic()
            elif time.monotonic() - last_data > silence_timeout_s:
                session.dead = True
                raise DeviceError(f'The tracker sent no stream data for {silence_timeout_s:g} s')


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
                             suspend_watch=suspend_watch, stop_event=threading.Event(),
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
