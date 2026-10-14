"""Fullscreen calibration targets in a web browser, served with the standard library.

The calibration runs where the tracker is attached (Linux or WSL); the page
can be open in any browser that reaches the port, including a Windows browser
on the same machine through WSL's mirrored networking (http://localhost:PORT).
The page polls the current target, draws it at the normalized position of the
full screen, and acknowledges each target once its shrinking animation ends,
so collection starts only after the target is actually on screen.
"""
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import threading
import time

from .calibration import CalibrationUI


class CalibrationAborted(RuntimeError):
    pass

PAGE = """<!doctype html>
<html><head><meta charset="utf-8"><title>Eye tracker calibration</title>
<style>
html,body{margin:0;height:100%;background:#202124;overflow:hidden;font-family:sans-serif;color:#e8eaed}
body.running{cursor:none}
#dot{position:absolute;width:60px;height:60px;margin:-30px 0 0 -30px;border-radius:50%;
background:#f5f5f5;transform:scale(1);transition:transform 900ms ease-in,left 500ms,top 500ms;display:none}
#dot::after{content:"";position:absolute;left:24px;top:24px;width:12px;height:12px;border-radius:50%;background:#d93025}
#dot.small{transform:scale(0.35)}
#dot.pop{transform:scale(0);transition:transform 250ms}
#intro,#message{position:absolute;left:10%;right:10%;top:36%;text-align:center}
#intro h1{font-size:34px;font-weight:normal;margin:0 0 18px}
#intro p{font-size:22px;margin:8px 0}
#status{font-size:26px;margin-top:28px;min-height:1.3em}
#intro .hint{font-size:17px;color:#9aa0a6;margin-top:28px}
#message{font-size:28px;display:none}
</style></head>
<body tabindex="-1"><div id="dot"></div>
<div id="intro"><h1>Let's calibrate for your eyes</h1>
<p>Look at each dot until it disappears. Keep the screen at the same angle while it runs.</p>
<p id="status"></p>
<p class="hint">Press any key or click to start now. Esc stops the calibration.</p></div>
<div id="message"></div>
<script>
const COUNTDOWN_S = 3;
const dot = document.getElementById('dot'), intro = document.getElementById('intro');
const message = document.getElementById('message'), status = document.getElementById('status');
let seq = -1, started = location.pathname === '/autostart', aborted = false, startNow = false;
let fullscreenSince = null;
function fullscreen() {
  return document.fullscreenElement != null ||
    (window.innerWidth >= screen.width - 2 && window.innerHeight >= screen.height - 2);
}
function ack(s) {
  fetch('/ack', {method: 'POST', body: JSON.stringify({seq: s, width: window.innerWidth,
    height: window.innerHeight, screen_width: screen.width, screen_height: screen.height,
    fullscreen: fullscreen(), started: started, aborted: aborted})}).catch(() => {});
}
function setText(element, text) { if (element.textContent !== text) element.textContent = text; }
function requestStart() {
  if (!document.fullscreenElement && document.documentElement.requestFullscreen)
    document.documentElement.requestFullscreen().catch(() => {});
  startNow = true;
}
function tick() {
  if (started || aborted) return;
  if (!fullscreen()) {
    fullscreenSince = null;
    setText(status, 'The page has to fill the screen: press F11 or click.');
  } else {
    fullscreenSince = fullscreenSince || performance.now();
    const left = COUNTDOWN_S - Math.floor((performance.now() - fullscreenSince) / 1000);
    if (startNow || left <= 0) { started = true; document.body.classList.add('running');
                                 setText(status, 'Starting...'); ack(seq); return; }
    setText(status, 'Starting in ' + left);
  }
  setTimeout(tick, 100);
}
function stop() {
  aborted = true; ack(seq);
  intro.style.display = 'none'; dot.style.display = 'none';
  message.style.display = 'block'; setText(message, 'Calibration stopped.');
}
document.addEventListener('click', requestStart);
document.addEventListener('keydown', (event) => { if (event.key === 'Escape') stop(); else requestStart(); });
function show(state) {
  if (state.phase === 'point') {
    intro.style.display = 'none'; message.style.display = 'none';
    dot.className = '';
    dot.style.display = 'block';
    dot.style.left = (state.x * window.innerWidth) + 'px';
    dot.style.top = (state.y * window.innerHeight) + 'px';
    const s = state.seq;
    setTimeout(() => { dot.className = 'small'; setTimeout(() => ack(s), 950); }, 550);
  } else if (state.phase === 'collected') {
    dot.className = 'pop';
    ack(state.seq);
  } else if (state.phase === 'message') {
    intro.style.display = 'none'; dot.style.display = 'none';
    message.style.display = 'block'; setText(message, state.message);
    ack(state.seq);
  } else ack(state.seq);
}
async function poll() {
  try {
    const state = await (await fetch('/state', {cache: 'no-store'})).json();
    if (state.seq !== seq) { seq = state.seq; show(state); }
    else if (state.phase === 'waiting') ack(seq);
  } catch (e) {}
  setTimeout(poll, 50);
}
window.focus(); document.body.focus();
tick(); poll();
</script></body></html>
"""


