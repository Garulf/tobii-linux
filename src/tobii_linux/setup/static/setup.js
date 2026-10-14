'use strict';
// The setup page: draws the screen the flow publishes on /state and reports what the user does on /event.
// Screens, texts and timings follow Tobii's configuration app; docs/setup-flow.md lists the sources.

const $ = (id) => document.getElementById(id);
const POLL_MS = 30;
const SECTIONS = {connecting: 'connecting', busy: 'busy', display: 'display', 'display-save': 'display-save',
                  positioning: 'positioning', calibration: 'calibration', 'calibration-failed': 'calibration-failed',
                  profile: 'profile', finished: 'finished', cancelled: 'finished', error: 'finished'};
const NOT_CANCELLABLE = new Set(['finished', 'cancelled', 'error']);
const NEEDS_FULLSCREEN = new Set(['display', 'positioning', 'calibration']);
const COLOURS = {line: '#afc4cd', highlight: '#ffffff', body: '#000000'};

let current = {seq: -1, name: null};
let live = {};

// Page state and the server ------------------------------------------------------------------

function fullscreen() {
  return document.fullscreenElement != null ||
    (window.innerWidth >= screen.width - 2 && window.innerHeight >= screen.height - 2);
}

function viewReport() {
  const dpr = window.devicePixelRatio || 1;
  return {width: window.innerWidth, height: window.innerHeight, device_pixel_ratio: dpr,
          screen_width: screen.width, screen_height: screen.height,
          screen_px: [Math.round(screen.width * dpr), Math.round(screen.height * dpr)], fullscreen: fullscreen()};
}

async function send(event) {
  try {
    await fetch('/event', {method: 'POST', body: JSON.stringify({...event, view: viewReport()})});
  } catch (error) { /* the next poll shows whether the server is still there */ }
}

const ack = (seq) => send({seq: seq, ack: true});
const act = (action, extra = {}) => send({seq: current.seq, action: action, ...extra});
const later = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

function setText(id, text) {
  const element = $(id);
  if (element.textContent !== text) element.textContent = text;
}

function enterFullscreen() {
  if (!document.fullscreenElement && document.documentElement.requestFullscreen)
    document.documentElement.requestFullscreen().catch(() => {});
}

function fade(element, keyframes, timing) {
  return element.animate(keyframes, {fill: 'forwards', ...timing});
}

function showSection(name) {
  const id = SECTIONS[name];
  for (const section of document.querySelectorAll('.screen')) section.classList.toggle('active', section.id === id);
  $('close').hidden = NOT_CANCELLABLE.has(name) || name === 'calibration';
  document.body.classList.toggle('calibrating', name === 'calibration');
  updateFullscreenBanner();
}

function updateFullscreenBanner() {
  $('fullscreen-banner').hidden = fullscreen() || !NEEDS_FULLSCREEN.has(current.name);
  if (current.name === 'display') drawDisplay();
}

// Cursor: hidden during the calibration, and after 2 s without movement while positioning.
let cursorTimer = null;
function updateCursor() {
  clearTimeout(cursorTimer);
  document.body.classList.toggle('cursor-hidden', current.name === 'calibration');
  if (current.name === 'positioning')
    cursorTimer = setTimeout(() => document.body.classList.add('cursor-hidden'), 2000);
}

function makeSpinners() {
  for (const spinner of document.querySelectorAll('.spinner')) {
    for (let i = 0; i < 12; i++) {
      const dot = document.createElement('i');
      dot.style.transform = `rotate(${i * 30}deg)`;
      dot.style.animationDelay = `${(i * 0.1 - 1.2).toFixed(1)}s`;
      spinner.appendChild(dot);
    }
  }
}

// Display setup -------------------------------------------------------------------------------

const BOTTOM_MARGIN = 150, HANDLE_HALF_WIDTH = 30, TRACKER_RATIO = 72.151 / 812.708;
const MARKS = [[150.8, 145.8], [407.37, 407.37], [662.2, 668.2]];
const display = {config: null, left: 0, right: 1, phase: 'intro', selected: 'left', hover: null, drag: null,
                 intro: null};

