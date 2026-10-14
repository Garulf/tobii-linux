'use strict';

const HISTORY_MS = 10000;
const STALE_STATUS_MS = 3500;
const GAZE_LOST_MS = 250;
const TRAIL_MS = 900;
const COLORS = { x: '#f87171', y: '#4ade80', z: '#60a5fa', accent: '#38bdf8', warn: '#f59e0b', muted: '#8a96b3', grid: '#1c2742' };

const $ = (id) => document.getElementById(id);

const app = {
  status: null,
  statusAt: 0,
  serverOnline: false,
  frame: null,
  frameImage: null,
  frameSequence: 0,
  pose: null,
  gaze: null,
  view: 'camera',
  poseHistory: [],
  poseMesh: null,
  topology: null,
  gazeHistory: [],
  dot: { x: 0.5, y: 0.5, seen: 0, initialised: false, trail: [] },
  dirty: { camera: true },
  options: { box: true, landmarks: true, mesh: true, mirror: false, smoothingMs: 90, trail: true, raw: false },
};

function fixed(value, digits, unit = '') {
  return value === null || value === undefined || !Number.isFinite(value) ? '-' : value.toFixed(digits) + unit;
}

function signed(value, digits) {
  if (value === null || value === undefined || !Number.isFinite(value)) return '-';
  return (value >= 0 ? '+' : '') + value.toFixed(digits);
}

function point(values, digits = 3) {
  return values ? values.map((v) => v.toFixed(digits)).join(', ') : '-';
}

function setText(id, text) {
  const element = $(id);
  if (element.textContent !== text) element.textContent = text;
}

function prune(history, now) {
  while (history.length && now - history[0].at > HISTORY_MS) history.shift();
}

/* Tabs */

function selectView(name) {
  if (!document.getElementById('view-' + name)) name = 'camera';
  app.view = name;
  for (const button of document.querySelectorAll('.tabs button')) {
    button.classList.toggle('active', button.dataset.view === name);
  }
  for (const section of document.querySelectorAll('.view')) {
    section.classList.toggle('active', section.id === 'view-' + name);
  }
  if (location.hash !== '#' + name) history.replaceState(null, '', '#' + name);
  app.dirty.camera = true;
  resizeCanvases();
}

/* Event stream */

function connect() {
  const events = new EventSource('/events');
  events.onopen = () => { app.serverOnline = true; renderConnection(); };
  events.onerror = () => { app.serverOnline = false; renderConnection(); };
  const on = (topic, handler) => events.addEventListener(topic, (event) => handler(JSON.parse(event.data)));
  on('status', onStatus);
  on('frame', onFrame);
  on('pose', onPose);
  on('gaze', onGaze);
  on('gaze_columns', onGazeColumns);
  on('mesh_topology', (topology) => { app.topology = topology; });
}

function onStatus(status) {
  app.status = status;
  app.statusAt = performance.now();
  app.serverOnline = true;
  renderConnection();
  renderStatus(status);
  renderRates(status);
}

function onFrame(frame) {
  const image = new Image();
  image.onload = () => {
    if (frame.seq < app.frameSequence) return;
    app.frameSequence = frame.seq;
    frame.meshPoints = app.topology ? decodeInt16(frame.mesh, app.topology.pixel_scale) : null;
    app.frame = frame;
    app.frameImage = image;
    app.dirty.camera = true;
    renderTracking(frame);
  };
  image.src = 'data:image/png;base64,' + frame.png;
}

function onPose(pose) {
  const now = performance.now();
  app.pose = pose;
  app.poseMesh = app.topology ? decodeInt16(pose.mesh3d, app.topology.millimetre_scale) : null;
  app.poseHistory.push({ at: now, pose });
  prune(app.poseHistory, now);
  renderPose(pose);
}

function onGaze(gaze) {
  const now = performance.now();
  app.gaze = gaze;
  app.gazeHistory.push({ at: now, gaze });
  prune(app.gazeHistory, now);
  if (gaze.point) app.dot.seen = now;
  renderGaze(gaze);
}

function onGazeColumns(message) {
  if (app.view !== 'gaze') return;
  const body = $('columnsTable').tBodies[0];
  const rows = message.columns.map(([id, name, value]) => {
    const shown = Array.isArray(value) ? value.join(', ') : String(value);
    return `<tr><td>${id}</td><td>${escapeHtml(name)}</td><td>${escapeHtml(shown)}</td></tr>`;
  });
  body.innerHTML = rows.join('');
}

function escapeHtml(text) {
  return text.replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]));
}

/* Connection, banner and status view */

const CONNECTION_PILLS = {
  streaming: ['ok', null], connecting: ['warn', 'Connecting'], starting: ['warn', 'Starting'],
  'no-device': ['bad', 'No tracker'], error: ['bad', 'Error'], stopping: ['idle', 'Stopping'], stopped: ['idle', 'Stopped'],
};