FULLSCREEN_TOLERANCE_PX = 2


def fills_screen(report):
    """Whether the page covered the whole screen: fullscreen, or a kiosk window as large as the screen."""
    if not report:
        return False
    if report.get('fullscreen'):
        return True
    try:
        return (report['width'] >= report['screen_width'] - FULLSCREEN_TOLERANCE_PX
                and report['height'] >= report['screen_height'] - FULLSCREEN_TOLERANCE_PX)
    except (KeyError, TypeError):
        return False


class PageState:
    """Shared target state between the calibration thread and the HTTP handlers."""

    def __init__(self):
        self.condition = threading.Condition()
        self.state = {'seq': 0, 'phase': 'waiting', 'message': ''}
        self.acks = {}
        self.display = None
        self.aborted = False

    def publish(self, **values):
        with self.condition:
            self.state = {'seq': self.state['seq'] + 1, **values}
            self.condition.notify_all()
            return self.state['seq']

    def acknowledge(self, report):
        with self.condition:
            self.acks[report.get('seq')] = report
            self.display = report
            self.aborted = self.aborted or bool(report.get('aborted'))
            self.condition.notify_all()

    def _wait(self, ready, timeout_s, failure):
        if not self.condition.wait_for(lambda: self.aborted or ready(), timeout=timeout_s):
            raise TimeoutError(f'{failure}; last page report: {self.display}')
        if self.aborted:
            raise CalibrationAborted('Stopped on the calibration page')

    def wait_ack(self, seq, timeout_s):
        with self.condition:
            self._wait(lambda: seq in self.acks, timeout_s, f'The calibration page did not show step {seq}')
            return self.acks[seq]

    def wait_started(self, timeout_s):
        """Block until a page covering the screen reports that the calibration started."""
        with self.condition:
            self._wait(lambda: fills_screen(self.display) and bool(self.display.get('started')),
                       timeout_s, 'No fullscreen calibration page was started')
            return self.display


def handler(page):
    class Handler(BaseHTTPRequestHandler):
        def _send(self, body, content_type):
            data = body.encode()
            self.send_response(200)
            self.send_header('Content-Type', content_type)
            self.send_header('Cache-Control', 'no-store')
            self.send_header('Content-Length', str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def do_GET(self):
            if self.path in ('/', '/autostart'):
                self._send(PAGE, 'text/html; charset=utf-8')
            elif self.path == '/state':
                with page.condition:
                    self._send(json.dumps(page.state), 'application/json')
            else:
                self.send_error(404)

        def do_POST(self):
            if self.path != '/ack':
                self.send_error(404)
                return
            length = int(self.headers.get('Content-Length', 0))
            try:
                page.acknowledge(json.loads(self.rfile.read(length) or b'{}'))
            except ValueError:
                self.send_error(400)
                return
            self._send('{}', 'application/json')

        def log_message(self, *args):
            pass
    return Handler


class PageCalibrationUI(CalibrationUI):
    """CalibrationUI that shows targets on the served page.

show_point returns after the page drew the target and finished its
animation (about 1.5 s, during which the user moves their eyes to it), plus
settle_s, the 200 ms the Tobii client waits for the saccade to end.
"""

    def __init__(self, host='127.0.0.1', port=8775, settle_s=0.2, step_timeout_s=30.0):
        self.page = PageState()
        self.server = ThreadingHTTPServer((host, port), handler(self.page))
        self.settle_s = settle_s
        self.step_timeout_s = step_timeout_s
        self.thread = threading.Thread(target=self.server.serve_forever, name='calibration-page', daemon=True)

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

    def wait_for_display(self, timeout_s):
        """Block until a fullscreen page started (after its countdown, or a key or click); returns its report."""
        return self.page.wait_started(timeout_s)

    @property
    def screen_px(self):
        display = self.page.display or {}
        return display.get('width'), display.get('height')

    def message(self, text):
        seq = self.page.publish(phase='message', message=text)
        return self.page.wait_ack(seq, self.step_timeout_s)

    def show_point(self, index, x, y):
        seq = self.page.publish(phase='point', index=index, x=x, y=y)
        report = self.page.wait_ack(seq, self.step_timeout_s)
        if not fills_screen(report):
            raise RuntimeError('The calibration page left fullscreen')
        time.sleep(self.settle_s)

    def restarting(self, attempt):
        self.message('Once more: look at each dot until it disappears')
        time.sleep(2.0)

    def point_done(self, index, accepted):
        seq = self.page.publish(phase='collected' if accepted else 'message',
                                index=index, message='' if accepted else 'Look at the dot again')
        self.page.wait_ack(seq, self.step_timeout_s)