function setLeft(value) { display.left = Math.max(Math.min(value, display.right - 0.05), 0.02); }
function setRight(value) { display.right = Math.min(Math.max(value, display.left + 0.05), 0.98); }

function trackerRect(left = display.left, right = display.right) {
  const w = window.innerWidth, h = window.innerHeight;
  const tw = (right - left) * display.config.picture_proportion * w, th = tw * TRACKER_RATIO;
  const top = h - th - BOTTOM_MARGIN;
  return {x: ((left + right) / 2) * w - tw / 2, y: top, w: tw, h: th, lineTop: top + th / 4};
}

function drawTracker(ctx, r, alpha = 1) {
  ctx.save();
  ctx.globalAlpha = alpha;
  ctx.strokeStyle = COLOURS.highlight; ctx.fillStyle = COLOURS.body; ctx.lineWidth = 2;
  const bodyTop = r.y + r.h / 4;
  ctx.beginPath(); ctx.roundRect(r.x, bodyTop, r.w, r.h * 0.75, Math.min(r.h * 0.3, 14)); ctx.fill(); ctx.stroke();
  for (const [topX, bottomX] of MARKS) {
    ctx.beginPath();
    ctx.moveTo(r.x + r.w * topX / 812.708, r.y);
    ctx.lineTo(r.x + r.w * bottomX / 812.708, bodyTop);
    ctx.stroke();
  }
  ctx.restore();
}

function drawLine(ctx, x, top, side) {
  const h = window.innerHeight, selected = display.selected === side;
  const colour = selected || display.hover === side ? COLOURS.highlight : COLOURS.line;
  ctx.strokeStyle = colour; ctx.lineWidth = selected ? 2 : 1;
  if (!selected) ctx.setLineDash([5, 5]);
  ctx.beginPath(); ctx.moveTo(x, top); ctx.lineTo(x, h - 1); ctx.stroke(); ctx.setLineDash([]);
  ctx.lineWidth = selected ? 2 : 1;
  ctx.beginPath(); ctx.moveTo(x - 6, h - 11); ctx.lineTo(x, h - 1); ctx.lineTo(x + 6, h - 11); ctx.stroke();
  const middle = (top + h) / 2;
  ctx.fillStyle = COLOURS.highlight;
  ctx.beginPath(); ctx.moveTo(x - 30, middle); ctx.lineTo(x - 16, middle - 14); ctx.lineTo(x - 16, middle + 14); ctx.fill();
  ctx.beginPath(); ctx.moveTo(x + 30, middle); ctx.lineTo(x + 16, middle - 14); ctx.lineTo(x + 16, middle + 14); ctx.fill();
  if (selected) {
    ctx.fillStyle = 'rgba(255, 255, 255, 0.07)';
    ctx.fillRect(x - HANDLE_HALF_WIDTH, top, HANDLE_HALF_WIDTH * 2, h - top);
  }
}

function drawMonitorIcon(ctx, alpha) {
  const w = 400, h = w * 109.5 / 143, x = (window.innerWidth - w) / 2, y = window.innerHeight * 0.54;
  ctx.save();
  ctx.globalAlpha = alpha;
  ctx.fillStyle = COLOURS.line;
  ctx.beginPath(); ctx.roundRect(x, y, w, h * 0.78, 8); ctx.fill();
  ctx.fillStyle = '#000'; ctx.fillRect(x + 10, y + 10, w - 20, h * 0.78 - 32);
  ctx.fillStyle = COLOURS.line;
  ctx.fillRect(x + w / 2 - 25, y + h * 0.78, 50, h * 0.14);
  ctx.beginPath(); ctx.roundRect(x + w / 2 - 80, y + h * 0.92, 160, h * 0.08, 4); ctx.fill();
  ctx.restore();
  return {x: x + (w - 250) / 2, y: y + h * 0.78 - 22 - 250 * TRACKER_RATIO / 2, w: 250, h: 250 * TRACKER_RATIO};
}

