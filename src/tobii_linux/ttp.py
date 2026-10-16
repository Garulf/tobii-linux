"""Recovered inner TTP framing, not the USB transport envelope.

Derived from the installed Tobii platform runtime; see docs/usb-protocol.md.
Unknown header words and TLV tags are preserved. Nothing here writes to hardware.
The implementation is the Rust crate tobii-protocol (crates/tobii-protocol/src/ttp.rs).
"""
from ._native import ProtocolError
from ._native import ttp as _ttp

MAX_PAYLOAD = _ttp.MAX_PAYLOAD  # Local resource limit, not a claimed device limit.
HARDWARE_INFO_COMMAND = _ttp.HARDWARE_INFO_COMMAND
HARDWARE_INFO_MIN_PROTOCOL = _ttp.HARDWARE_INFO_MIN_PROTOCOL

Message = _ttp.Message
Field = _ttp.Field
Decoder = _ttp.Decoder
fields = _ttp.fields
uint32_array = _ttp.uint32_array
read_uint32_array = _ttp.read_uint32_array
protocol_upgrade = _ttp.protocol_upgrade
hardware_info_request = _ttp.hardware_info_request


def uint32(value: int) -> int:
    if type(value) is not int or not 0 <= value <= 0xFFFFFFFF:
        raise ProtocolError("Expected an unsigned 32-bit integer")
    return value


__all__ = ["MAX_PAYLOAD", "HARDWARE_INFO_COMMAND", "HARDWARE_INFO_MIN_PROTOCOL", "ProtocolError", "Message",
           "Field", "Decoder", "fields", "uint32", "uint32_array", "read_uint32_array", "protocol_upgrade",
           "hardware_info_request"]
