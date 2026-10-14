"""Keeps a tracker session running across unplugging, USB errors, stalls and host suspend.

The Windows runtime treats every such event as the device being removed: it
shuts its client server down and starts the device again from scratch once it
is back (docs/service-behaviour.md). TrackerLink does the same for one Linux
process: it opens a session, runs it until it fails, reports the loss, waits
with a growing delay and opens a new one. A session that ran long enough
resets the delay.
"""
from dataclasses import dataclass
import logging
import threading
import time

from .driver import NoTrackerError

log = logging.getLogger('tobii_linux.supervisor')


@dataclass
class Backoff:
    """Delays between session attempts: doubling from `first_s` up to `max_s`."""
    first_s: float = 0.5
    max_s: float = 2.0
    healthy_s: float = 10.0

    def __post_init__(self):
        self.next_s = self.first_s

    def delay(self, session_s):
        """Delay before the next attempt after a session that lasted `session_s` seconds."""
        if session_s >= self.healthy_s:
            self.next_s = self.first_s
        delay, self.next_s = self.next_s, min(self.next_s * 2, self.max_s)
        return delay


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
        self.sessions = self.failures = 0

    def stop(self):
        self.stopping.set()

    def run(self):
        while not self.stopping.is_set():
            started = self.clock()
            try:
                self.on_state('starting', 'Starting the tracker')
                with self.open_session() as session:
                    self.sessions += 1
                    self.run_session(session, self.stopping)
                if not self.stopping.is_set():
                    self.on_state('lost', 'The tracker session ended')
            except NoTrackerError as error:
                self.on_state('searching', str(error))
            except Exception as error:
                self.failures += 1
                log.warning('tracker session ended: %s: %s', type(error).__name__, error)
                self.on_state('lost', f'{type(error).__name__}: {error}')
            if self.stopping.is_set():
                break
            self.stopping.wait(self.backoff.delay(self.clock() - started))
        self.on_state('stopped', 'Stopped')