function drawDisplay() {
  const canvas = $('display-canvas'), ctx = canvas.getContext('2d'), dpr = window.devicePixelRatio || 1;
  canvas.width = Math.round(window.innerWidth * dpr); canvas.height = Math.round(window.innerHeight * dpr);
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.clearRect(0, 0, window.innerWidth, window.innerHeight);
  if (!display.config) return;
  if (display.phase === 'intro' || display.phase === 'moving') {
    const progress = display.phase === 'moving' ? display.intro : 0;
    const flat = drawMonitorIcon(ctx, 1 - progress), target = trackerRect();
    const ease = progress < 0.5 ? 2 * progress * progress : 1 - Math.pow(-2 * progress + 2, 2) / 2;
    const mix = (a, b) => a + (b - a) * ease;
    drawTracker(ctx, {x: mix(flat.x, target.x), y: mix(flat.y, target.y), w: mix(flat.w, target.w),
                      h: mix(flat.h, target.h)});
    return;
  }
  const r = trackerRect();
  drawTracker(ctx, r);
  drawLine(ctx, display.left * window.innerWidth, r.lineTop, 'left');
  drawLine(ctx, display.right * window.innerWidth, r.lineTop, 'right');
  const config = display.config, width = config.tracker_width_mm / (display.right - display.left);
  const height = width / config.aspect, offset = ((display.left + display.right) / 2 - 0.5) * width;
  const diagonal = Math.hypot(width, height) / 25.4;
  setText('display-readout', `Screen ${width.toFixed(0)} x ${height.toFixed(0)} mm (${diagonal.toFixed(1)} in), ` +
    (Math.abs(offset) < 0.5 ? 'eye tracker centred' :
      `eye tracker ${Math.abs(offset).toFixed(0)} mm ${offset > 0 ? 'right' : 'left'} of centre`));
  $('display-done').disabled = !fullscreen() || display.phase !== 'lines';
}

function lineAt(x, y) {
  if (display.phase !== 'lines') return null;
  const w = window.innerWidth, r = trackerRect();
  const near = ['left', 'right']
    .map((side) => ({side: side, distance: Math.abs(x - display[side] * w)}))
    .filter((candidate) => candidate.distance <= HANDLE_HALF_WIDTH && y >= r.lineTop)
    .sort((a, b) => a.distance - b.distance);
  if (near.length) return near[0].side;
  return x >= r.x && x <= r.x + r.w && y >= r.y && y <= r.y + r.h ? 'tracker' : null;
}

function slideLinesIn() {
  display.phase = 'sliding';
  const l0 = display.config.left, r0 = display.config.right, start = performance.now();
  const backEase = (t) => {
    const c = 1.70158 * 1.525;
    return t < 0.5 ? (Math.pow(2 * t, 2) * ((c + 1) * 2 * t - c)) / 2
                   : (Math.pow(2 * t - 2, 2) * ((c + 1) * (t * 2 - 2) + c) + 2) / 2;
  };
  function frame(now) {
    const t = Math.min(1, (now - start) / 1500), e = backEase(t);
    display.left = l0 - 0.01 * (1 - e); display.right = r0 + 0.01 * (1 - e);
    if (t < 1) { drawDisplay(); requestAnimationFrame(frame); return; }
    display.left = l0; display.right = r0; display.phase = 'lines'; drawDisplay();
  }
  requestAnimationFrame(frame);
}

function showLines() {
  $('display-intro').hidden = true; $('display-lines').hidden = false;
  $('this-display').hidden = true; $('display-done').hidden = false;
  for (const element of [$('display-lines'), $('display-done')])
    fade(element, [{opacity: 0}, {opacity: 1}], {delay: 500, duration: 1000});
  slideLinesIn();
}

function chooseThisDisplay() {
  if (display.phase !== 'intro') return;
  enterFullscreen();
  $('this-display').disabled = true;
  display.phase = 'moving';
  const start = performance.now();
  function frame(now) {
    display.intro = Math.min(1, (now - start) / 1000);
    drawDisplay();
    if (display.intro < 1) requestAnimationFrame(frame); else showLines();
  }
  requestAnimationFrame(frame);
}

