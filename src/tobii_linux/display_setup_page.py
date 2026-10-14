"""The display setup step as a fullscreen web page, served with the standard library.

It follows Tobii's ScreenPlaneSetupControl: a picture of the tracker sits
150 px above the bottom of the screen and two dashed lines run from it down to
the bottom edge. The user lines them up with the two marks on top of the real
tracker below the screen, by dragging a line (moves that line) or the tracker
picture (moves both). The line positions, as fractions of the screen width,
give the monitor width and the tracker offset (display_setup.size_from_tracker_lines).
As a Linux addition the arrow keys nudge the selected line by one device pixel
(Shift: ten), Tab, L and R select a line, Enter confirms and Esc cancels.
The intro asking where the tracker is shows only when configure() says so,
as Tobii shows it only while no monitor is mapped.

Line positions are fractions of the page width, so the page has to cover the
whole monitor: a kiosk window as large as the screen counts, otherwise it asks
for fullscreen on a click and refuses to confirm. Under WSL a Windows browser
reaches the page through mirrored networking at http://localhost:PORT/.
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
#panel{position:absolute;left:8%;right:8%;top:8%;text-align:center}
h1{font-size:clamp(22px,2.4vw,34px);font-weight:normal;margin:0 0 .6em}
#instruction{font-size:clamp(16px,1.6vw,24px);margin:0 0 .5em}
#hint{font-size:clamp(13px,1.1vw,17px);color:#9aa0a6;margin:0}
#readout{margin-top:1.1em;font-size:clamp(13px,1.2vw,18px);color:#bdc1c6;min-height:1.3em}
#notice{margin-top:.6em;font-size:clamp(13px,1.2vw,18px);color:#fdd663;min-height:1.3em}
#buttons{margin-top:1em}
button{font-size:clamp(14px,1.2vw,18px);padding:8px 28px;margin:0 8px;border:0;border-radius:4px;cursor:pointer}
#primary{background:#8ab4f8;color:#202124}
#primary:disabled{background:#5f6368;cursor:default}
#cancel{background:transparent;color:#e8eaed;border:1px solid #5f6368}
</style></head>
<body tabindex="-1"><canvas id="view"></canvas>
<div id="panel"><h1 id="title">Connecting to the eye tracker...</h1>
<p id="instruction"></p><p id="hint"></p><div id="readout"></div><div id="notice"></div>
<div id="buttons" hidden><button id="primary"></button><button id="cancel">Cancel</button></div></div>
<script>
const view = document.getElementById('view'), ctx = view.getContext('2d');
const $ = (id) => document.getElementById(id);
const auto = location.pathname === '/auto';
const BOTTOM_MARGIN = 150, HIT = 24, ART_RATIO = 72.151 / 812.708;
const SELECTED = '#8ab4f8', IDLE = '#bdc1c6';
let config = null, left = 0, right = 1, phase = 'loading', drag = null, selected = 'left', hover = null;

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
function setText(id, text) { if ($(id).textContent !== text) $(id).textContent = text; }
function screenTexts(title, instruction, hint) { setText('title', title); setText('instruction', instruction); setText('hint', hint); }
function showButtons(primary) { $('buttons').hidden = !primary; if (primary) setText('primary', primary); }
function setLeft(v) { left = Math.max(Math.min(v, right - 0.05), 0.02); }
function setRight(v) { right = Math.min(Math.max(v, left + 0.05), 0.98); }
function geometry() {
  const width = config.tracker_width_mm / (right - left);
  return {width: width, height: width / config.aspect, offset: ((left + right) / 2 - 0.5) * width};
}
function tracker() {
  const w = window.innerWidth, h = window.innerHeight, span = (right - left) * config.picture_proportion;
  const tw = span * w, th = tw * ART_RATIO, top = h - th - BOTTOM_MARGIN;
  return {x: ((left + right) / 2) * w - tw / 2, y: top, w: tw, h: th, lineTop: top + th / 4};
}
function drawLine(x, top, isSelected, isHover) {
  const h = window.innerHeight, colour = isSelected ? SELECTED : (isHover ? '#ffffff' : IDLE);
  ctx.strokeStyle = colour; ctx.fillStyle = colour; ctx.lineWidth = isSelected ? 2 : 1;
  if (!isSelected) ctx.setLineDash([5, 5]);
  ctx.beginPath(); ctx.moveTo(x, top); ctx.lineTo(x, h); ctx.stroke(); ctx.setLineDash([]);
  ctx.beginPath(); ctx.moveTo(x - 9, h - 14); ctx.lineTo(x, h); ctx.lineTo(x + 9, h - 14); ctx.closePath(); ctx.fill();
  const y = top + 30;
  ctx.beginPath(); ctx.moveTo(x - 24, y); ctx.lineTo(x - 10, y - 14); ctx.lineTo(x - 10, y + 14); ctx.fill();
  ctx.beginPath(); ctx.moveTo(x + 24, y); ctx.lineTo(x + 10, y - 14); ctx.lineTo(x + 10, y + 14); ctx.fill();
}
function drawTracker(t) {
  const highlighted = hover === 'tracker' || (drag && drag.kind === 'tracker');
  ctx.fillStyle = '#000'; ctx.strokeStyle = highlighted ? SELECTED : '#fff'; ctx.lineWidth = 2;
  ctx.beginPath(); ctx.roundRect(t.x, t.y, t.w, t.h, Math.min(t.h / 2, 12)); ctx.fill(); ctx.stroke();
  const label = 'Eye tracker: drag to move both arrows', inner = (right - left) * window.innerWidth - 60;
  let size = Math.min(18, t.h * 0.3);
  ctx.font = size + 'px sans-serif';
  if (ctx.measureText(label).width > inner) size *= inner / ctx.measureText(label).width;
  if (size < 10) return;
  ctx.font = size + 'px sans-serif'; ctx.fillStyle = highlighted ? SELECTED : '#9aa0a6';
  ctx.textAlign = 'center'; ctx.textBaseline = 'middle';
  ctx.fillText(label, t.x + t.w / 2, t.y + t.h * 0.62);
}
function draw() {
  const dpr = window.devicePixelRatio || 1;
  view.width = Math.round(window.innerWidth * dpr); view.height = Math.round(window.innerHeight * dpr);
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0); ctx.clearRect(0, 0, window.innerWidth, window.innerHeight);
  if (phase !== 'lines') return;
  const t = tracker();
  drawTracker(t);
  for (const side of ['left', 'right'])
    drawLine((side === 'left' ? left : right) * window.innerWidth, t.lineTop, selected === side, hover === side);
  const g = geometry();
  setText('readout', `Screen ${g.width.toFixed(0)} x ${g.height.toFixed(0)} mm (${(Math.hypot(g.width, g.height) / 25.4).toFixed(1)} in), ` +
    (Math.abs(g.offset) < 0.5 ? 'eye tracker centred' :
     `eye tracker ${Math.abs(g.offset).toFixed(0)} mm ${g.offset > 0 ? 'right' : 'left'} of centre`));
  setText('notice', fullscreen() ? '' : 'This page has to fill the screen: press F11 or click anywhere.');
  $('primary').disabled = !fullscreen();
}
function target(x, y) {
  const w = window.innerWidth, t = tracker();
  const near = ['left', 'right'].map((side) => ({side: side, d: Math.abs(x - (side === 'left' ? left : right) * w)}))
    .filter((c) => c.d <= HIT && y >= t.lineTop).sort((a, b) => a.d - b.d);
  if (near.length) return near[0].side;
  if (x >= t.x && x <= t.x + t.w && y >= t.y && y <= t.y + t.h) return 'tracker';
  return null;
}
function enterFullscreen() {
  if (!document.fullscreenElement && document.documentElement.requestFullscreen)
    document.documentElement.requestFullscreen().catch(() => {});
}
async function loadConfig() {
  const v = viewport();
  for (;;) {
    try { return await (await fetch(`/setup?sw=${v.screen_px[0]}&sh=${v.screen_px[1]}`, {cache: 'no-store'})).json(); }
    catch (e) { await new Promise((r) => setTimeout(r, 500)); }
  }
}
function showIntro() {
  phase = 'intro';
  screenTexts('Where is your eye tracker?', 'If it is not attached below this display yet, attach it now.',
              'Then choose This display, or press Enter.');
  showButtons('This display');
}
function showLines() {
  phase = 'lines';
  screenTexts('Line up the arrows with your eye tracker', config.instructions,
              'Drag an arrow, or nudge the blue one with the Left and Right keys (Shift for bigger steps). ' +
              'Tab picks the other arrow. Enter finishes, Esc cancels.');
  showButtons('Done');
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
  phase = 'confirmed'; showButtons(null); draw();
  screenTexts('Saving...', '', '');
  const reply = await post({left: left, right: right});
  screenTexts(reply.message || 'Display setup saved', reply.summary || '', '');
  setText('readout', ''); setText('notice', '');
  followUp(reply);
}
async function followUp(reply) {
  if (reply.next_url) {
    setText('hint', 'Starting the calibration...');
    for (;;) {
      await new Promise(r => setTimeout(r, 1000));
      try { await fetch(reply.next_url, {mode: 'no-cors', cache: 'no-store'}); location.href = reply.next_url; return; }
      catch (e) {}
    }
  }
  for (;;) {
    await new Promise(r => setTimeout(r, 500));
    try { const s = await (await fetch('/status', {cache: 'no-store'})).json(); if (s.message) { setText('hint', s.message); if (s.final) return; } }
    catch (e) { return; }
  }
}
async function cancel() {
  if (phase !== 'lines' && phase !== 'intro') return;
  phase = 'cancelled'; showButtons(null); draw();
  screenTexts('Display setup cancelled', 'Nothing was changed.', '');
  setText('readout', ''); setText('notice', '');
  await post({cancelled: true});
}
function primary() { if (phase === 'intro') { enterFullscreen(); showLines(); } else confirm(); }
view.addEventListener('mousedown', (e) => {
  enterFullscreen();
  if (phase !== 'lines') return;
  const w = window.innerWidth, hit = target(e.clientX, e.clientY);
  if (!hit) return;
  if (hit !== 'tracker') selected = hit;
  drag = {kind: hit, dl: left * w - e.clientX, dr: right * w - e.clientX};
  draw();
});
window.addEventListener('mousemove', (e) => {
  if (phase !== 'lines') return;
  const w = window.innerWidth;
  if (drag) {
    if (drag.kind === 'tracker') {
      const shift = Math.min(Math.max((e.clientX + drag.dl) / w, 0.02), 0.98 - (right - left)) - left;
      left += shift; right += shift;
    } else if (drag.kind === 'left') setLeft((e.clientX + drag.dl) / w);
    else setRight((e.clientX + drag.dr) / w);
  }
  hover = drag ? drag.kind : target(e.clientX, e.clientY);
  view.style.cursor = hover ? 'ew-resize' : 'default';
  draw();
});
window.addEventListener('mouseup', () => { drag = null; draw(); });
document.addEventListener('mouseleave', () => { drag = null; });
$('primary').addEventListener('click', (e) => { e.stopPropagation(); primary(); });
$('cancel').addEventListener('click', (e) => { e.stopPropagation(); cancel(); });
document.addEventListener('keydown', (e) => {
  if (e.key === 'Escape') { cancel(); return; }
  if (e.key === 'Enter') { e.preventDefault(); primary(); return; }
  if (phase !== 'lines') return;
  const k = e.key.toLowerCase();
  if (e.key === 'Tab') { selected = selected === 'left' ? 'right' : 'left'; e.preventDefault(); }
  else if (k === 'l') selected = 'left'; else if (k === 'r') selected = 'right';
  else if (e.key === 'ArrowLeft' || e.key === 'ArrowRight') {
    const step = (e.shiftKey ? 10 : 1) / ((window.devicePixelRatio || 1) * window.innerWidth);
    const d = e.key === 'ArrowLeft' ? -step : step;
    if (selected === 'left') setLeft(left + d); else setRight(right + d);
    e.preventDefault();
  } else return;
  draw();
});
window.addEventListener('resize', draw);
document.addEventListener('fullscreenchange', draw);
window.focus(); document.body.focus();
loadConfig().then((c) => { config = c; if (c.intro && !auto) showIntro(); else showLines(); });
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
