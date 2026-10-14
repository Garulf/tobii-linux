"""The display setup step as a fullscreen web page, served with the standard library.

It follows Tobii's ScreenPlaneSetupControl: a picture of the tracker sits
150 px above the bottom of the screen and two dashed lines run from it down to
the bottom edge. The user lines them up with the two marks on top of the real
tracker below the screen, by dragging a line (moves that line) or the tracker
picture (moves both). The line positions, as fractions of the screen width,
give the monitor width and the tracker offset (display_setup.size_from_tracker_lines).
The drawing is the setup page's (setup/static/alignment.js, Tobii's layout,
vector artwork and colours). As an invisible Linux addition the arrow keys
nudge the last used line by one device pixel (Shift: ten) and show it in the
hover colour; Tab, L and R pick a line, Enter is Done and Esc cancels.
The intro asking where the tracker is shows only when configure() says so,
as Tobii shows it only while no monitor is mapped.

Line positions are fractions of the page width, so the page has to cover the
whole monitor: a kiosk window as large as the screen counts, otherwise it asks
for fullscreen on a click and refuses to confirm. Under WSL a Windows browser
reaches the page through mirrored networking at http://localhost:PORT/.
"""
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from importlib import resources
import json
import threading
from urllib.parse import parse_qs, urlsplit

DEFAULT_PORT = 8776
MIN_LINE = 0.02
MAX_LINE = 0.98
MIN_LINE_DISTANCE = 0.05


class SetupCancelled(RuntimeError):
    pass


def valid_lines(left, right):
    """ScreenPlaneSetupViewModel's limits: both lines in 0.02..0.98 and at least 0.05 apart."""
    return MIN_LINE <= left and right <= MAX_LINE and right - left >= MIN_LINE_DISTANCE - 1e-12


PAGE = """<!doctype html>
<html><head><meta charset="utf-8"><title>Eye tracker display setup</title>
<style>
html,body{margin:0;height:100%;background:#021419;overflow:hidden}
#close{position:absolute;top:0;right:0;z-index:10;width:35px;height:30px;padding:5px 5px 0 0;border:0;
background:transparent;color:#afc4cd;cursor:pointer}
#close:hover{color:#fff}
</style></head>
<body tabindex="-1"><button id="close" title="Cancel (Esc)" aria-label="Cancel"><svg viewBox="0 0 60 60" width="25" height="25">
<path d="M15,15 L45,45 M15,45 L45,15" stroke="currentColor" stroke-width="2.4" stroke-linecap="round"/></svg></button>
<script src="/alignment.js"></script>
<script>
const auto = location.pathname === '/auto';
let phase = 'loading';
function fullscreen() {
  return document.fullscreenElement != null ||
    (window.innerWidth >= screen.width - 2 && window.innerHeight >= screen.height - 2);
}
function viewport() {
  const dpr = window.devicePixelRatio || 1;
  return {width: window.innerWidth, height: window.innerHeight, device_pixel_ratio: dpr,
          screen_width: screen.width, screen_height: screen.height,
          screen_px: [Math.round(screen.width * dpr), Math.round(screen.height * dpr)], fullscreen: fullscreen()};
}
function enterFullscreen() {
  if (!document.fullscreenElement && document.documentElement.requestFullscreen)
    document.documentElement.requestFullscreen().catch(() => {});
}
const alignment = new TrackerAlignment(document.body, {
  canFinish: () => auto || fullscreen(), onThisDisplay: enterFullscreen, onDone: (left, right) => confirm(left, right)});
async function post(body) {
  const reply = await fetch('/result', {method: 'POST', body: JSON.stringify({...body, viewport: viewport()})});
  return reply.json();
}
async function loadConfig() {
  const v = viewport();
  for (;;) {
    try { return await (await fetch(`/setup?sw=${v.screen_px[0]}&sh=${v.screen_px[1]}`, {cache: 'no-store'})).json(); }
    catch (e) { await new Promise((r) => setTimeout(r, 500)); }
  }
}
async function confirm(left, right) {
  phase = 'confirmed';
  document.getElementById('close').hidden = true;
  const reply = await post({left: left, right: right});
  alignment.setText([reply.message || 'Display setup saved', reply.summary || '']);
  followUp(reply);
}
async function followUp(reply) {
  if (reply.next_url) {
    for (;;) {
      await new Promise(r => setTimeout(r, 1000));
      try { await fetch(reply.next_url, {mode: 'no-cors', cache: 'no-store'}); location.href = reply.next_url; return; }
      catch (e) {}
    }
  }
  for (;;) {
    await new Promise(r => setTimeout(r, 500));
    try { const s = await (await fetch('/status', {cache: 'no-store'})).json();
          if (s.message) { alignment.setText([s.message]); if (s.final) return; } }
    catch (e) { return; }
  }
}
async function cancel() {
  if (phase !== 'shown') return;
  phase = 'cancelled';
  document.getElementById('close').hidden = true;
  alignment.setText(['Display setup cancelled', 'Nothing was changed.']);
  await post({cancelled: true});
}
document.getElementById('close').addEventListener('click', cancel);
document.addEventListener('mousedown', enterFullscreen);
document.addEventListener('keydown', (e) => { if (e.key === 'Escape') cancel(); else if (phase === 'shown') alignment.handleKey(e); });
document.addEventListener('fullscreenchange', () => alignment.draw());
window.focus(); document.body.focus();
loadConfig().then((config) => {
  phase = 'shown';
  alignment.show({...config, intro: config.intro && !auto});
  if (auto) setTimeout(() => alignment.finish(), 1700);
});
</script></body></html>
"""


