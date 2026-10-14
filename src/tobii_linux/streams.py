"""Several subscribed TTP streams read from one TrackerSession and split by stream.

TrackerSession.frames() keeps only image frames. StreamReader subscribes any
set of streams and decodes images (frames.image_frame), gaze (gaze.gaze_frame
of gaze.gaze_message, which drops a message exactly where the runtime does) and
presence (gaze.presence_message); other stream messages pass through undecoded.
GazeAndImageStreams adds the threading and clock mapping a live service needs.
"""
from collections import Counter
from dataclasses import dataclass, replace
import threading
import time

from . import gaze
from .driver import ClockMapper, DeviceError, head_pose_processor, newest_frames
from .frames import image_frame
from .ttp import ProtocolError
from .ttp_commands import STREAM_DATA_KIND, Stream, subscribe_stream, unsubscribe_stream, wire_stream_id

READ_SLICE_MS = 100
READER_EXIT_TIMEOUT_S = 2.0


@dataclass(frozen=True)
class Presence:
    """One presence stream (0x504) record: device timestamp and the device's presence value."""
    timestamp_us: int
    value: int


def gaze_frame(message):
    device = gaze.gaze_message(message)
    return gaze.gaze_frame(device) if device is not None else None


def presence(message):
    return Presence(*gaze.presence_message(message.payload))


DECODERS = {Stream.IMAGE: image_frame, Stream.GAZE: gaze_frame, Stream.PRESENCE: presence}


@dataclass(frozen=True)
class StreamItem:
    stream: int
    value: object


class StreamReader:
    """Reads stream data from a started session; stop() may be called from another thread.

Streams listed in `optional` may refuse the subscription; they are then left
out and listed in `refused`.
"""
    def __init__(self, session, streams=(Stream.GAZE, Stream.IMAGE), optional=()):
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
        self.stopping = False

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
        """Unsubscribe every subscribed stream; errors are collected so each stream gets its request."""
        errors = []
        while self.subscribed:
            stream = self.subscribed.pop()
            try:
                self.session.call(lambda rid, s=stream: unsubscribe_stream(rid, s))
            except (DeviceError, OSError, RuntimeError) as error:
                errors.append(error)
        self.session.pending_streams = []
        if errors:
            raise errors[0]

    def stop(self):
        self.stopping = True

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
            return StreamItem(stream, message)
        try:
            value = decoder(message)
        except ProtocolError as error:
            self.decode_errors[stream] += 1
            self.last_error = f'{stream.name.lower()} message: {error}'
            return None
        if value is None:
            self.dropped[stream] += 1
            return None
        return StreamItem(stream, value)

    def items(self, silence_timeout_s=3.0):
        """Decoded items as they arrive until stop() is called.

Raises DeviceError when no stream data arrives for `silence_timeout_s`.
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
            data = session.channel.read(timeout_ms=READ_SLICE_MS)
            pending = session.decoder.feed(data) if data else []
            if pending:
                last_data = time.monotonic()
            elif time.monotonic() - last_data > silence_timeout_s:
                raise DeviceError(f'The tracker sent no stream data for {silence_timeout_s:g} s')


class GazeAndImageStreams:
    """Gaze, image and optionally presence streams of one session, for a live service.

Every gaze frame goes to `on_gaze(frame)` and every presence record to
`on_presence(timestamp_us, value)` as it arrives, restamped onto `host_clock`
(microseconds). Both run on the reader thread, so they must be thread safe.
newest_frames() yields only the newest image frame, dropping frames that
arrive while the consumer is busy; image_time maps their device timestamps
onto `host_clock`. Gaze and images use separate clock mappers, so head pose
timing is the same as without gaze.
"""
    def __init__(self, session, host_clock, on_gaze, on_presence=None, silence_timeout_s=3.0):
        streams = (Stream.GAZE, Stream.IMAGE) + ((Stream.PRESENCE,) if on_presence is not None else ())
        self.reader = StreamReader(session, streams, optional=(Stream.PRESENCE,))
        self.on_gaze, self.on_presence = on_gaze, on_presence
        self.image_time = ClockMapper(host_clock)
        self.gaze_time = ClockMapper(host_clock)
        self.silence_timeout_s = silence_timeout_s
        self.reading = threading.Event()
        self.finished = threading.Event()

    def __enter__(self):
        self.reader.subscribe()
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
                    yield item.value
                elif item.stream == Stream.GAZE:
                    self.on_gaze(item.value.with_timestamp(self._device_time(item.value.timestamp_us)))
                elif item.stream == Stream.PRESENCE:
                    self.on_presence(self._device_time(item.value.timestamp_us), item.value.value)
        finally:
            self.finished.set()

    def newest_frames(self):
        return newest_frames(self._images(), on_receive=lambda frame: self.image_time.observe(frame.timestamp))


def head_pose_and_gaze(session, model_directory, display_area, host_clock, on_gaze, on_presence=None, camera=0):
    """driver.head_pose_samples with a host clock, plus live gaze (and presence) through the callbacks."""
    process = head_pose_processor(session, model_directory, display_area, camera)
    with GazeAndImageStreams(session, host_clock, on_gaze, on_presence) as streams:
        for frame in streams.newest_frames():
            sample = process(frame)
            yield replace(sample, timestamp_us=streams.image_time(sample.timestamp_us))
