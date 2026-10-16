"""Raw USB traffic log for live experiments: every transfer, as JSON lines.

LoggingTransport wraps a transport (usb.BulkTransport or a Python object with
the same methods) and records each bulk write, each bulk read (including the
partial data of a read that timed out, and every chunk the channel's continuous
reader receives) and each control transfer with the host monotonic time, before
any framing or TTP decoding. replay_messages rebuilds the TTP messages of one
channel from such a log with the same parsers the session uses, so a stalled
run can be analysed offline. The implementation is crates/tobii-usb/src/traffic.rs.
"""
from ._native import usb as _usb

TrafficLog = _usb.TrafficLog
LoggingTransport = _usb.LoggingTransport


def replay_messages(path, channel=0):
    """(time, direction, Message or error text) for a traffic log, decoded as the session would."""
    yield from _usb.replay_messages(str(path), channel)
