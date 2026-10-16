"""Direction-specific channel framing recovered from the ET5 Windows runtime.

Supplemental static/emulated evidence is in docs/usb-transport-static.md.
Validation limits are our design, not vendor limits. The implementation is
crates/tobii-protocol/src/framing.rs.
"""
from ._native import ttp as _ttp

CHANNEL_COUNT = _ttp.CHANNEL_COUNT
MAX_FRAME = _ttp.MAX_FRAME  # Local memory bound; not a claimed firmware limit.
MAX_OUT_PAYLOAD = _ttp.MAX_OUT_PAYLOAD

outgoing_frames = _ttp.outgoing_frames
IncomingFrame = _ttp.IncomingFrame
IncomingFrames = _ttp.IncomingFrames

__all__ = ["CHANNEL_COUNT", "MAX_FRAME", "MAX_OUT_PAYLOAD", "outgoing_frames", "IncomingFrame", "IncomingFrames"]
