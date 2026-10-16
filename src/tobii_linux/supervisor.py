"""Keeps a tracker session running across unplugging, USB errors, stalls and host suspend.

The Windows runtime treats every such event as the device being removed: it
shuts its client server down and starts the device again from scratch once it
is back (docs/service-behaviour.md). TrackerLink does the same for one Linux
process: it opens a session, runs it until it fails, reports the loss, waits
with a growing delay and opens a new one. A session that ran long enough
resets the delay. The loop and the backoff are crates/tobii-device/src/supervisor.rs.
"""
import logging
import threading
import time

from ._native import device as _device

log = logging.getLogger('tobii_linux.supervisor')

# Backoff(first_s=0.5, max_s=2.0, healthy_s=10.0).delay(session_s): the delay before the
# next attempt after a session that lasted `session_s` seconds, doubling up to max_s.
Backoff = _device.Backoff


class TrackerLink:
    """Runs `run_session(session, stop)` on sessions from `open_session()` until stop() is called.

`open_session` returns a context manager yielding a started session, such as
driver.usb_session. `on_state(state, detail)` receives 'searching' (no
tracker), 'starting', 'lost' (a session ended with an error) and 'stopped'.
`run_session` reports when it is running itself (it knows when streaming
starts). Any exception from a session counts as a lost session.
"""
    def __init__(self, open_session, run_session, on_state=None, backoff=None, clock=time.monotonic):
        self.open_session = open_session
        self.run_session = run_session
        self.on_state = on_state or (lambda state, detail: None)
        self.backoff = backoff or Backoff()
        self.clock = clock
        self.stopping = threading.Event()
        self.native = _device.TrackerLink()

    @property
    def sessions(self):
        return self.native.sessions

    @property
    def failures(self):
        return self.native.failures

    def stop(self):
        self.stopping.set()

    def run(self):
        self.native.run(self)
