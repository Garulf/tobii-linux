"""Raw USB traffic log for live experiments: every transfer, as JSON lines.

LoggingTransport wraps a usb.Transport and records each bulk write, each bulk
read (including the partial data of a read that timed out) and each control
transfer with the host monotonic time, before any framing or TTP decoding.
replay_messages rebuilds the TTP messages of channel 0 from such a log with
the same parsers the session uses, so a stalled run can be analysed offline.
"""
import json
import time

from .ttp import Decoder
from .usb import USBError
from .usb_framing import IncomingFrames


class TrafficLog:
    def __init__(self, path):
        self.file = open(path, 'w', buffering=1)
        self.start = time.monotonic()
        self.record('start', wall_time=time.time())

    def record(self, op, **values):
        self.file.write(json.dumps({'t': round(time.monotonic() - self.start, 6), 'op': op, **values}) + '\n')

    def close(self):
        self.file.close()


class LoggingTransport:
    def __init__(self, transport, log: TrafficLog):
        self.transport = transport
        self.log = log

    def read(self, size, *, timeout_ms=1000):
        begin = time.monotonic()
        try:
            data = self.transport.read(size, timeout_ms=timeout_ms)
        except USBError as error:
            self.log.record('read_error', code=error.code, timeout_ms=timeout_ms, hex=error.partial_data.hex(),
                            waited_ms=round((time.monotonic() - begin) * 1000, 1))
            raise
        self.log.record('read', timeout_ms=timeout_ms, hex=data.hex(),
                        waited_ms=round((time.monotonic() - begin) * 1000, 1))
        return data

    def write(self, data, *, timeout_ms=1000):
        begin = time.monotonic()
        try:
            count = self.transport.write(data, timeout_ms=timeout_ms)
        except USBError as error:
            self.log.record('write_error', code=error.code, hex=bytes(data).hex(), transferred=error.transferred)
            raise
        self.log.record('write', hex=bytes(data).hex(), transferred=count,
                        waited_ms=round((time.monotonic() - begin) * 1000, 1))
        return count

    def control_out(self, request, **kwargs):
        try:
            result = self.transport.control_out(request, **kwargs)
        except USBError as error:
            self.log.record('control_out_error', request=request, value=kwargs.get('value'), code=error.code)
            raise
        self.log.record('control_out', request=request, value=kwargs.get('value'))
        return result

    def control_in(self, request, **kwargs):
        data = self.transport.control_in(request, **kwargs)
        self.log.record('control_in', request=request, value=kwargs.get('value'), hex=bytes(data).hex())
        return data

    def __getattr__(self, name):
        return getattr(self.transport, name)


def replay_messages(path, channel=0):
    """(time, direction, Message or error text) for a traffic log, decoded as the session would."""
    frames, decoder, sent = IncomingFrames(), Decoder(), Decoder()
    with open(path) as source:
        for line in source:
            row = json.loads(line)
            if row['op'] == 'write':
                data = bytes.fromhex(row['hex'])[8:]
                for message in sent.feed(data):
                    yield row['t'], 'out', message
            elif row['op'] in ('read', 'read_error') and row.get('hex'):
                try:
                    for frame in frames.feed(bytes.fromhex(row['hex'])):
                        if frame.for_channel(channel):
                            for message in decoder.feed(frame.payload):
                                yield row['t'], 'in', message
                except ValueError as error:
                    yield row['t'], 'in', f'{type(error).__name__}: {error}'
                    return