function renderConnection() {
  const pill = $('pill');
  const stale = performance.now() - app.statusAt > STALE_STATUS_MS;
  let tone, label;
  if (!app.serverOnline || (app.status && stale)) {
    [tone, label] = ['bad', 'Server offline'];
  } else if (!app.status) {
    [tone, label] = ['warn', 'Connecting'];
  } else {
    const [pillTone, pillLabel] = CONNECTION_PILLS[app.status.connection] || ['warn', app.status.connection];
    tone = pillTone;
    label = pillLabel || (app.status.mode === 'replay' ? 'Replay' : 'Live');
  }
  pill.className = 'pill ' + tone;
  setText('pill', label);
  renderBanner(stale);
}

function renderBanner(stale) {
  const banner = $('banner');
  let title = null, text = '', warn = false;
  const status = app.status;
  if (!app.serverOnline || (status && stale)) {
    title = 'Lost the demo server.';
    text = 'Retrying. Check that the demo process is still running.';
  } else if (status && status.connection === 'no-device') {
    title = 'No tracker found.';
    text = status.message + ' Retrying every few seconds.';
  } else if (status && status.connection === 'error') {
    title = 'Tracker error.';
    text = status.message + ' Reconnecting.';
  } else if (status && status.head_pose && status.head_pose.state === 'unavailable') {
    title = 'Head pose is unavailable.';
    text = status.head_pose.message;
    warn = true;
  }
  banner.hidden = title === null;
  banner.classList.toggle('warn', warn);
  if (title !== null) {
    setText('bannerTitle', title);
    setText('bannerText', text);
  }
}

function definitionList(id, entries) {
  const list = $(id);
  list.innerHTML = entries.map(([key, value]) =>
    `<dt>${escapeHtml(key)}</dt><dd>${escapeHtml(value === null || value === undefined || value === '' ? '-' : String(value))}</dd>`).join('');
}

function presenceText(presence) {
  return presence ? `${presence.state} (device value ${presence.value})` : 'not reported';
}

function label(key) {
  const text = key.replace(/_/g, ' ');
  return text.charAt(0).toUpperCase() + text.slice(1);
}

function duration(seconds) {
  const s = Math.floor(seconds);
  const h = Math.floor(s / 3600), m = Math.floor((s % 3600) / 60);
  return (h ? h + ' h ' : '') + (h || m ? m + ' min ' : '') + (s % 60) + ' s';
}

function renderStatus(status) {
  definitionList('connectionList', [
    ['State', status.connection], ['Message', status.message],
    ['Mode', status.mode === 'replay' ? 'Replay of recorded data' : 'Live tracker'],
    ['Uptime', duration(status.uptime_s)],
    ['Head pose', status.head_pose.message],
    ['User presence', presenceText(status.presence)],
  ]);
  const device = Object.entries(status.device || {}).map(([key, value]) => [label(key), value]);
  definitionList('deviceList', device.length ? device : [['Device', 'Not connected']]);
  const rates = status.rates, counters = status.counters;
  const rows = [
    ['Camera images (0x501)', fixed(rates.camera, 1, ' fps'), counters.frames],
    ['Head pose processed', fixed(rates.processed, 1, ' fps'), counters.processed],
    ['Dropped frames', '', counters.dropped_frames],
    ['Gaze (0x500)', fixed(rates.gaze, 1, ' Hz'), counters.gaze],
    ['Gaze point valid', fixed(rates.gaze_valid, 1, ' Hz'), counters.gaze_valid],
    ['Gaze dropped by the runtime rules', '', counters.gaze_dropped],
    ['Presence (0x504)', '', counters.presence],
    ['Decode errors', '', counters.decode_errors],
    ['Other stream messages', '', counters.other_stream_messages],
  ];
  $('streamRows').innerHTML = rows.map(([name, rate, total]) =>
    `<tr><td>${name}</td><td class="num">${rate}</td><td class="num">${total}</td></tr>`).join('');
  const area = status.display_area;
  const session = [
    ['Display area', `${area.width_mm} x ${area.height_mm} mm`],
    ['Top left', area.top_left.join(', ')], ['Top right', area.top_right.join(', ')],
    ['Bottom left', area.bottom_left.join(', ')],
    ['Gaze source', `${status.gaze_source.name}: ${status.gaze_source.description}`],
    ['Image updates', fixed(status.image_rate_hz, 1, ' per second')],
    ['Processing time', fixed(status.processing_ms, 1, ' ms')],
    ...Object.entries(status.session || {}).map(([key, value]) =>
      [label(key), Array.isArray(value) ? value.join(', ') : value]),
  ];
  if (status.last_decode_error) session.push(['Last decode error', status.last_decode_error]);
  definitionList('sessionList', session);
  const errors = status.errors || [];
  $('errorList').innerHTML = errors.length ? errors.slice().reverse().map((error) => {
    const time = new Date(error.time * 1000).toLocaleTimeString();
    const repeat = error.count > 1 ? ` (x${error.count})` : '';
    return `<li><time>${time}</time>${escapeHtml(error.message)}${repeat}</li>`;
  }).join('') : '<li class="muted">None</li>';
  setText('gazeSource', status.gaze_source.description);
  setText('gazePresence', presenceText(status.presence));
  if (app.view === 'gaze') fitCanvas($('gazePreview'), previewAspect());
  const headPose = status.head_pose;
  setText('headPoseNote', headPose.state === 'running' ? '' : `Head pose: ${headPose.message}`);
}

