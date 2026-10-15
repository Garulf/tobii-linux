"""The setup page's server: one page, the screen the flow shows now, and what the user does on it.

The flow thread publishes screens (`PageChannel.show`); every screen has a
sequence number and the page acknowledges each one once it is drawn, so the
flow can wait for a calibration target to be on screen before collecting.
Values that change many times a second (the eye positions) go into `live`
without a new screen. The page sends user actions (`continue`, `done`, ...)
with the screen they were made on; actions for an older screen are ignored,
except `cancel` (Esc), which stands until the flow handles it. Everything is
polled over plain HTTP so any browser works, including a Windows browser
reaching a WSL server through mirrored networking.
"""
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from importlib import resources
import json
import threading
import time

FULLSCREEN_TOLERANCE_PX = 2
PAGE_GONE_S = 20.0
CONTENT_TYPES = {'.html': 'text/html; charset=utf-8', '.js': 'text/javascript; charset=utf-8',
                 '.css': 'text/css; charset=utf-8'}
STATIC_FILES = {'/': 'index.html', '/setup.js': 'setup.js', '/setup.css': 'setup.css', '/alignment.js': 'alignment.js'}


class Cancelled(RuntimeError):
    """The user cancelled on the page."""


class PageGone(RuntimeError):
    """The page stopped answering."""


def fills_screen(view):
    """Whether the page covers the whole screen: fullscreen, or a kiosk window as large as the screen."""
    if not view:
        return False
    if view.get('fullscreen'):
        return True
    try:
        return (view['width'] >= view['screen_width'] - FULLSCREEN_TOLERANCE_PX
                and view['height'] >= view['screen_height'] - FULLSCREEN_TOLERANCE_PX)
    except (KeyError, TypeError):
        return False


class PageChannel:
    """Screens from the flow to the page, acknowledgements and actions back; safe across threads."""

    def __init__(self, clock=time.monotonic):
        self.condition = threading.Condition()
        self.clock = clock
        self.screen = {'seq': 0, 'name': 'connecting'}
        self.live = {}
        self.acked = 0
        self.actions = []
        self.view = None
        self.last_contact = None
        self.cancel_requested = False
        self.monitor = None
        self.page_monitor = None

    def show(self, name, **data):
        with self.condition:
            placed = {'monitor': self.monitor} if self.monitor else {}
            self.screen = {'seq': self.screen['seq'] + 1, 'name': name, **placed, **data}
            self.actions.clear()
            self.condition.notify_all()
            return self.screen['seq']

    def update_live(self, **values):
        with self.condition:
            self.live = {**self.live, **values}

    def snapshot(self):
        """What the page polls; polling also tells the flow that the page is still there."""
        with self.condition:
            self.last_contact = self.clock()
            return {**self.screen, 'live': self.live}

    def clear_cancel(self):
        with self.condition:
            self.cancel_requested = False

    def follow_monitor(self, key):
        """Accept events only from pages opened for monitor `key` (None: from any page)."""
        with self.condition:
            self.monitor = key
            self.page_monitor = None

    def page_event(self, event):
        """An acknowledgement or action from the page; returns False for one made on an older screen.

Once the setup moved to another monitor, events from a page opened for a
different monitor (or for none) are ignored; that page closes itself when it
sees a screen placed on another monitor.
"""
        with self.condition:
            if self.monitor and event.get('monitor') != self.monitor:
                return False
            self.last_contact = self.clock()
            self.page_monitor = event.get('monitor')
            if isinstance(event.get('view'), dict):
                self.view = event['view']
            seq = event.get('seq')
            if event.get('action') == 'cancel':
                self.cancel_requested = True
            if seq == self.screen['seq']:
                self.acked = max(self.acked, seq)
                if event.get('action'):
                    self.actions.append(event)
            self.condition.notify_all()
            return seq == self.screen['seq']

    def has_action(self, name):
        with self.condition:
            return any(event['action'] == name for event in self.actions)

    def check_cancel(self):
        """Raise Cancelled when the user pressed Esc or Cancel, PageGone when the page stopped polling."""
        with self.condition:
            self._check()

    def _check(self):
        if self.cancel_requested:
            raise Cancelled('Cancelled on the page')
        if self.last_contact is not None and self.clock() - self.last_contact > PAGE_GONE_S:
            raise PageGone(f'The page has not been seen for {PAGE_GONE_S:.0f} s')

    @property
    def fullscreen(self):
        with self.condition:
            return fills_screen(self.view)

    def wait_ack(self, seq, timeout_s, idle=None):
        """Block until the page drew screen seq; `idle()` runs between waits (it may read the device)."""
        return self._wait(lambda: self.acked >= seq, timeout_s, idle, f'The page did not show screen {seq}')

    def wait_action(self, actions, timeout_s=None, idle=None, ready=None):
        """The first page action named in `actions` (taken off the queue), or `ready()`'s value once true.

A cancel ends the wait with Cancelled. `idle()` runs between waits.
"""
        def outcome():
            for event in self.actions:
                if event['action'] in actions:
                    self.actions.remove(event)
                    return event
            return ready() if ready else None
        return self._wait(outcome, timeout_s, idle, 'No answer from the page')

    def _wait(self, outcome, timeout_s, idle, failure, slice_s=0.1):
        deadline = None if timeout_s is None else self.clock() + timeout_s
        while True:
            with self.condition:
                self._check()
                result = outcome()
                if result:
                    return result
                if idle is None:
                    remaining = slice_s if deadline is None else max(0.0, min(slice_s, deadline - self.clock()))
                    self.condition.wait(remaining)
                    result = outcome()
                    if result:
                        return result
            if idle is not None:
                idle()
            if deadline is not None and self.clock() >= deadline:
                raise TimeoutError(failure)


def static_file(name):
    return resources.files(__package__).joinpath('static', name).read_bytes()


def handler(channel):
    class Handler(BaseHTTPRequestHandler):
        def _send(self, body, content_type, status=200):
            self.send_response(status)
            self.send_header('Content-Type', content_type)
            self.send_header('Cache-Control', 'no-store')
            self.send_header('Content-Length', str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def _json(self, value, status=200):
            self._send(json.dumps(value).encode(), 'application/json', status)

        def do_GET(self):
            path = self.path.split('?', 1)[0]
            if path in STATIC_FILES:
                name = STATIC_FILES[path]
                self._send(static_file(name), CONTENT_TYPES['.' + name.rsplit('.', 1)[1]])
            elif path == '/state':
                self._json(channel.snapshot())
            else:
                self.send_error(404)

        def do_POST(self):
            if self.path != '/event':
                self.send_error(404)
                return
            length = int(self.headers.get('Content-Length', 0))
            try:
                event = json.loads(self.rfile.read(length) or b'{}')
                if not isinstance(event, dict):
                    raise ValueError('event must be an object')
            except ValueError as error:
                self._json({'error': str(error)}, 400)
                return
            self._json({'current': channel.page_event(event)})

        def log_message(self, *args):
            pass
    return Handler


class SetupServer:
    """Serves the page and the channel on host:port (port 0 picks a free one)."""

    def __init__(self, channel, host='127.0.0.1', port=0):
        self.channel = channel
        self.server = ThreadingHTTPServer((host, port), handler(channel))
        self.thread = threading.Thread(target=self.server.serve_forever, name='setup-page', daemon=True)

    @property
    def url(self):
        host, port = self.server.server_address[:2]
        return f'http://{"localhost" if host in ("127.0.0.1", "0.0.0.0") else host}:{port}/'

    def start(self):
        self.thread.start()
        return self

    def close(self):
        self.server.shutdown()
        self.server.server_close()