function renderDisplay(screenState) {
  display.config = screenState;
  display.left = screenState.left; display.right = screenState.right;
  display.selected = 'left'; display.hover = null; display.drag = null;
  setText('display-instruction', screenState.instructions);
  $('this-display').disabled = false;
  if (screenState.intro) {
    display.phase = 'intro';
    $('display-intro').hidden = false; $('display-lines').hidden = true;
    $('this-display').hidden = false; $('display-done').hidden = true;
    fade($('display-intro'), [{opacity: 0}, {opacity: 1}], {delay: 500, duration: 1000});
    drawDisplay();
  } else {
    $('display-intro').hidden = true;
    showLines();
  }
}

function confirmLines() {
  if (display.phase !== 'lines' || !fullscreen()) return;
  display.phase = 'confirmed';
  $('display-done').disabled = true;
  act('done', {left: display.left, right: display.right});
}

function nudgeSelected(direction, large) {
  const step = (large ? 10 : 1) / ((window.devicePixelRatio || 1) * window.innerWidth) * direction;
  if (display.selected === 'left') setLeft(display.left + step); else setRight(display.right + step);
  drawDisplay();
}

function displayKey(event) {
  if (event.key === 'Enter') {
    event.preventDefault();
    if (display.phase === 'intro') chooseThisDisplay(); else confirmLines();
    return;
  }
  if (display.phase !== 'lines') return;
  const key = event.key.toLowerCase();
  if (event.key === 'Tab') { display.selected = display.selected === 'left' ? 'right' : 'left'; event.preventDefault(); }
  else if (key === 'l') display.selected = 'left';
  else if (key === 'r') display.selected = 'right';
  else if (event.key === 'ArrowLeft' || event.key === 'ArrowRight') {
    nudgeSelected(event.key === 'ArrowLeft' ? -1 : 1, event.shiftKey);
    event.preventDefault();
    return;
  } else return;
  drawDisplay();
}

$('display-canvas').addEventListener('mousedown', (event) => {
  enterFullscreen();
  const hit = lineAt(event.clientX, event.clientY);
  if (!hit) return;
  if (hit !== 'tracker') display.selected = hit;
  const w = window.innerWidth;
  display.drag = {kind: hit, toLeft: display.left * w - event.clientX, toRight: display.right * w - event.clientX};
  drawDisplay();
});

window.addEventListener('mousemove', (event) => {
  if (current.name === 'positioning') updateCursor();
  if (current.name !== 'display' || !display.config) return;
  const w = window.innerWidth, drag = display.drag;
  if (drag && display.phase === 'lines') {
    if (drag.kind === 'tracker') {
      const span = display.right - display.left;
      display.left = Math.min(Math.max((event.clientX + drag.toLeft) / w, 0.02), 0.98 - span);
      display.right = display.left + span;
    } else if (drag.kind === 'left') setLeft((event.clientX + drag.toLeft) / w);
    else setRight((event.clientX + drag.toRight) / w);
  }
  display.hover = drag ? drag.kind : lineAt(event.clientX, event.clientY);
  $('display-canvas').style.cursor = display.hover ? 'ew-resize' : 'default';
  drawDisplay();
});

window.addEventListener('mouseup', () => { if (display.drag) { display.drag = null; drawDisplay(); } });
document.addEventListener('mouseleave', () => { display.drag = null; });
$('this-display').addEventListener('click', chooseThisDisplay);
$('display-done').addEventListener('click', confirmLines);
$('display-save-yes').addEventListener('click', () => act('save'));
$('display-save-no').addEventListener('click', () => act('skip'));

// Eye positioning -----------------------------------------------------------------------------

const positioning = {start: 0, leaving: false, box: {w: 0, h: 0}};

function layoutTrackbox() {
  const h = Math.floor(Math.min(window.innerWidth, window.innerHeight) / 2.25), w = Math.floor(h * 1.618);
  const box = $('trackbox');
  box.style.width = w + 'px'; box.style.height = h + 'px';
  box.style.marginLeft = -w / 2 + 'px'; box.style.marginTop = -h / 2 + 'px';
  positioning.box = {w: w, h: h};
}

