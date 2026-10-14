'use strict';
// The tracker alignment screen of Tobii's display setup (ScreenPlaneSetupControl), redrawn for the browser.
// Layout, drawings, colours, texts and animations follow the control's XAML; docs/display-setup.md lists them.
// Used by the setup page and by the standalone display setup page.

/* exported TrackerAlignment */
const TrackerAlignment = (() => {
  const COLOURS = {background: '#021419', foreground: '#afc4cd', highlight: '#ffffff', text: '#d9f4ff',
                   button: '#004b66', buttonHover: '#018ec1', buttonPressed: '#00beff',
                   disabled: '#2b3940', disabledText: '#637279', screenBlack: '#000000'};
  const HEADER = 60, ROWS = [0.35, 1, 1, 2], BOTTOM_MARGIN = 150, HANDLE_HALF = 30;
  const MIN_LINE = 0.02, MAX_LINE = 0.98, MIN_DISTANCE = 0.05;
  const TEXTS = {
    where: 'Where is your eye tracker?',
    attach: "If you haven't yet attached it to the display, now is the time.",
    thisDisplay: 'This display',
    done: 'Done',
  };

  // EyeTracker5.xaml (812.708 x 72.151) and EyeTracker5OnScreen.xaml (812.708 x 49.437): [path, stroke, filled].
  const TRACKER = {width: 812.708, height: 72.151, paths: [
    ['M3.916,29.994 L17.39,10.54 C20.81,4.632 27.12,0.996 33.947,1 L776.691,1.393 C782.221,1.396 787.073,4.391 790.425,8.79 C796.916,15.994 807.916,28.994 807.916,28.994', 2, true],
    ['M361.252,64.799 L24.564,64.543 C11.54,64.533 0.99,54.036 1,41.097 L1,41.097 C1.01,28.159 11.576,17.678 24.6,17.688 L788.144,18.269 C801.168,18.279 811.718,28.776 811.708,41.715 L811.708,41.715 C811.699,54.653 801.132,65.134 788.108,65.124 L451.649,64.868', 2, true],
    ['M150.833,0.916 L145.819,18.081', 2, false],
    ['M662.232,1.995 L668.219,18.478', 2, false],
    ['M64.174,59.831 L24.461,59.802 C14.292,59.795 6.055,51.545 6.063,41.377 L6.063,41.377 C6.072,31.209 14.32,22.973 24.488,22.98 L64.202,23.01 C74.37,23.017 82.607,31.267 82.599,41.435 L82.599,41.435 C82.591,51.603 74.342,59.839 64.174,59.831', 1.8, false],
    ['M788.859,59.831 L749.146,59.802 C738.977,59.795 730.74,51.545 730.749,41.377 L730.749,41.377 C730.757,31.209 739.005,22.973 749.173,22.98 L788.887,23.01 C799.055,23.017 807.292,31.267 807.284,41.435 L807.284,41.435 C807.276,51.603 799.027,59.839 788.859,59.831', 1.8, false],
    ['M354.701,46.928 L354.704,42.908 C354.714,29.834 364.757,19.244 377.136,18.256 L437.609,18.302 C449.986,19.309 460.013,29.915 460.003,42.988 L460,47.008', 1.8, false],
    ['M438.248,72.151 L376.346,72.101 C363.617,72.091 353.306,61.765 353.315,49.036 L353.316,48.128 C353.325,35.398 363.653,25.086 376.383,25.096 L438.285,25.145 C451.014,25.155 461.325,35.482 461.316,48.211 L461.315,49.119 C461.306,61.849 450.978,72.161 438.248,72.151', 1.8, true],
    ['M380.375,48.629 C380.372,52.53 377.197,55.691 373.283,55.688 C369.369,55.685 366.199,52.52 366.202,48.618 C366.205,44.716 369.38,41.556 373.294,41.559 C377.208,41.561 380.378,44.727 380.375,48.629', 1.8, false],
    ['M407.371,0.713 L407.372,18.004', 2, false],
  ]};
  const FLAT_TRACKER = {width: 812.708, height: 49.437, paths: [
    ['M407.658,48.182 L24.564,47.855 C11.54,47.845 0.99,37.348 1,24.41 L1,24.41 C1.01,11.471 11.576,0.99 24.6,1 L788.144,1.582 C801.168,1.591 811.718,12.088 811.708,25.027 L811.708,25.027 C811.699,37.966 801.132,48.447 788.108,48.437 L407.658,48.182', 2, true],
    ['M64.481,43.144 L24.768,43.115 C14.599,43.107 6.362,34.858 6.37,24.689 L6.37,24.689 C6.379,14.522 14.627,6.285 24.795,6.293 L64.509,6.322 C74.677,6.33 82.914,14.579 82.906,24.748 L82.906,24.748 C82.898,34.915 74.649,43.151 64.481,43.144', 2, false],
    ['M788.167,43.144 L748.453,43.115 C738.284,43.107 730.047,34.858 730.056,24.689 L730.056,24.689 C730.064,14.522 738.312,6.285 748.48,6.293 L788.194,6.322 C798.362,6.33 806.599,14.579 806.591,24.748 L806.591,24.748 C806.583,34.915 798.334,43.151 788.167,43.144', 2, false],
    ['M437.549,48.398 L375.645,48.351 C362.916,48.341 352.605,38.014 352.615,25.285 L352.615,24.38 C352.625,11.652 362.952,1.341 375.68,1.351 L437.585,1.398 C450.314,1.407 460.625,11.734 460.615,24.463 L460.615,25.368 C460.605,38.097 450.278,48.407 437.549,48.398', 2, false],
    ['M379.685,24.88 C379.682,28.781 376.507,31.942 372.593,31.939 C368.68,31.936 365.509,28.771 365.512,24.869 C365.515,20.967 368.69,17.806 372.604,17.809 C376.518,17.812 379.688,20.978 379.685,24.88', 2, false],
  ]};
  // Screen.xaml, the monitor of the intro (143 x 109.5): body and stand in the foreground colour, a black screen.
  const MONITOR = {width: 143, height: 109.5,
    body: 'M140.6 85.8H2.8c-0.6 0-1.1-0.5-1.1-1.1V3.1C1.7 2.5 2.2 2 2.8 2h137.8c0.6 0 1.1 0.5 1.1 1.1v81.6C141.7 85.3 141.2 85.8 140.6 85.8z',
    stand: 'M64.2 84.8v14.9c0 0.7-0.5 1.3-1.2 1.3l-23.7 1.8c-0.4 0-0.7 0.4-0.7 0.7v3.3c0 0.4 0.3 0.7 0.7 0.7c0 0 31.4 0.4 32.5 0.4s32.5-0.4 32.5-0.4c0.4 0 0.7-0.3 0.7-0.7v-3.3c0-0.4-0.3-0.7-0.7-0.7l-23.7-1.8c-0.7-0.1-1.2-0.6-1.2-1.3V84.8H64.2z',
    screen: [7.3, 7.6, 128.9, 72.5]};
  const INTRO_MONITOR_WIDTH = 400, INTRO_TRACKER_TOP = 222, INTRO_TRACKER_WIDTH = 250;

  const STYLE = `
    .tracker-alignment { position: absolute; inset: 0; background: ${COLOURS.background}; overflow: hidden;
      font-family: "Segoe UI Light", "Segoe UI", "Selawik", "Open Sans", "Noto Sans", system-ui, sans-serif;
      font-weight: 300; user-select: none; }
    .tracker-alignment canvas { position: absolute; left: 0; top: 0; width: 100%; height: 100%; }
    .tracker-alignment .ta-text { position: absolute; left: 0; right: 0; display: flex; flex-direction: column;
      align-items: center; justify-content: flex-end; text-align: center; pointer-events: none; color: ${COLOURS.text}; }
    .tracker-alignment .ta-explaining { font-size: 37px; margin: 0; line-height: 1.33; }
    .tracker-alignment .ta-headline { font-size: 28px; margin: 0; line-height: 1.33; }
    .tracker-alignment .ta-buttons { position: absolute; display: flex; align-items: center; justify-content: center; }
    .tracker-alignment button { font: inherit; font-size: 16px; min-width: 120px; min-height: 33px; padding: 0 25px;
      border: 0; border-radius: 16.5px; color: ${COLOURS.highlight}; background: ${COLOURS.button}; cursor: pointer;
      transform: scale(0.95); transition: background .1s, transform .1s; }
    .tracker-alignment button:hover { background: ${COLOURS.buttonHover}; transform: scale(0.975); }
    .tracker-alignment button:active { background: ${COLOURS.buttonPressed}; transform: scale(0.95); }
    .tracker-alignment button:disabled { background: ${COLOURS.disabled}; color: ${COLOURS.disabledText};
      cursor: default; transform: scale(0.95); }
    .tracker-alignment .ta-done { margin-right: 60px; }
  `;

  function easeQuadraticInOut(t) { return t < 0.5 ? 2 * t * t : 1 - Math.pow(-2 * t + 2, 2) / 2; }

  // WPF BackEase (Amplitude 1) in EaseInOut mode.
  function easeBackInOut(t) {
    const easeIn = (x) => x * x * x - x * Math.sin(Math.PI * x);
    return t < 0.5 ? easeIn(2 * t) / 2 : (1 - easeIn(2 - 2 * t)) / 2 + 0.5;
  }

  function addStyle() {
    if (document.getElementById('tracker-alignment-style')) return;
    const style = document.createElement('style');
    style.id = 'tracker-alignment-style';
    style.textContent = STYLE;
    document.head.appendChild(style);
  }

  function element(tag, className, text) {
    const node = document.createElement(tag);
    if (className) node.className = className;
    if (text !== undefined) node.textContent = text;
    return node;
  }

  function fadeIn(node) {
    node.animate([{opacity: 0}, {opacity: 0, offset: 1 / 3}, {opacity: 1}], {duration: 1500, fill: 'backwards'});
  }

  class TrackerAlignment {
    /* options: onDone(left, right), onThisDisplay(), canFinish() -> bool (Done enabled) */
    constructor(container, options = {}) {
      addStyle();
      this.options = options;
      this.root = element('div', 'tracker-alignment');
      this.canvas = element('canvas');
      this.text = element('div', 'ta-text');
      this.buttons = element('div', 'ta-buttons');
      this.thisDisplayButton = element('button', 'ta-this-display', TEXTS.thisDisplay);
      this.doneButton = element('button', 'ta-done', TEXTS.done);
      this.buttons.append(this.thisDisplayButton, this.doneButton);
      this.root.append(this.canvas, this.text, this.buttons);
      container.appendChild(this.root);
      this.paths = new Map();
      this.config = null;
      this.state = 'idle';
      this.left = 0; this.right = 1;
      this.trackerTop = null;
      this.hover = null; this.drag = null; this.keyboardLine = null;
      this.intro = {progress: 0, crossFade: 0};
      this.thisDisplayButton.addEventListener('click', () => this.chooseThisDisplay());
      this.doneButton.addEventListener('click', () => this.finish());
      this.canvas.addEventListener('mousedown', (event) => this.press(event));
      window.addEventListener('mousemove', (event) => this.move(event));
      window.addEventListener('mouseup', () => this.release());
      document.addEventListener('mouseleave', () => this.release());
      window.addEventListener('resize', () => { this.layout(); this.draw(); });
    }

    // Layout: a 60 px header, then rows 0.35 : 1 : 1 : 2; text at the bottom of row 1, buttons in row 2,
    // the lines' canvas in row 3. The tracker sits 150 px above the bottom.
    rows() {
      const total = ROWS.reduce((a, b) => a + b, 0), content = window.innerHeight - HEADER;
      let top = HEADER;
      return ROWS.map((weight) => { const row = {top: top, height: content * weight / total}; top += row.height; return row; });
    }

    layout() {
      const rows = this.rows(), w = window.innerWidth;
      Object.assign(this.text.style, {top: rows[1].top + 'px', height: rows[1].height + 'px'});
      Object.assign(this.buttons.style, {top: rows[2].top + 'px', height: rows[2].height + 'px',
                                         left: w / 5 + 'px', width: w * 3 / 5 + 'px'});
    }

    show(config) {
      this.config = config;
      this.left = config.left; this.right = config.right;
      this.hover = null; this.drag = null; this.keyboardLine = null; this.trackerTop = null;
      this.layout();
      this.text.replaceChildren();
      if (config.intro) {
        this.state = 'intro';
        this.intro = {progress: 0, crossFade: 0};
        this.text.append(element('p', 'ta-explaining', TEXTS.where), element('p', 'ta-headline', TEXTS.attach));
        this.thisDisplayButton.hidden = false; this.thisDisplayButton.disabled = false;
        this.doneButton.hidden = true;
        fadeIn(this.text); fadeIn(this.buttons);
        this.draw();
      } else {
        this.showLines();
      }
    }

    showLines() {
      this.text.replaceChildren(element('p', 'ta-explaining', this.config.instructions));
      this.thisDisplayButton.hidden = true; this.doneButton.hidden = false;
      fadeIn(this.text); fadeIn(this.buttons);
      this.slideIn();
    }

    slideIn() {
      this.state = 'sliding';
      const l0 = this.config.left, r0 = this.config.right, start = performance.now();
      const frame = (now) => {
        const t = Math.min(1, (now - start) / 1500), e = easeBackInOut(t);
        this.left = l0 - 0.01 * (1 - e); this.right = r0 + 0.01 * (1 - e);
        if (t >= 1) { this.left = l0; this.right = r0; this.state = 'lines'; }
        this.draw();
        if (t < 1) requestAnimationFrame(frame);
      };
      requestAnimationFrame(frame);
    }

    chooseThisDisplay() {
      if (this.state !== 'intro') return;
      this.thisDisplayButton.disabled = true;
      this.state = 'moving';
      if (this.options.onThisDisplay) this.options.onThisDisplay();
      const start = performance.now();
      const frame = (now) => {
        const elapsed = now - start;
        this.intro.progress = Math.min(1, elapsed / 1000);
        this.intro.crossFade = Math.min(1, Math.max(0, (elapsed - 1000) / 500));
        this.draw();
        if (elapsed < 1500) { requestAnimationFrame(frame); return; }
        this.showLines();
      };
      requestAnimationFrame(frame);
    }

    finish() {
      if (this.state !== 'lines' || !this.finishAllowed()) return;
      this.state = 'finished';
      this.doneButton.disabled = true;
      this.draw();
      if (this.options.onDone) this.options.onDone(this.left, this.right);
    }

    finishAllowed() { return this.options.canFinish ? this.options.canFinish() : true; }

    setText(lines) {
      this.text.replaceChildren(...lines.filter(Boolean).map((line, i) => element('p', i ? 'ta-headline' : 'ta-explaining', line)));
      this.buttons.hidden = true;
    }

    // Geometry --------------------------------------------------------------------------------------

    trackerBox(left = this.left, right = this.right) {
      const w = window.innerWidth, h = window.innerHeight;
      const width = (right - left) * this.config.picture_proportion * w, height = width * TRACKER.height / TRACKER.width;
      const top = this.trackerTop === null ? h - height - BOTTOM_MARGIN : this.trackerTop;
      return {x: ((left + right) / 2) * w - width / 2, y: top, w: width, h: height, lineTop: top + height / 4};
    }

    lineAt(x, y) {
      if (this.state !== 'lines' && this.state !== 'sliding') return null;
      const w = window.innerWidth, box = this.trackerBox();
      const near = ['left', 'right'].map((side) => ({side: side, distance: Math.abs(x - this[side] * w)}))
        .filter((c) => c.distance <= HANDLE_HALF && y >= box.lineTop).sort((a, b) => a.distance - b.distance);
      if (near.length) return near[0].side;
      return x >= box.x && x <= box.x + box.w && y >= box.y && y <= box.y + box.h ? 'tracker' : null;
    }

    setLeft(value) { this.left = Math.max(Math.min(value, this.right - MIN_DISTANCE), MIN_LINE); }
    setRight(value) { this.right = Math.min(Math.max(value, this.left + MIN_DISTANCE), MAX_LINE); }

    // Input -----------------------------------------------------------------------------------------

    press(event) {
      if (this.state !== 'lines') return;
      const hit = this.lineAt(event.clientX, event.clientY);
      if (!hit) return;
      const w = window.innerWidth;
      this.keyboardLine = null;
      this.drag = {kind: hit, toLeft: this.left * w - event.clientX, toRight: this.right * w - event.clientX};
    }

    move(event) {
      if (!this.config || !this.root.isConnected || this.root.offsetParent === null) return;
      const w = window.innerWidth, drag = this.drag;
      if (drag && this.state === 'lines') {
        if (drag.kind !== 'right') this.setLeft((event.clientX + drag.toLeft) / w);
        if (drag.kind !== 'left') this.setRight((event.clientX + drag.toRight) / w);
      }
      this.keyboardLine = null;
      this.hover = this.lineAt(event.clientX, event.clientY);
      this.canvas.style.cursor = drag || this.hover ? 'ew-resize' : 'default';
      this.draw();
    }

    release() { if (this.drag) { this.drag = null; this.draw(); } }

    /* Keyboard extras: Enter is the default button; arrows nudge the last used line by a device pixel. */
    handleKey(event) {
      if (event.key === 'Enter') {
        event.preventDefault();
        if (this.state === 'intro') this.chooseThisDisplay(); else this.finish();
        return true;
      }
      if (this.state !== 'lines') return false;
      const key = event.key.toLowerCase();
      if (event.key === 'Tab') this.keyboardLine = this.keyboardLine === 'right' ? 'left' : 'right';
      else if (key === 'l' || key === 'r') this.keyboardLine = key === 'l' ? 'left' : 'right';
      else if (event.key === 'ArrowLeft' || event.key === 'ArrowRight') {
        const line = this.keyboardLine || (this.hover === 'right' ? 'right' : 'left');
        this.keyboardLine = line;
        const step = (event.shiftKey ? 10 : 1) / ((window.devicePixelRatio || 1) * window.innerWidth);
        const delta = event.key === 'ArrowLeft' ? -step : step;
        if (line === 'left') this.setLeft(this.left + delta); else this.setRight(this.right + delta);
      } else return false;
      event.preventDefault();
      this.draw();
      return true;
    }

    // Drawing ---------------------------------------------------------------------------------------

    path(data) {
      if (!this.paths.has(data)) this.paths.set(data, new Path2D(data));
      return this.paths.get(data);
    }

    drawArtwork(ctx, art, box, alpha = 1) {
      ctx.save();
      ctx.globalAlpha = alpha;
      ctx.translate(box.x, box.y);
      ctx.scale(box.w / art.width, box.h / art.height);
      ctx.lineJoin = 'miter'; ctx.miterLimit = 1;
      for (const [data, stroke, filled] of art.paths) {
        const path = this.path(data);
        if (filled) { ctx.fillStyle = COLOURS.screenBlack; ctx.fill(path); }
        ctx.lineWidth = stroke; ctx.strokeStyle = COLOURS.highlight; ctx.stroke(path);
      }
      ctx.restore();
    }

    drawMonitor(ctx, alpha) {
      const rows = this.rows(), scale = INTRO_MONITOR_WIDTH / MONITOR.width;
      const x = (window.innerWidth - INTRO_MONITOR_WIDTH) / 2, y = rows[3].top;
      ctx.save();
      ctx.globalAlpha = alpha;
      ctx.translate(x, y); ctx.scale(scale, scale);
      ctx.fillStyle = COLOURS.foreground;
      ctx.fill(this.path(MONITOR.body)); ctx.fill(this.path(MONITOR.stand));
      ctx.fillStyle = COLOURS.screenBlack;
      ctx.fillRect(...MONITOR.screen);
      ctx.restore();
    }

    drawIntro(ctx) {
      const rows = this.rows(), w = window.innerWidth, progress = easeQuadraticInOut(this.intro.progress);
      const flatHeight = INTRO_TRACKER_WIDTH * FLAT_TRACKER.height / FLAT_TRACKER.width;
      const flatTop = rows[3].top + INTRO_TRACKER_TOP;
      const target = this.trackerBox(this.config.left, this.config.right);
      const detailTop = Math.min(flatTop - target.h / 4, target.y);
      const flatTargetTop = detailTop + target.h / 4;
      const mix = (a, b) => a + (b - a) * progress;
      this.drawMonitor(ctx, 1 - this.intro.progress);
      const flat = {x: mix((w - INTRO_TRACKER_WIDTH) / 2, target.x), y: mix(flatTop, flatTargetTop),
                    w: mix(INTRO_TRACKER_WIDTH, target.w), h: mix(flatHeight, target.w * FLAT_TRACKER.height / FLAT_TRACKER.width)};
      this.drawArtwork(ctx, FLAT_TRACKER, flat, 1 - this.intro.crossFade);
      if (this.intro.crossFade > 0) {
        this.trackerTop = detailTop;
        this.drawArtwork(ctx, TRACKER, this.trackerBox(this.config.left, this.config.right), this.intro.crossFade);
      }
    }

    drawHandle(ctx, side, lineTop) {
      const x = Math.round(this[side] * window.innerWidth) + 0.5, h = window.innerHeight, middle = (lineTop + h) / 2;
      const active = this.hover === side || this.keyboardLine === side || (this.drag && this.drag.kind === side && this.hover === side);
      const colour = active ? COLOURS.highlight : COLOURS.foreground;
      ctx.strokeStyle = colour; ctx.lineWidth = 1;
      ctx.setLineDash([5, 5]);
      ctx.beginPath(); ctx.moveTo(x, lineTop); ctx.lineTo(x, h); ctx.stroke();
      ctx.setLineDash([]);
      ctx.beginPath(); ctx.moveTo(x - 6, h - 10); ctx.lineTo(x, h); ctx.lineTo(x + 6, h - 10); ctx.stroke();
      ctx.fillStyle = COLOURS.highlight; ctx.strokeStyle = COLOURS.highlight;
      const left = x - 30, right = x + 10;
      for (const points of [[[left, middle], [left + 14, middle - 14], [left + 14, middle + 14]],
                            [[right + 14, middle], [right, middle - 14], [right, middle + 14]]]) {
        ctx.beginPath(); ctx.moveTo(...points[0]); ctx.lineTo(...points[1]); ctx.lineTo(...points[2]); ctx.closePath();
        ctx.fill(); ctx.stroke();
      }
    }

    draw() {
      const ctx = this.canvas.getContext('2d'), dpr = window.devicePixelRatio || 1;
      const width = Math.round(window.innerWidth * dpr), height = Math.round(window.innerHeight * dpr);
      if (this.canvas.width !== width || this.canvas.height !== height) { this.canvas.width = width; this.canvas.height = height; }
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      ctx.clearRect(0, 0, window.innerWidth, window.innerHeight);
      if (!this.config) return;
      ctx.save();
      const canvasTop = this.rows()[3].top;
      ctx.beginPath(); ctx.rect(0, canvasTop, window.innerWidth, window.innerHeight - canvasTop); ctx.clip();
      if (this.state === 'intro' || this.state === 'moving') {
        this.drawIntro(ctx);
      } else {
        const box = this.trackerBox();
        this.drawArtwork(ctx, TRACKER, box);
        this.drawHandle(ctx, 'left', box.lineTop);
        this.drawHandle(ctx, 'right', box.lineTop);
      }
      ctx.restore();
      this.doneButton.disabled = this.state !== 'lines' || !this.finishAllowed();
    }
  }

  TrackerAlignment.TEXTS = TEXTS;
  return TrackerAlignment;
})();