class SetupPageState:
    """Configuration for the page and the user's answer, shared with the HTTP handlers."""

    def __init__(self, configure, evaluate):
        self.configure = configure
        self.evaluate = evaluate
        self.condition = threading.Condition()
        self.result = None
        self.status = {'message': '', 'final': False}

    def answer(self, report):
        with self.condition:
            if self.result is not None:
                return {'message': 'Already answered'}
            if report.get('cancelled'):
                self.result = report
                self.condition.notify_all()
                return {'message': 'Cancelled'}
            left, right = float(report['left']), float(report['right'])
            if not valid_lines(left, right):
                raise ValueError('Line positions outside 0.02..0.98 or closer than 0.05')
            reply = self.evaluate(left, right, report.get('viewport') or {})
            self.result = {**report, 'left': left, 'right': right, 'reply': reply}
            self.condition.notify_all()
            return reply

    def wait(self, timeout_s):
        with self.condition:
            if not self.condition.wait_for(lambda: self.result is not None, timeout=timeout_s):
                raise TimeoutError('No answer from the display setup page')
            if self.result.get('cancelled'):
                raise SetupCancelled('Display setup cancelled on the page')
            return self.result

    def finish(self, message):
        with self.condition:
            self.status = {'message': message, 'final': True}


def alignment_script():
    """The tracker alignment drawing shared with the setup page (tobii_linux/setup/static/alignment.js)."""
    return resources.files('tobii_linux.setup').joinpath('static', 'alignment.js').read_text()


def handler(state):
    class Handler(BaseHTTPRequestHandler):
        def _send(self, body, content_type='application/json', status=200):
            data = body.encode()
            self.send_response(status)
            self.send_header('Content-Type', content_type)
            self.send_header('Cache-Control', 'no-store')
            self.send_header('Content-Length', str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def do_GET(self):
            url = urlsplit(self.path)
            if url.path in ('/', '/auto'):
                self._send(PAGE, 'text/html; charset=utf-8')
            elif url.path == '/alignment.js':
                self._send(alignment_script(), 'text/javascript; charset=utf-8')
            elif url.path == '/setup':
                query = parse_qs(url.query)
                screen_px = tuple(int(float(query[k][0])) for k in ('sw', 'sh')) if 'sw' in query else None
                self._send(json.dumps(state.configure(screen_px)))
            elif url.path == '/status':
                with state.condition:
                    self._send(json.dumps(state.status))
            else:
                self.send_error(404)

        def do_POST(self):
            if self.path != '/result':
                self.send_error(404)
                return
            length = int(self.headers.get('Content-Length', 0))
            try:
                reply = state.answer(json.loads(self.rfile.read(length) or b'{}'))
            except (ValueError, KeyError, TypeError) as error:
                self._send(json.dumps({'error': str(error)}), status=400)
                return
            self._send(json.dumps(reply))

        def log_message(self, *args):
            pass
    return Handler


class DisplaySetupPage:
    """Serves the page; configure(screen_px) returns the initial lines, evaluate(left, right, viewport) the outcome."""

    def __init__(self, configure, evaluate, host='127.0.0.1', port=DEFAULT_PORT):
        self.state = SetupPageState(configure, evaluate)
        self.server = ThreadingHTTPServer((host, port), handler(self.state))
        self.thread = threading.Thread(target=self.server.serve_forever, name='display-setup-page', daemon=True)

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

    def wait_result(self, timeout_s):
        return self.state.wait(timeout_s)

    def finish(self, message):
        self.state.finish(message)