function renderPositioning(screenState, previous) {
  layoutTrackbox();
  $('positioning-upright').hidden = !screenState.sit_upright;
  if (screenState.leaving) { leavePositioning(screenState.seq); return; }
  if (previous.name === 'positioning') { ack(screenState.seq); return; }
  positioning.start = performance.now();
  positioning.leaving = false;
  $('trackbox').style.opacity = 1;
  fade($('welcome-one'), [{opacity: 0}, {opacity: 0, offset: 1 / 6}, {opacity: 1, offset: 1 / 2},
                          {opacity: 1, offset: 2 / 3}, {opacity: 0}], {duration: 3000});
  fade($('welcome-two'), [{opacity: 0}, {opacity: 1, offset: 1 / 3}, {opacity: 1, offset: 2 / 3}, {opacity: 0}],
       {delay: 4000, duration: 3000});
  fade($('positioning-hint'), [{opacity: 0}, {opacity: 1}], {delay: 7000, duration: 1000});
  updateCursor();
  ack(screenState.seq);
}

function eyeStyle(element, eye, size) {
  if (!eye) { element.style.visibility = 'hidden'; return; }
  const scale = positioning.box.h / 640, diameter = Math.max(4, size * scale);
  const sinceStart = (performance.now() - positioning.start) / 1000;
  const reveal = Math.min(1, Math.max(0, sinceStart - 3));
  element.style.visibility = 'visible';
  element.style.width = element.style.height = diameter + 'px';
  element.style.left = eye.x * positioning.box.w - diameter / 2 + 'px';
  element.style.top = eye.y * positioning.box.h - diameter / 2 + 'px';
  element.style.background = `rgb(${eye.grey}, ${eye.grey}, ${eye.grey})`;
  element.style.opacity = eye.opacity * reveal;
}

function renderPositioningLive() {
  const eyes = live.eyes;
  if (!eyes || positioning.leaving) return;
  eyeStyle($('eye-left'), eyes.left, eyes.size);
  eyeStyle($('eye-right'), eyes.right, eyes.size);
  setText('positioning-hint', eyes.hint || '');
  const sinceStart = performance.now() - positioning.start;
  let status = '';
  if (eyes.good && sinceStart > 3000) status = 'Good. Stay like this, the calibration starts in a moment, or press any key.';
  setText('positioning-status', status);
}

async function leavePositioning(seq) {
  positioning.leaving = true;
  setText('positioning-status', '');
  $('eye-right').animate([{transform: 'scaleY(0)'}, {transform: 'scaleY(1)'}], {duration: 400});
  await later(400);
  await fade($('trackbox'), [{opacity: 1}, {opacity: 0}], {duration: 1000}).finished;
  ack(seq);
}

// Calibration ---------------------------------------------------------------------------------

const PARTICLES = 20;
const calibration = {targets: new Map(), spinnerTimer: null};

function randomBetween(low, high) { return low + Math.random() * (high - low); }

function makeTarget(index, x, y) {
  const element = document.createElement('div');
  element.className = 'target';
  element.style.left = x * window.innerWidth + 'px';
  element.style.top = y * window.innerHeight + 'px';
  const particles = [];
  for (let i = 0; i < PARTICLES; i++) {
    const size = i === 0 ? 18 : randomBetween(4, 18), particle = document.createElement('div');
    particle.className = 'particle';
    Object.assign(particle.style, {width: size + 'px', height: size + 'px', left: -size / 2 + 'px', top: -size / 2 + 'px'});
    const radius = Math.sqrt(Math.random()) * randomBetween(40, 70), angle = Math.random() * 2 * Math.PI;
    particles.push({element: particle, offset: [Math.cos(angle) * radius, Math.sin(angle) * radius],
                    fadeDelay: Math.floor(Math.random() * 400), spin: null});
    element.appendChild(particle);
  }
  $('targets').appendChild(element);
  const target = {element: element, particles: particles, x: x, y: y};
  calibration.targets.set(index, target);
  return target;
}

function spawn(target, duration) {
  const dx = window.innerWidth / 2 - target.x * window.innerWidth, dy = window.innerHeight / 2 - target.y * window.innerHeight;
  const motion = fade(target.element, [{transform: `translate(${dx}px, ${dy}px) scale(2)`},
                                       {transform: 'translate(0, 0) scale(1)'}],
                      {duration: duration, easing: 'cubic-bezier(0.65, 0, 0.35, 1)'});
  fade(target.element, [{opacity: 0}, {opacity: 1}], {duration: duration, easing: 'cubic-bezier(0.83, 0, 0.17, 1)'});
  return motion.finished;
}

