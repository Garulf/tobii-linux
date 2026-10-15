'use strict';
// The setup page: draws the screen the flow publishes on /state and reports what the user does on /event.
// Screens, texts and timings follow Tobii's configuration app; docs/setup-flow.md lists the sources.

const $ = (id) => document.getElementById(id);
const POLL_MS = 30;
const SECTIONS = {connecting: 'connecting', busy: 'busy', moving: 'busy', display: 'display', 'display-save': 'display-save',
                  'aspect-warning': 'aspect-warning', elsewhere: 'finished',
                  positioning: 'positioning', calibration: 'calibration', 'calibration-failed': 'calibration-failed',
                  profile: 'profile', finished: 'finished', cancelled: 'finished', error: 'finished'};
const NOT_CANCELLABLE = new Set(['finished', 'cancelled', 'error', 'elsewhere']);
const NEEDS_FULLSCREEN = new Set(['display', 'positioning', 'calibration']);

// The monitor this page was opened for (?monitor=KEY) when the setup can move between monitors.
const MY_MONITOR = new URLSearchParams(location.search).get('monitor');

let current = {seq: -1, name: null};
let live = {};
let retired = false;

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
    await fetch('/event', {method: 'POST', body: JSON.stringify({...event, monitor: MY_MONITOR, view: viewReport()})});
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
  if (current.name === 'display') alignment.draw();
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

// Display setup: Tobii's alignment screen (alignment.js) ------------------------------------------

const alignment = new TrackerAlignment($('display'), {
  canFinish: fullscreen,
  onDone: (left, right) => act('done', {left: left, right: right}),
  onThisDisplay: enterFullscreen,
  onMove: (direction) => act(direction),
});

window.addEventListener('mousemove', () => { if (current.name === 'positioning') updateCursor(); });
$('display-save-yes').addEventListener('click', () => act('save'));
$('display-save-no').addEventListener('click', () => act('skip'));
$('aspect-yes').addEventListener('click', () => act('yes'));
$('aspect-no').addEventListener('click', () => act('no'));
$('aspect-close').addEventListener('click', () => act('no'));

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
  moving: (s) => { setText('busy-title', ''); ack(s.seq); },
  'aspect-warning': (s) => { $('aspect-yes').focus(); ack(s.seq); },
  display: (s) => { alignment.show(s); ack(s.seq); },
  'display-save': (s) => { setText('display-save-summary', s.summary || ''); $('display-save-yes').focus(); ack(s.seq); },
  positioning: renderPositioning,
  calibration: renderCalibration,
  'calibration-failed': (s) => { $('retry').focus(); ack(s.seq); },
  profile: (s) => { renderProfile(s); ack(s.seq); },
  finished: (s) => { renderEnd(s); ack(s.seq); },
  cancelled: (s) => { renderEnd(s); ack(s.seq); },
  error: (s) => { renderEnd(s); ack(s.seq); },
};

// Tobii closes the setup window and opens it again on the chosen monitor; here the page opened for the
// other monitor takes over, and this one closes once the flow shows a screen placed there.
function retire() {
  retired = true;
  showSection('elsewhere');
  setText('finished-title', 'The setup continues on the other display.');
  setText('finished-text', '');
  $('finished-close').hidden = true;
  window.close();
}

function render(screenState, previous) {
  if (screenState.monitor && screenState.monitor !== MY_MONITOR) { retire(); return; }
  if (screenState.name !== previous.name) showSection(screenState.name);
  updateCursor();
  const renderer = RENDERERS[screenState.name];
  if (renderer) renderer(screenState, previous); else ack(screenState.seq);
}

// Input -----------------------------------------------------------------------------------------

document.addEventListener('keydown', (event) => {
  if (event.key === 'Escape') { if (!NOT_CANCELLABLE.has(current.name)) act('cancel'); return; }
  if (current.name === 'display') { alignment.handleKey(event); return; }
  if (current.name === 'positioning') { enterFullscreen(); act('continue'); return; }
  if (event.key !== 'Enter') return;
  if (current.name === 'calibration-failed') act('retry');
  else if (current.name === 'display-save') act('save');
  else if (current.name === 'aspect-warning') act('yes');
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
  if (!retired) setTimeout(poll, POLL_MS);
}

makeSpinners();
window.focus();
document.body.focus();
send({seq: -1});
poll();