function renderRates(status) {
  const rates = status.rates;
  setText('vitalCamera', fixed(rates.camera, 0));
  setText('vitalGaze', fixed(rates.gaze, 0));
  setText('rateCamera', fixed(rates.camera, 1));
  setText('rateProcessed', fixed(rates.processed, 1));
  setText('procMs', fixed(status.processing_ms, 1));
  setText('droppedFrames', String(status.counters.dropped_frames));
  setText('imageRate', fixed(status.image_rate_hz, 0));
  setText('gazeRate', fixed(rates.gaze, 1));
}

/* Camera */

function renderTracking(frame) {
  const state = frame.state || 'unavailable';
  const element = $('trackState');
  element.className = 'state-dot ' + state;
  setText('trackState', frame.state ? state : 'no head pose');
  const tag = $('cameraTag');
  tag.className = 'canvas-tag ' + state;
  setText('cameraTag', frame.state ? state : 'camera only');
  const quality = frame.quality;
  $('qualityBar').style.width = (quality === null ? 0 : Math.max(0, Math.min(1, quality)) * 100) + '%';
  setText('qualityText', fixed(quality, 2));
  setText('gateText', frame.accepted === null ? '-' : frame.accepted ? 'Accepted' : 'Rejected');
}

function drawCamera() {
  const canvas = $('cameraCanvas');
  const ctx = canvas.getContext('2d');
  const size = canvas.width;
  ctx.setTransform(1, 0, 0, 1, 0, 0);
  ctx.fillStyle = '#05080f';
  ctx.fillRect(0, 0, size, size);
  const frame = app.frame, image = app.frameImage;
  if (!frame || !image) {
    ctx.fillStyle = COLORS.muted;
    ctx.font = `${size / 28}px system-ui, sans-serif`;
    ctx.textAlign = 'center';
    ctx.fillText('Waiting for camera images', size / 2, size / 2);
    return;
  }
  if (app.options.mirror) ctx.setTransform(-1, 0, 0, 1, size, 0);
  ctx.imageSmoothingEnabled = true;
  ctx.imageSmoothingQuality = 'high';
  ctx.drawImage(image, 0, 0, size, size);
  const sx = size / frame.width, sy = size / frame.height;
  const tone = frame.state === 'tracking' ? COLORS.accent : COLORS.warn;
  if (app.options.box && frame.face) {
    const [x, y, w, h] = frame.face;
    ctx.strokeStyle = tone;
    ctx.lineWidth = size / 280;
    ctx.setLineDash([size / 60, size / 90]);
    ctx.strokeRect(x * sx, y * sy, w * sx, h * sy);
    ctx.setLineDash([]);
  }
  if (app.options.mesh && frame.meshPoints && app.topology) {
    const points = frame.meshPoints, edges = app.topology.edges;
    const sx2 = size / frame.width, sy2 = size / frame.height;
    ctx.strokeStyle = 'rgba(125, 211, 252, 0.45)';
    ctx.lineWidth = size / 900;
    ctx.beginPath();
    for (let i = 0; i < edges.length; i += 2) {
      const a = edges[i] * 2, b = edges[i + 1] * 2;
      ctx.moveTo(points[a] * sx2, points[a + 1] * sy2);
      ctx.lineTo(points[b] * sx2, points[b + 1] * sy2);
    }
    ctx.stroke();
  }
  if (app.options.landmarks && frame.landmarks) {
    const points = frame.landmarks;
    const radius = size / 190;
    ctx.fillStyle = tone;
    ctx.shadowColor = tone;
    ctx.shadowBlur = size / 90;
    ctx.beginPath();
    for (let i = 0; i < points.length; i += 2) {
      const px = points[i] * size, py = points[i + 1] * size;
      ctx.moveTo(px + radius, py);
      ctx.arc(px, py, radius, 0, Math.PI * 2);
    }
    ctx.fill();
    ctx.shadowBlur = 0;
  }
}

/* Head pose */

function renderPose(pose) {
  const p = pose.position, r = pose.rotation;
  const values = [
    ['poseX', 'miniX', p && p[0], 1], ['poseY', 'miniY', p && p[1], 1], ['poseZ', 'miniZ', p && p[2], 1],
    ['posePitch', 'miniPitch', r && r[0], 1], ['poseYaw', 'miniYaw', r && r[1], 1], ['poseRoll', 'miniRoll', r && r[2], 1],
  ];
  if (app.view === 'pose' || app.view === 'camera') {
    for (const [big, mini, value, digits] of values) {
      setText(big, signed(value, digits));
      setText(mini, signed(value, digits));
    }
  }
  const badge = $('poseBadge');
  badge.className = 'badge ' + (pose.valid ? 'ok' : 'bad');
  setText('poseBadge', pose.valid ? 'valid' : 'no pose');
  const valid = app.poseHistory.filter((entry) => entry.pose.valid).length;
  setText('poseValidShare', app.poseHistory.length ? `${Math.round(100 * valid / app.poseHistory.length)} % of the last 10 s` : '-');
  setText('poseTime', `${(pose.t / 1e6).toFixed(3)} s device time`);
}

