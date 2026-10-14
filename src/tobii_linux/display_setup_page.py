"""The display setup step as a fullscreen web page, served with the standard library.

It follows Tobii's ScreenPlaneSetupControl: a picture of the tracker sits
150 px above the bottom of the screen and two dashed lines run from it down to
the bottom edge. The user lines them up with the two marks on top of the real
tracker below the screen, by dragging a line (moves that line) or the tracker
picture (moves both). The line positions, as fractions of the screen width,
give the monitor width and the tracker offset (display_setup.size_from_tracker_lines).
Arrow keys fine-tune by one device pixel, Enter confirms, Esc cancels; Tobii's
control has no keyboard handling, so the keys are a Linux addition.

Line positions are fractions of the page width, so the page has to cover the
whole monitor: it asks for fullscreen on the first click or key and refuses to
confirm otherwise. Under WSL a Windows browser reaches the page through
mirrored networking at http://localhost:PORT/.
"""
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
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
html,body{margin:0;height:100%;background:#1b1c1e;overflow:hidden;font-family:sans-serif;color:#e8eaed;user-select:none}
canvas{position:absolute;left:0;top:0;width:100%;height:100%}
#panel{position:absolute;left:8%;right:8%;top:7%;text-align:center}
#text{font-size:clamp(16px,2.2vw,28px);line-height:1.45}
#hint{font-size:clamp(12px,1.2vw,16px);color:#9aa0a6}
#readout{margin-top:1.2em;font-size:clamp(13px,1.5vw,20px);color:#bdc1c6}
#done{margin-top:1em;font-size:clamp(14px,1.3vw,18px);padding:8px 28px;
background:#8ab4f8;color:#202124;border:0;border-radius:4px;cursor:pointer}
#done:disabled{background:#5f6368;cursor:default}
</style></head>
<body><canvas id="view"></canvas>
<div id="panel"><div id="text">Where is your eye tracker?<br><span id="hint">If it is not attached below this display yet, attach it now.
Then click or press a key to continue on this display.</span></div>
<div id="readout"></div><button id="done" style="display:none">Done</button></div>
<script>
const view = document.getElementById('view'), ctx = view.getContext('2d');
const text = document.getElementById('text'), hint = document.getElementById('hint');
const readout = document.getElementById('readout'), done = document.getElementById('done');
const auto = location.pathname === '/auto';
const BOTTOM_MARGIN = 150, HIT = 24, ART_RATIO = 72.151 / 812.708;
let config = null, left = 0, right = 1, phase = 'intro', drag = null, selected = 'both', hover = null;

function fullscreen() {
  return document.fullscreenElement != null ||
    (window.innerWidth >= screen.width - 1 && window.innerHeight >= screen.height - 1);
}
function viewport() {
  const dpr = window.devicePixelRatio || 1;
  return {width: window.innerWidth, height: window.innerHeight, device_pixel_ratio: dpr,
          screen_width: screen.width, screen_height: screen.height,
          screen_px: [Math.round(screen.width * dpr), Math.round(screen.height * dpr)], fullscreen: fullscreen()};
}
function setLeft(v) { v = Math.min(v, right - 0.05); left = Math.max(v, 0.02); }
function setRight(v) { v = Math.max(v, left + 0.05); right = Math.min(v, 0.98); }
function geometry() {
  const width = config.tracker_width_mm / (right - left);
  return {width: width, height: width / config.aspect, offset: ((left + right) / 2 - 0.5) * width};
}
function tracker() {
  const w = window.innerWidth, h = window.innerHeight, span = (right - left) * config.picture_proportion;
  const tw = span * w, th = tw * ART_RATIO, top = h - th - BOTTOM_MARGIN;
  return {x: ((left + right) / 2) * w - tw / 2, y: top, w: tw, h: th, lineTop: top + th / 4};
}
function drawLine(x, top, active) {
  const h = window.innerHeight;
  ctx.strokeStyle = active ? '#8ab4f8' : '#e8eaed'; ctx.fillStyle = '#8ab4f8'; ctx.lineWidth = 1;
  ctx.setLineDash([5, 5]); ctx.beginPath(); ctx.moveTo(x, top); ctx.lineTo(x, h); ctx.stroke(); ctx.setLineDash([]);
  ctx.beginPath(); ctx.moveTo(x - 6, h - 10); ctx.lineTo(x, h); ctx.lineTo(x + 6, h - 10); ctx.stroke();
  const y = top + 30;
  ctx.beginPath(); ctx.moveTo(x - 24, y); ctx.lineTo(x - 10, y - 14); ctx.lineTo(x - 10, y + 14); ctx.fill();
  ctx.beginPath(); ctx.moveTo(x + 24, y); ctx.lineTo(x + 10, y - 14); ctx.lineTo(x + 10, y + 14); ctx.fill();
}
function draw() {
  const dpr = window.devicePixelRatio || 1;
  view.width = Math.round(window.innerWidth * dpr); view.height = Math.round(window.innerHeight * dpr);
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0); ctx.clearRect(0, 0, window.innerWidth, window.innerHeight);
  if (phase !== 'lines' || !config) return;
  const t = tracker(), r = Math.min(t.h / 2, 12);
  ctx.fillStyle = '#000'; ctx.strokeStyle = (hover === 'tracker' || selected === 'both') ? '#8ab4f8' : '#fff';
  ctx.lineWidth = 2; ctx.beginPath(); ctx.roundRect(t.x, t.y, t.w, t.h, r); ctx.fill(); ctx.stroke();
  drawLine(left * window.innerWidth, t.lineTop, hover === 'left' || selected === 'left');
  drawLine(right * window.innerWidth, t.lineTop, hover === 'right' || selected === 'right');
  const g = geometry(), v = viewport();
  readout.innerHTML = `Display ${g.width.toFixed(1)} x ${g.height.toFixed(1)} mm (${(Math.hypot(g.width, g.height) / 25.4).toFixed(1)} in),
    tracker ${g.offset >= 0 ? g.offset.toFixed(1) + ' mm right of' : (-g.offset).toFixed(1) + ' mm left of'} centre` +
    (v.fullscreen ? '' : '<br>Not fullscreen: press F11 or click to enter fullscreen before confirming');
  done.disabled = !v.fullscreen;
}
function target(x, y) {
  const w = window.innerWidth, t = tracker();
  if (y >= t.lineTop && Math.abs(x - left * w) <= HIT) return 'left';
  if (y >= t.lineTop && Math.abs(x - right * w) <= HIT) return 'right';
  if (x >= t.x && x <= t.x + t.w && y >= t.y && y <= t.y + t.h) return 'tracker';
  return null;
}
function enterFullscreen() {
  if (!document.fullscreenElement && document.documentElement.requestFullscreen)
    document.documentElement.requestFullscreen().catch(() => {});
}
async function loadLines() {
  const v = viewport();
  config = await (await fetch(`/setup?sw=${v.screen_px[0]}&sh=${v.screen_px[1]}`, {cache: 'no-store'})).json();
  phase = 'lines';
  text.innerHTML = config.instructions + '<br><span id="hint">Drag a line to move it, drag the tracker picture to move both. ' +
    'Arrow keys move the selected item by one pixel (Shift: ten); L, R and B select the left line, the right line or both. ' +
    'Enter confirms, Esc cancels.</span>';
  done.style.display = 'inline-block';
  const l0 = config.left, r0 = config.right, started = performance.now();
  function animate(now) {
    const p = Math.min(1, (now - started) / 1500), e = p < 0.5 ? 2 * p * p : 1 - Math.pow(-2 * p + 2, 2) / 2;
    left = l0 - 0.01 * (1 - e); right = r0 + 0.01 * (1 - e); draw();
    if (p < 1) requestAnimationFrame(animate);
    else { left = l0; right = r0; draw(); if (auto) confirm(); }
  }
  requestAnimationFrame(animate);
}
async function post(body) {
  const reply = await fetch('/result', {method: 'POST', body: JSON.stringify({...body, viewport: viewport()})});
  return reply.json();
}
async function confirm() {
  if (phase !== 'lines' || (!auto && !fullscreen())) return;
  phase = 'confirmed'; done.style.display = 'none';
  const reply = await post({left: left, right: right});
  text.textContent = reply.message || 'Done';
  readout.textContent = reply.summary || '';
  draw(); followUp(reply);
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
    try { const s = await (await fetch('/status', {cache: 'no-store'})).json(); if (s.message) { text.textContent = s.message; if (s.final) return; } }
    catch (e) { return; }
  }
}
async function cancel() {
  phase = 'cancelled'; done.style.display = 'none'; draw();
  text.textContent = 'Display setup cancelled. Nothing was changed.';
  readout.textContent = '';
  await post({cancelled: true});
}
function start() { enterFullscreen(); if (phase === 'intro') loadLines(); }
view.addEventListener('mousedown', (e) => {
  if (phase === 'intro') { start(); return; }
  enterFullscreen();
  if (phase !== 'lines') return;
  const w = window.innerWidth, hit = target(e.clientX, e.clientY);
  if (!hit) return;
  selected = hit === 'tracker' ? 'both' : hit;
  drag = {kind: hit, dl: left * w - e.clientX, dr: right * w - e.clientX};
});
window.addEventListener('mousemove', (e) => {
  if (phase !== 'lines') return;
  const w = window.innerWidth;
  if (drag) {
    if (drag.kind === 'left' || drag.kind === 'tracker') setLeft((e.clientX + drag.dl) / w);
    if (drag.kind === 'right' || drag.kind === 'tracker') setRight((e.clientX + drag.dr) / w);
  }
  hover = drag ? drag.kind : target(e.clientX, e.clientY);
  view.style.cursor = hover ? 'ew-resize' : 'default';
  draw();
});
window.addEventListener('mouseup', () => { drag = null; });
document.addEventListener('mouseleave', () => { drag = null; });
done.addEventListener('click', confirm);
document.addEventListener('keydown', (e) => {
  if (e.key === 'Escape') { if (phase === 'lines' || phase === 'intro') cancel(); return; }
  if (phase === 'intro') { start(); return; }
  if (phase !== 'lines') return;
  const step = (e.shiftKey ? 10 : 1) / ((window.devicePixelRatio || 1) * window.innerWidth);
  const k = e.key.toLowerCase();
  if (k === 'l') selected = 'left'; else if (k === 'r') selected = 'right'; else if (k === 'b') selected = 'both';
  else if (e.key === 'Enter') { confirm(); return; }
  else if (e.key === 'ArrowLeft' || e.key === 'ArrowRight') {
    const d = e.key === 'ArrowLeft' ? -step : step;
    if (selected === 'both') { if (d < 0) { setLeft(left + d); setRight(right + d); } else { setRight(right + d); setLeft(left + d); } }
    else if (selected === 'left') setLeft(left + d); else setRight(right + d);
    e.preventDefault();
  } else return;
  draw();
});
window.addEventListener('resize', draw);
document.addEventListener('fullscreenchange', draw);
if (auto) start();
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