function focusTarget(target, focused) {
  for (const particle of target.particles) {
    if (particle.spin) { particle.spin.cancel(); particle.spin = null; }
    if (focused) {
      fade(particle.element, [{transform: 'rotate(-1440deg) scale(1, 1)'}, {transform: 'rotate(0deg) scale(0.5, 0.8)'}],
           {duration: 1000, easing: 'cubic-bezier(0.55, 0.085, 0.68, 0.53)'});
      particle.spin = particle.element.animate(
        [{transform: 'rotate(0deg) scale(0.5, 0.8)'}, {transform: 'rotate(3240deg) scale(0.5, 0.8)'}],
        {duration: 2000, delay: 1000, iterations: Infinity});
    } else {
      fade(particle.element, [{transform: 'rotate(0deg) scale(1, 1)'}], {duration: 200});
    }
  }
}

function explode(index) {
  const target = calibration.targets.get(index);
  if (!target) return;
  calibration.targets.delete(index);
  let longest = 0;
  for (const particle of target.particles) {
    if (particle.spin) particle.spin.cancel();
    const [ox, oy] = particle.offset, travel = 500 + 0.5 * particle.fadeDelay;
    fade(particle.element, [{transform: 'translate(0, 0) scale(0.5)'},
                            {transform: `translate(${ox * 1.5}px, ${oy * 1.5}px) scale(0.5)`}],
         {duration: travel, easing: 'cubic-bezier(0.075, 0.82, 0.165, 1)'});
    fade(particle.element, [{opacity: 1}, {opacity: 0}], {delay: particle.fadeDelay, duration: 400});
    longest = Math.max(longest, travel, particle.fadeDelay + 400);
  }
  setTimeout(() => target.element.remove(), longest + 50);
}

function clearTargets() {
  calibration.targets.clear();
  $('targets').replaceChildren();
}

async function calibrationIntro(screenState) {
  clearTargets();
  $('calibration-spinner').hidden = true;
  if (screenState.welcome) {
    await fade($('calibration-welcome'), [{opacity: 0}, {opacity: 1, offset: 0.2}, {opacity: 1, offset: 0.8}, {opacity: 0}],
               {duration: 2500}).finished;
  }
  const hold = screenState.again ? 3000 : 2000;
  fade($('calibration-instruction'), [{opacity: 0}, {opacity: 1, offset: 500 / (hold + 500)},
                                      {opacity: 1, offset: hold / (hold + 500)}, {opacity: 0}],
       {duration: hold + 500});
  const dotStart = hold - 1250;
  fade($('centre-dot'), [{transform: 'scale(0)'}, {transform: 'scale(1)', offset: 0.6,
                                                   easing: 'cubic-bezier(0.34, 1.56, 0.64, 1)'},
                         {transform: 'scale(1)', offset: 0.99}, {transform: 'scale(0)'}],
       {delay: dotStart, duration: 2500});
  await later(dotStart);
  ack(screenState.seq);
}

async function renderCalibration(screenState) {
  const phase = screenState.phase;
  if (phase === 'intro') { calibrationIntro(screenState); return; }
  if (phase === 'targets') {
    const duration = screenState.targets.length === 1 ? 1500 : 1000;
    const arrivals = screenState.targets.map((t) => spawn(makeTarget(t.index, t.x, t.y), duration));
    await Promise.all(arrivals);
  } else if (phase === 'focus') {
    const target = calibration.targets.get(screenState.index);
    if (target) focusTarget(target, screenState.focused);
  } else if (phase === 'missed') {
    const target = calibration.targets.get(screenState.index);
    if (target) focusTarget(target, false);
  } else if (phase === 'pop') {
    explode(screenState.index);
  } else if (phase === 'computing') {
    calibration.spinnerTimer = setTimeout(() => { $('calibration-spinner').hidden = false; }, 500);
  } else if (phase === 'computed') {
    clearTimeout(calibration.spinnerTimer);
    $('calibration-spinner').hidden = true;
    await later(screenState.stage === 0 ? 500 : 700);
  } else if (phase === 'restart') {
    await fade($('targets'), [{opacity: 1}, {opacity: 0}], {duration: 200}).finished;
    clearTargets();
    await later(900);
    $('targets').getAnimations().forEach((animation) => animation.cancel());
  }
  ack(screenState.seq);
}