const DEFAULT_VIEW = { yaw: 2.25, pitch: 0.28, distance: 1600, target: [0, 20, 380] };
const view = { ...DEFAULT_VIEW };

function decodeInt16(text, scale) {
  if (!text) return null;
  const bytes = Uint8Array.from(atob(text), (c) => c.charCodeAt(0));
  const ints = new Int16Array(bytes.buffer);
  const values = new Float32Array(ints.length);
  for (let i = 0; i < ints.length; i++) values[i] = ints[i] / scale;
  return values;
}

function sceneCamera(size) {
  const { yaw, pitch, distance, target } = view;
  const eye = [target[0] + distance * Math.sin(yaw) * Math.cos(pitch), target[1] + distance * Math.sin(pitch),
    target[2] + distance * Math.cos(yaw) * Math.cos(pitch)];
  const forward = normalize([target[0] - eye[0], target[1] - eye[1], target[2] - eye[2]]);
  const right = normalize(cross(forward, [0, 1, 0]));
  const up = cross(right, forward);
  const focal = size * 1.9;
  return (p) => {
    const d = [p[0] - eye[0], p[1] - eye[1], p[2] - eye[2]];
    const depth = dot(d, forward);
    if (depth < 10) return null;
    return [size / 2 + focal * dot(d, right) / depth, size / 2 - focal * dot(d, up) / depth, depth];
  };
}

function dot(a, b) { return a[0] * b[0] + a[1] * b[1] + a[2] * b[2]; }
function cross(a, b) { return [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]; }
function normalize(v) { const n = Math.hypot(...v) || 1; return [v[0] / n, v[1] / n, v[2] / n]; }

function line(ctx, project, a, b) {
  const pa = project(a), pb = project(b);
  if (!pa || !pb) return;
  ctx.beginPath();
  ctx.moveTo(pa[0], pa[1]);
  ctx.lineTo(pb[0], pb[1]);
  ctx.stroke();
}

function sceneLabel(ctx, project, point, text) {
  const p = project(point);
  if (p) ctx.fillText(text, p[0], p[1]);
}

function drawScene() {
  const canvas = $('scene');
  const ctx = canvas.getContext('2d');
  const size = canvas.width;
  const glow = ctx.createRadialGradient(size / 2, size / 2, size * 0.05, size / 2, size / 2, size * 0.6);
  glow.addColorStop(0, '#101a33');
  glow.addColorStop(1, '#05080f');
  ctx.fillStyle = glow;
  ctx.fillRect(0, 0, size, size);
  const project = sceneCamera(size);
  const topology = app.topology;
  ctx.font = `${size / 52}px system-ui, sans-serif`;
  ctx.textAlign = 'center';
  if (topology) {
    const corners = topology.display;
    const floor = Math.min(...corners.map((c) => c[1])) - 40;
    ctx.strokeStyle = 'rgba(56, 189, 248, 0.07)';
    ctx.lineWidth = 1;
    for (let x = -600; x <= 600; x += 100) line(ctx, project, [x, floor, -100], [x, floor, 1000]);
    for (let z = -100; z <= 1000; z += 100) line(ctx, project, [-600, floor, z], [600, floor, z]);
    const screen = corners.map(project);
    if (screen.every(Boolean)) {
      ctx.fillStyle = 'rgba(56, 189, 248, 0.06)';
      ctx.strokeStyle = 'rgba(148, 197, 255, 0.7)';
      ctx.lineWidth = size / 400;
      ctx.beginPath();
      screen.forEach(([x, y], i) => (i ? ctx.lineTo(x, y) : ctx.moveTo(x, y)));
      ctx.closePath();
      ctx.fill();
      ctx.stroke();
    }
    ctx.fillStyle = COLORS.muted;
    const top = [(corners[0][0] + corners[1][0]) / 2, corners[0][1] + 25, (corners[0][2] + corners[1][2]) / 2];
    sceneLabel(ctx, project, top, 'display');
    const t = topology.tracker;
    ctx.strokeStyle = '#e6ebf5';
    ctx.lineWidth = size / 140;
    line(ctx, project, [t[0] - 140, t[1], t[2]], [t[0] + 140, t[1], t[2]]);
    ctx.fillStyle = COLORS.muted;
    sceneLabel(ctx, project, [t[0], t[1] - 35, t[2]], 'tracker');
  }
  const pose = app.pose;
  const mesh = pose && pose.valid ? app.poseMesh : null;
  if (!topology || !mesh) {
    ctx.fillStyle = COLORS.muted;
    ctx.font = `${size / 34}px system-ui, sans-serif`;
    ctx.fillText(topology ? 'No face' : 'Waiting for the head-pose model', size / 2, size * 0.12);
    drawPositionInset(ctx, size, null);
    return;
  }
  const points = [];
  for (let i = 0; i < mesh.length; i += 3) points.push(project([mesh[i], mesh[i + 1], mesh[i + 2]]));
  const depths = points.filter(Boolean).map((p) => p[2]);
  const near = Math.min(...depths), far = Math.max(...depths);
  const edges = topology.edges;
  ctx.lineWidth = size / 650;
  for (let i = 0; i < edges.length; i += 2) {
    const a = points[edges[i]], b = points[edges[i + 1]];
    if (!a || !b) continue;
    const depth = ((a[2] + b[2]) / 2 - near) / Math.max(1, far - near);
    ctx.strokeStyle = `rgba(125, 211, 252, ${0.95 - 0.6 * depth})`;
    ctx.beginPath();
    ctx.moveTo(a[0], a[1]);
    ctx.lineTo(b[0], b[1]);
    ctx.stroke();
  }
  const t = topology.tracker, p = pose.position;
  ctx.setLineDash([size / 120, size / 120]);
  ctx.strokeStyle = 'rgba(245, 158, 11, 0.6)';
  ctx.lineWidth = size / 500;
  line(ctx, project, t, p);
  ctx.setLineDash([]);
  ctx.fillStyle = COLORS.warn;
  const middle = [(t[0] + p[0]) / 2, (t[1] + p[1]) / 2, (t[2] + p[2]) / 2];
  sceneLabel(ctx, project, middle, `${Math.hypot(p[0] - t[0], p[1] - t[1], p[2] - t[2]).toFixed(0)} mm`);
  drawPositionInset(ctx, size, p);
}

function bindScene() {
  const canvas = $('scene');
  let drag = null;
  canvas.addEventListener('pointerdown', (event) => {
    drag = { x: event.clientX, y: event.clientY, yaw: view.yaw, pitch: view.pitch };
    canvas.setPointerCapture(event.pointerId);
  });
  canvas.addEventListener('pointermove', (event) => {
    if (!drag) return;
    view.yaw = drag.yaw - (event.clientX - drag.x) * 0.008;
    view.pitch = Math.max(-1.2, Math.min(1.4, drag.pitch + (event.clientY - drag.y) * 0.008));
  });
  canvas.addEventListener('pointerup', () => { drag = null; });
  canvas.addEventListener('wheel', (event) => {
    event.preventDefault();
    view.distance = Math.max(600, Math.min(5000, view.distance * Math.exp(event.deltaY * 0.001)));
  }, { passive: false });
  canvas.addEventListener('dblclick', () => Object.assign(view, DEFAULT_VIEW));
}

function drawPositionInset(ctx, size, position) {
  const box = size * 0.26, left = size * 0.03, top = size - box - size * 0.03, range = 250;
  ctx.fillStyle = 'rgba(18, 26, 46, 0.85)';
  ctx.strokeStyle = COLORS.grid;
  ctx.lineWidth = 1.5;
  ctx.fillRect(left, top, box, box);
  ctx.strokeRect(left, top, box, box);
  ctx.beginPath();
  ctx.moveTo(left + box / 2, top);
  ctx.lineTo(left + box / 2, top + box);
  ctx.moveTo(left, top + box / 2);
  ctx.lineTo(left + box, top + box / 2);
  ctx.stroke();
  ctx.fillStyle = COLORS.muted;
  ctx.font = `${size / 48}px system-ui, sans-serif`;
  ctx.textAlign = 'left';
  ctx.fillText('x / y position', left + 6, top + size / 40);
  if (!position) return;
  const clamp = (v) => Math.max(-range, Math.min(range, v));
  const px = left + box / 2 + clamp(position[0]) / range * box / 2;
  const py = top + box / 2 - clamp(position[1]) / range * box / 2;
  ctx.fillStyle = COLORS.accent;
  ctx.shadowColor = COLORS.accent;
  ctx.shadowBlur = 12;
  ctx.beginPath();
  ctx.arc(px, py, size / 110, 0, Math.PI * 2);
  ctx.fill();
  ctx.shadowBlur = 0;
  ctx.fillStyle = COLORS.muted;
  ctx.fillText(`z ${position[2].toFixed(0)} mm`, left + 6, top + box - 8);
}

/* Plots */