// Other screens -------------------------------------------------------------------------------

function renderProfile(screenState) {
  const input = $('profile-name');
  if (input.value !== screenState.suggested || !screenState.problem) input.value = screenState.suggested || '';
  setText('profile-problem', screenState.problem || '');
  input.focus();
  input.select();
}

function saveProfile() { act('save', {name: $('profile-name').value}); }

function renderEnd(screenState) {
  setText('finished-title', screenState.title || '');
  setText('finished-text', screenState.text || '');
  setText('finished-hint', '');
  $('finished-close').focus();
}

function closePage() {
  window.close();
  setTimeout(() => setText('finished-hint', 'You can close this window now (Alt+F4).'), 300);
}

$('retry').addEventListener('click', () => act('retry'));
$('failed-cancel').addEventListener('click', () => act('cancel'));
$('profile-save').addEventListener('click', saveProfile);
$('finished-close').addEventListener('click', closePage);
$('close').addEventListener('click', (event) => { event.stopPropagation(); act('cancel'); });

const RENDERERS = {
  connecting: (s) => ack(s.seq),
  busy: (s) => { setText('busy-title', s.title || ''); ack(s.seq); },
  display: (s) => { renderDisplay(s); ack(s.seq); },
  'display-save': (s) => { setText('display-save-summary', s.summary || ''); $('display-save-yes').focus(); ack(s.seq); },
  positioning: renderPositioning,
  calibration: renderCalibration,
  'calibration-failed': (s) => { $('retry').focus(); ack(s.seq); },
  profile: (s) => { renderProfile(s); ack(s.seq); },
  finished: (s) => { renderEnd(s); ack(s.seq); },
  cancelled: (s) => { renderEnd(s); ack(s.seq); },
  error: (s) => { renderEnd(s); ack(s.seq); },
};

function render(screenState, previous) {
  if (screenState.name !== previous.name) showSection(screenState.name);
  updateCursor();
  const renderer = RENDERERS[screenState.name];
  if (renderer) renderer(screenState, previous); else ack(screenState.seq);
}

// Input -----------------------------------------------------------------------------------------

document.addEventListener('keydown', (event) => {
  if (event.key === 'Escape') { if (!NOT_CANCELLABLE.has(current.name)) act('cancel'); return; }
  if (current.name === 'display') { displayKey(event); return; }
  if (current.name === 'positioning') { enterFullscreen(); act('continue'); return; }
  if (event.key !== 'Enter') return;
  if (current.name === 'calibration-failed') act('retry');
  else if (current.name === 'display-save') act('save');
  else if (current.name === 'profile') saveProfile();
  else if (NOT_CANCELLABLE.has(current.name)) closePage();
});

document.addEventListener('click', (event) => {
  if (NEEDS_FULLSCREEN.has(current.name)) enterFullscreen();
  if (current.name === 'positioning' && event.target.tagName !== 'BUTTON') act('continue');
});

window.addEventListener('resize', () => { updateFullscreenBanner(); send({seq: -1}); });
document.addEventListener('fullscreenchange', () => { updateFullscreenBanner(); send({seq: -1}); });

// Polling ---------------------------------------------------------------------------------------

async function poll() {
  try {
    const state = await (await fetch('/state', {cache: 'no-store'})).json();
    live = state.live || {};
    if (state.seq !== current.seq) {
      const previous = current;
      current = state;
      render(state, previous);
    }
    if (current.name === 'positioning') renderPositioningLive();
  } catch (error) { /* keep polling: the server may be starting or the flow restarting */ }
  setTimeout(poll, POLL_MS);
}

makeSpinners();
window.focus();
document.body.focus();
send({seq: -1});
poll();