function drawPlot(canvas, entries, pick, colors, minimumSpan, now) {
  const ctx = canvas.getContext('2d');
  const w = canvas.width, h = canvas.height, dpr = window.devicePixelRatio || 1;
  const pad = { left: 46 * dpr, right: 10 * dpr, top: 10 * dpr, bottom: 10 * dpr };
  ctx.clearRect(0, 0, w, h);
  let low = Infinity, high = -Infinity;
  for (const entry of entries) {
    const values = pick(entry);
    if (!values) continue;
    for (const v of values) if (v !== null && Number.isFinite(v)) { low = Math.min(low, v); high = Math.max(high, v); }
  }
  ctx.font = `${11 * dpr}px system-ui, sans-serif`;
  if (low === Infinity) {
    ctx.fillStyle = COLORS.muted;
    ctx.textAlign = 'center';
    ctx.fillText('No valid samples', w / 2, h / 2);
    return;
  }
  const middle = (low + high) / 2;
  const span = Math.max(high - low, minimumSpan) * 1.15;
  low = middle - span / 2;
  high = middle + span / 2;
  const x = (at) => pad.left + (1 - (now - at) / HISTORY_MS) * (w - pad.left - pad.right);
  const y = (v) => pad.top + (high - v) / (high - low) * (h - pad.top - pad.bottom);
  ctx.strokeStyle = COLORS.grid;
  ctx.fillStyle = COLORS.muted;
  ctx.lineWidth = 1;
  ctx.textAlign = 'right';
  const step = niceStep(span / 4);
  for (let v = Math.ceil(low / step) * step; v <= high; v += step) {
    ctx.beginPath();
    ctx.moveTo(pad.left, y(v));
    ctx.lineTo(w - pad.right, y(v));
    ctx.stroke();
    ctx.fillText(step < 1 ? v.toFixed(1) : v.toFixed(0), pad.left - 6 * dpr, y(v) + 4 * dpr);
  }
  ctx.lineWidth = 1.8 * dpr;
  ctx.lineJoin = 'round';
  colors.forEach((color, index) => {
    ctx.strokeStyle = color;
    ctx.beginPath();
    let drawing = false;
    for (const entry of entries) {
      const values = pick(entry);
      const v = values ? values[index] : null;
      if (v === null || v === undefined || !Number.isFinite(v)) { drawing = false; continue; }
      if (drawing) ctx.lineTo(x(entry.at), y(v)); else ctx.moveTo(x(entry.at), y(v));
      drawing = true;
    }
    ctx.stroke();
  });
}

function drawLanes(canvas, entries, pick, colors, minimumSpan, digits, now) {
  const ctx = canvas.getContext('2d');
  const w = canvas.width, h = canvas.height, dpr = window.devicePixelRatio || 1;
  const left = 58 * dpr, right = 10 * dpr, laneHeight = h / colors.length;
  ctx.clearRect(0, 0, w, h);
  ctx.font = `${11 * dpr}px system-ui, sans-serif`;
  const x = (at) => left + (1 - (now - at) / HISTORY_MS) * (w - left - right);
  colors.forEach((color, index) => {
    const top = index * laneHeight, inset = 8 * dpr;
    let low = Infinity, high = -Infinity, last = null;
    for (const entry of entries) {
      const values = pick(entry);
      const v = values ? values[index] : null;
      if (v === null || v === undefined || !Number.isFinite(v)) continue;
      low = Math.min(low, v);
      high = Math.max(high, v);
      last = v;
    }
    if (index > 0) {
      ctx.strokeStyle = COLORS.grid;
      ctx.lineWidth = 1;
      ctx.beginPath();
      ctx.moveTo(0, top);
      ctx.lineTo(w, top);
      ctx.stroke();
    }
    ctx.textAlign = 'right';
    if (last === null) {
      ctx.fillStyle = COLORS.muted;
      ctx.fillText('no data', left - 8 * dpr, top + laneHeight / 2 + 4 * dpr);
      return;
    }
    const middle = (low + high) / 2, span = Math.max(high - low, minimumSpan) * 1.2;
    const y = (v) => top + inset + ((middle + span / 2) - v) / span * (laneHeight - 2 * inset);
    ctx.fillStyle = color;
    ctx.font = `600 ${12 * dpr}px ui-monospace, monospace`;
    ctx.fillText(last.toFixed(digits), left - 8 * dpr, top + laneHeight / 2 + 4 * dpr);
    ctx.font = `${10 * dpr}px system-ui, sans-serif`;
    ctx.fillStyle = COLORS.muted;
    ctx.fillText(`±${(span / 2).toFixed(digits)}`, left - 8 * dpr, top + laneHeight - inset);
    ctx.strokeStyle = color;
    ctx.lineWidth = 1.8 * dpr;
    ctx.lineJoin = 'round';
    ctx.beginPath();
    let drawing = false;
    for (const entry of entries) {
      const values = pick(entry);
      const v = values ? values[index] : null;
      if (v === null || v === undefined || !Number.isFinite(v)) { drawing = false; continue; }
      if (drawing) ctx.lineTo(x(entry.at), y(v)); else ctx.moveTo(x(entry.at), y(v));
      drawing = true;
    }
    ctx.stroke();
  });
}

function niceStep(raw) {
  const power = Math.pow(10, Math.floor(Math.log10(raw)));
  const unit = raw / power;
  return (unit < 1.5 ? 1 : unit < 3.5 ? 2 : unit < 7.5 ? 5 : 10) * power;
}

/* Gaze */

function eyeCell(id, eye) {
  const element = $(id);
  element.className = eye.valid ? 'valid' : 'invalid';
  setText(id, eye.valid ? 'Valid' : 'Not found');
}

function renderGaze(gaze) {
  if (app.view !== 'gaze') return;
  for (const [side, eye] of [['left', gaze.left], ['right', gaze.right]]) {
    eyeCell(side + 'Valid', eye);
    setText(side + 'Point', point(eye.point));
    setText(side + 'Pupil', fixed(eye.pupil, 2, ' mm'));
    setText(side + 'Open', fixed(eye.openness, 2));
    setText(side + 'Origin', eye.origin ? point(eye.origin, 0) + ' mm' : '-');
  }
  setText('gazePoint', point(gaze.point));
  const valid = app.gazeHistory.filter((entry) => entry.gaze.point).length;
  setText('gazeValidShare', app.gazeHistory.length ? `${Math.round(100 * valid / app.gazeHistory.length)} % of the last 10 s` : '-');
}

function updateDot(now, dt) {
  const gaze = app.gaze;
  const dot = app.dot;
  if (gaze && gaze.point && now - dot.seen < GAZE_LOST_MS) {
    const [tx, ty] = gaze.point;
    if (!dot.initialised) {
      dot.x = tx; dot.y = ty; dot.initialised = true;
    } else {
      const k = app.options.smoothingMs <= 0 ? 1 : 1 - Math.exp(-dt / app.options.smoothingMs);
      dot.x += (tx - dot.x) * k;
      dot.y += (ty - dot.y) * k;
    }
    dot.trail.push({ at: now, x: dot.x, y: dot.y });
  }
  while (dot.trail.length && now - dot.trail[0].at > TRAIL_MS) dot.trail.shift();
}

function drawGazeScene(ctx, width, height, now, scale) {
  ctx.fillStyle = '#03050b';
  ctx.fillRect(0, 0, width, height);
  ctx.strokeStyle = 'rgba(56, 189, 248, 0.07)';
  ctx.lineWidth = scale;
  for (let i = 1; i < 6; i++) {
    ctx.beginPath();
    ctx.moveTo(width * i / 6, 0);
    ctx.lineTo(width * i / 6, height);
    ctx.moveTo(0, height * i / 6);
    ctx.lineTo(width, height * i / 6);
    ctx.stroke();
  }
  const dot = app.dot;
  if (app.options.raw) {
    ctx.fillStyle = 'rgba(245, 158, 11, 0.55)';
    for (const entry of app.gazeHistory) {
      if (!entry.gaze.point || now - entry.at > TRAIL_MS * 1.5) continue;
      ctx.beginPath();
      ctx.arc(entry.gaze.point[0] * width, entry.gaze.point[1] * height, 3 * scale, 0, Math.PI * 2);
      ctx.fill();
    }
  }
  if (app.options.trail && dot.trail.length > 1) {
    ctx.lineCap = 'round';
    for (let i = 1; i < dot.trail.length; i++) {
      const a = dot.trail[i - 1], b = dot.trail[i];
      const fade = 1 - (now - b.at) / TRAIL_MS;
      ctx.strokeStyle = `rgba(56, 189, 248, ${0.5 * fade})`;
      ctx.lineWidth = 7 * scale * fade + scale;
      ctx.beginPath();
      ctx.moveTo(a.x * width, a.y * height);
      ctx.lineTo(b.x * width, b.y * height);
      ctx.stroke();
    }
  }
  const lost = now - dot.seen;
  if (!dot.initialised) return;
  const alpha = lost < GAZE_LOST_MS ? 1 : Math.max(0.15, 1 - (lost - GAZE_LOST_MS) / 600);
  const x = dot.x * width, y = dot.y * height, radius = 30 * scale;
  const halo = ctx.createRadialGradient(x, y, 0, x, y, radius);
  halo.addColorStop(0, `rgba(125, 211, 252, ${0.9 * alpha})`);
  halo.addColorStop(0.35, `rgba(56, 189, 248, ${0.45 * alpha})`);
  halo.addColorStop(1, 'rgba(56, 189, 248, 0)');
  ctx.fillStyle = halo;
  ctx.beginPath();
  ctx.arc(x, y, radius, 0, Math.PI * 2);
  ctx.fill();
  ctx.fillStyle = `rgba(255, 255, 255, ${alpha})`;
  ctx.beginPath();
  ctx.arc(x, y, 5 * scale, 0, Math.PI * 2);
  ctx.fill();
  if (lost >= GAZE_LOST_MS) {
    ctx.fillStyle = COLORS.muted;
    ctx.font = `${14 * scale}px system-ui, sans-serif`;
    ctx.textAlign = 'center';
    ctx.fillText('no gaze', x, y + radius + 16 * scale);
  }
}

function previewAspect() {
  const area = app.status && app.status.display_area;
  return area && area.width_mm > 0 && area.height_mm > 0 ? area.width_mm / area.height_mm : 16 / 9;
}

function drawGazePreview(now) {
  const canvas = $('gazePreview');
  const ctx = canvas.getContext('2d');
  drawGazeScene(ctx, canvas.width, canvas.height, now, canvas.width / 900);
  ctx.strokeStyle = COLORS.grid;
  ctx.lineWidth = 2;
  ctx.strokeRect(1, 1, canvas.width - 2, canvas.height - 2);
}

function drawStage(now) {
  const canvas = $('stageCanvas');
  drawGazeScene(canvas.getContext('2d'), canvas.width, canvas.height, now, window.devicePixelRatio || 1);
}

let hintTimer = 0;

function showStageHint() {
  const hint = $('stageHint');
  hint.classList.remove('hidden');
  clearTimeout(hintTimer);
  hintTimer = setTimeout(() => hint.classList.add('hidden'), 2500);
}

async function openStage() {
  const stage = $('gazeStage');
  stage.hidden = false;
  resizeCanvases();
  showStageHint();
  try {
    await stage.requestFullscreen({ navigationUI: 'hide' });
  } catch (error) {
    showStageHint();
  }
}

function closeStage() {
  $('gazeStage').hidden = true;
  if (document.fullscreenElement) document.exitFullscreen();
}

function setSmoothing(ms) {
  app.options.smoothingMs = Math.max(0, Math.min(400, ms));
  $('optSmoothing').value = app.options.smoothingMs;
  setText('smoothingText', app.options.smoothingMs + ' ms');
}

/* Layout and animation */

function fitCanvas(canvas, aspect) {
  const dpr = window.devicePixelRatio || 1;
  const width = Math.max(1, Math.round(canvas.clientWidth * dpr));
  const height = Math.max(1, Math.round((aspect ? canvas.clientWidth / aspect : canvas.clientHeight) * dpr));
  if (canvas.width !== width || canvas.height !== height) {
    canvas.width = width;
    canvas.height = height;
  }
}

function resizeCanvases() {
  for (const id of ['plotPosition', 'plotRotation', 'plotPupil']) fitCanvas($(id));
  fitCanvas($('gazePreview'), previewAspect());
  const stage = $('stageCanvas');
  if (!$('gazeStage').hidden) fitCanvas(stage);
  app.dirty.camera = true;
}

let lastFrameAt = performance.now();

function animate(now) {
  const dt = Math.min(100, now - lastFrameAt);
  lastFrameAt = now;
  prune(app.poseHistory, now);
  prune(app.gazeHistory, now);
  updateDot(now, dt);
  if (app.view === 'camera' && app.dirty.camera) {
    drawCamera();
    app.dirty.camera = false;
  }
  if (app.view === 'pose') {
    drawScene();
    const position = (entry) => entry.pose.valid ? entry.pose.position : null;
    const rotation = (entry) => entry.pose.valid ? entry.pose.rotation : null;
    drawLanes($('plotPosition'), app.poseHistory, position, [COLORS.x, COLORS.y, COLORS.z], 10, 1, now);
    drawLanes($('plotRotation'), app.poseHistory, rotation, [COLORS.x, COLORS.y, COLORS.z], 2, 1, now);
  }
  if (app.view === 'gaze') {
    drawGazePreview(now);
    const pupils = (entry) => [entry.gaze.left.pupil, entry.gaze.right.pupil];
    drawPlot($('plotPupil'), app.gazeHistory, pupils, [COLORS.x, COLORS.z], 1, now);
  }
  if (!$('gazeStage').hidden) drawStage(now);
  if (app.status && performance.now() - app.statusAt > STALE_STATUS_MS) renderConnection();
  requestAnimationFrame(animate);
}

function bindControls() {
  for (const button of document.querySelectorAll('.tabs button')) {
    button.addEventListener('click', () => selectView(button.dataset.view));
  }
  const toggle = (id, key) => $(id).addEventListener('change', (event) => {
    app.options[key] = event.target.checked;
    app.dirty.camera = true;
  });
  toggle('optBox', 'box');
  toggle('optLandmarks', 'landmarks');
  toggle('optMesh', 'mesh');
  toggle('optMirror', 'mirror');
  toggle('optTrail', 'trail');
  toggle('optRaw', 'raw');
  $('optSmoothing').addEventListener('input', (event) => setSmoothing(Number(event.target.value)));
  $('fullscreenButton').addEventListener('click', openStage);
  $('gazeStage').addEventListener('dblclick', closeStage);
  $('gazeStage').addEventListener('mousemove', showStageHint);
  document.addEventListener('fullscreenchange', () => {
    if (!document.fullscreenElement) $('gazeStage').hidden = true;
    resizeCanvases();
  });
  document.addEventListener('keydown', (event) => {
    if ($('gazeStage').hidden) return;
    if (event.key === 'Escape') closeStage();
    else if (event.key === 't' || event.key === 'T') { $('optTrail').click(); showStageHint(); }
    else if (event.key === 'r' || event.key === 'R') { $('optRaw').click(); showStageHint(); }
    else if (event.key === '[') setSmoothing(app.options.smoothingMs - 20);
    else if (event.key === ']') setSmoothing(app.options.smoothingMs + 20);
  });
  window.addEventListener('resize', resizeCanvases);
  window.addEventListener('hashchange', () => selectView(location.hash.slice(1)));
}

bindControls();
bindScene();
selectView(location.hash.slice(1) || 'camera');
connect();
requestAnimationFrame(animate);
