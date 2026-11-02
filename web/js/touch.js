// On-screen touch controller: buttons (multi-touch, slide between buttons, haptics),
// selectable direction control (D-pad / joystick / floating joystick) and gestures
// directly on the game picture (tap, two-finger tap, hold, drag = direction).
import { BUTTONS } from './input.js';

const BIT = Object.fromEntries(BUTTONS.map((b, i) => [b, 1 << i]));
const TAP_MS = 220;       // max duration of a tap
const HOLD_MS = 260;      // finger kept still this long = hold the tap button
const PULSE_MS = 90;      // how long a tap presses the button
const TWO_FINGER_MS = 200;

export function isTouchDevice() {
  return (window.matchMedia && matchMedia('(pointer: coarse)').matches) || navigator.maxTouchPoints > 0;
}

/** Map a vector (|v| <= 1) to 8-way directions and analog axes. */
function vecToDirs(x, y, dead) {
  const dirs = [];
  const m = Math.hypot(x, y);
  if (m > dead) {
    const a = Math.atan2(y, x) * 180 / Math.PI; // 0 = right, 90 = down
    if (a > -67.5 && a < 67.5) dirs.push('right');
    if (a > 22.5 && a < 157.5) dirs.push('down');
    if (a > 112.5 || a < -112.5) dirs.push('left');
    if (a > -157.5 && a < -22.5) dirs.push('up');
  }
  return dirs;
}

export class TouchPad {
  /**
   * @param {HTMLElement} host
   * @param {(state:{buttons:number,lx:number,ly:number}) => void} onState
   * @param {(action:string, down:boolean) => void} onAction
   */
  constructor(host, onState, onAction) {
    this.host = host;
    this.onState = onState;
    this.onAction = onAction;
    this.pointers = new Map(); // pointerId -> state
    this.pulses = new Map();   // button -> release time (taps)
    this.opts = {
      haptics: true, dir: 'dpad', gestures: false, gestureOnly: false,
      tapButton: 'cross', tap2Button: 'circle',
    };
    this.el = document.createElement('div');
    this.el.id = 'touch';
    this.el.innerHTML = `
      <div class="tp-gesture" data-gesture></div>
      <div class="tp tp-shoulder tp-left"><button data-b="l2">L2</button><button data-b="l1">L1</button></div>
      <div class="tp tp-shoulder tp-right"><button data-b="r1">R1</button><button data-b="r2">R2</button></div>
      <div class="tp tp-dpad" data-dpad>
        <span class="u" data-dir="up"></span><span class="r" data-dir="right"></span>
        <span class="d" data-dir="down"></span><span class="l" data-dir="left"></span><i></i>
      </div>
      <div class="tp tp-joy" data-joy><div class="ring"><div class="knob"></div><b>ANALOG</b></div></div>
      <div class="tp tp-face">
        <button data-b="triangle" class="tri">△</button><button data-b="square" class="sq">□</button>
        <button data-b="circle" class="ci">○</button><button data-b="cross" class="cr">✕</button>
      </div>
      <div class="tp tp-mid"><button data-b="select">SELECT</button><button data-b="start">START</button></div>
      <div class="tp tp-sys">
        <button data-act="menu" title="Menu">☰</button>
        <button data-act="ff" title="Fast-forward (tahan)">⏩</button>
        <button data-act="save" title="Quick save">💾</button>
        <button data-act="load" title="Quick load">↺</button>
        <button data-act="analog" class="tp-analog" title="Mode analog DualShock (seperti tombol ANALOG)">A</button>
      </div>
      <div class="tp-gknob hidden"><i></i></div>`;
    host.appendChild(this.el);
    this.joy = this.el.querySelector('.tp-joy');
    this.joyRing = this.el.querySelector('.tp-joy .ring');
    this.joyKnob = this.el.querySelector('.tp-joy .knob');
    this.gknob = this.el.querySelector('.tp-gknob');
    const opt = { passive: false };
    this.el.addEventListener('pointerdown', (e) => this.down(e), opt);
    this.el.addEventListener('pointermove', (e) => this.move(e), opt);
    for (const t of ['pointerup', 'pointercancel', 'lostpointercapture']) this.el.addEventListener(t, (e) => this.up(e), opt);
    this.el.addEventListener('contextmenu', (e) => e.preventDefault());
  }

  setVisible(v) {
    this.el.classList.toggle('hidden', !v);
    if (!v) { this.pointers.clear(); this.pulses.clear(); this.emit(); }
  }

  setOptions(o) {
    Object.assign(this.opts, o);
    this.el.style.setProperty('--tp-opacity', String((o.opacity ?? 60) / 100));
    this.el.classList.toggle('dir-joy', this.opts.dir !== 'dpad');
    this.el.classList.toggle('joy-float', this.opts.dir.endsWith('float'));
    this.el.classList.toggle('joy-analog', this.opts.dir.startsWith('analog'));
    this.el.classList.toggle('gestures', !!this.opts.gestures);
    this.el.classList.toggle('gesture-only', !!this.opts.gestures && !!this.opts.gestureOnly);
    this.resetJoy();
  }

  buzz() {
    if (this.opts.haptics && navigator.vibrate) navigator.vibrate(8);
  }

  target(e) {
    return e.target.closest('[data-b],[data-dpad],[data-joy],[data-act],[data-gesture]');
  }

  down(e) {
    const t = this.target(e);
    if (!t) return;
    e.preventDefault();
    try { this.el.setPointerCapture(e.pointerId); } catch { /* ignore */ }
    if (t.dataset.act) {
      this.pointers.set(e.pointerId, { kind: 'act', name: t.dataset.act, el: t });
      t.classList.add('on');
      this.onAction(t.dataset.act, true);
      this.buzz();
      return;
    }
    if (t.dataset.gesture !== undefined) return this.gestureDown(e);
    if (t.dataset.dpad !== undefined) {
      const p = { kind: 'dpad', el: t, dirs: [] };
      this.pointers.set(e.pointerId, p);
      this.updateDpad(p, e);
    } else if (t.dataset.joy !== undefined) {
      const p = { kind: 'joy', el: t, dirs: [], x: 0, y: 0 };
      if (this.opts.dir.endsWith('float')) this.placeJoy(e);
      this.pointers.set(e.pointerId, p);
      this.updateJoy(p, e);
    } else {
      this.pointers.set(e.pointerId, { kind: 'btn', name: t.dataset.b, el: t });
    }
    this.buzz();
    this.emit();
  }

  move(e) {
    const p = this.pointers.get(e.pointerId);
    if (!p) return;
    e.preventDefault();
    if (p.kind === 'dpad') this.updateDpad(p, e);
    else if (p.kind === 'joy') this.updateJoy(p, e);
    else if (p.kind === 'gesture') this.gestureMove(p, e);
    else if (p.kind === 'btn') {
      // allow sliding a finger across face/shoulder buttons
      const under = document.elementFromPoint(e.clientX, e.clientY);
      const b = under && under.closest && under.closest('#touch [data-b]');
      if (b && b.dataset.b !== p.name) {
        p.name = b.dataset.b;
        p.el = b;
        this.buzz();
      }
    } else return;
    this.emit();
  }

  up(e) {
    const p = this.pointers.get(e.pointerId);
    if (!p) return;
    this.pointers.delete(e.pointerId);
    if (p.kind === 'act') {
      p.el.classList.remove('on');
      this.onAction(p.name, false);
      return;
    }
    if (p.kind === 'gesture') this.gestureUp(p, e);
    if (p.kind === 'joy') this.resetJoy();
    this.emit();
  }

  // ---------------- direction controls ----------------

  updateDpad(p, e) {
    const r = p.el.getBoundingClientRect();
    const dx = (e.clientX - (r.left + r.width / 2)) / (r.width / 2);
    const dy = (e.clientY - (r.top + r.height / 2)) / (r.height / 2);
    const dirs = vecToDirs(dx, dy, 0.24);
    if (dirs.some((d) => !p.dirs.includes(d))) this.buzz();
    p.dirs = dirs;
  }

  placeJoy(e) {
    // floating joystick: centre the ring under the finger (inside the joystick zone)
    const z = this.joy.getBoundingClientRect();
    const rr = this.joyRing.getBoundingClientRect();
    const cx = Math.min(Math.max(e.clientX, z.left + rr.width / 2), z.right - rr.width / 2);
    const cy = Math.min(Math.max(e.clientY, z.top + rr.height / 2), z.bottom - rr.height / 2);
    this.joyRing.style.left = `${cx - z.left - rr.width / 2}px`;
    this.joyRing.style.top = `${cy - z.top - rr.height / 2}px`;
    this.joyRing.classList.add('active');
  }

  resetJoy() {
    if (!this.joyRing) return;
    this.joyKnob.style.transform = '';
    this.joyRing.classList.remove('active');
    this.joyRing.style.left = '';
    this.joyRing.style.top = '';
  }

  updateJoy(p, e) {
    const r = this.joyRing.getBoundingClientRect();
    const rad = r.width / 2;
    let x = (e.clientX - (r.left + rad)) / rad;
    let y = (e.clientY - (r.top + rad)) / rad;
    const m = Math.hypot(x, y);
    if (m > 1) { x /= m; y /= m; }
    p.x = x;
    p.y = y;
    const dirs = vecToDirs(x, y, 0.3);
    if (dirs.some((d) => !p.dirs.includes(d))) this.buzz();
    p.dirs = dirs;
    this.joyKnob.style.transform = `translate(${x * rad * 0.55}px, ${y * rad * 0.55}px)`;
  }

  // ---------------- gestures on the game picture ----------------

  gestureDown(e) {
    const now = performance.now();
    const others = [...this.pointers.values()].filter((p) => p.kind === 'gesture' && !p.moved && now - p.t0 < TWO_FINGER_MS);
    const p = { kind: 'gesture', x0: e.clientX, y0: e.clientY, t0: now, moved: false, hold: false, x: 0, y: 0, dirs: [], two: false };
    if (others.length) {
      // second finger: this pair becomes a two-finger tap candidate
      p.two = true;
      for (const o of others) { o.two = true; clearTimeout(o.timer); }
    } else {
      p.timer = setTimeout(() => {
        if (!p.moved && !p.two && this.pointers.get(e.pointerId) === p) {
          p.hold = true; // keep the tap button pressed while the finger stays down
          this.buzz();
          this.emit();
        }
      }, HOLD_MS);
    }
    this.pointers.set(e.pointerId, p);
  }

  gestureMove(p, e) {
    const dx = e.clientX - p.x0;
    const dy = e.clientY - p.y0;
    const radius = Math.max(40, Math.min(innerWidth, innerHeight) * 0.09);
    if (!p.moved && Math.hypot(dx, dy) > radius * 0.25 && !p.two) {
      p.moved = true;
      p.hold = false;
      clearTimeout(p.timer);
      this.gknob.classList.remove('hidden');
    }
    if (!p.moved) return;
    let x = dx / radius, y = dy / radius;
    const m = Math.hypot(x, y);
    if (m > 1) { x /= m; y /= m; }
    p.x = x;
    p.y = y;
    const dirs = vecToDirs(x, y, 0.35);
    if (dirs.some((d) => !p.dirs.includes(d))) this.buzz();
    p.dirs = dirs;
    const hr = this.el.getBoundingClientRect();
    this.gknob.style.left = `${p.x0 - hr.left}px`;
    this.gknob.style.top = `${p.y0 - hr.top}px`;
    this.gknob.firstElementChild.style.transform = `translate(${x * 28}px, ${y * 28}px)`;
  }

  gestureUp(p) {
    clearTimeout(p.timer);
    this.gknob.classList.add('hidden');
    const quick = performance.now() - p.t0 < TAP_MS + (p.two ? 120 : 0);
    if (!p.moved && !p.hold && quick) {
      if (p.two) {
        // fire once per two-finger tap (when the last of the pair lifts)
        const rest = [...this.pointers.values()].some((o) => o.kind === 'gesture' && o.two);
        if (!rest) this.pulse(this.opts.tap2Button);
      } else {
        this.pulse(this.opts.tapButton);
      }
    }
  }

  pulse(name) {
    if (!BIT[name]) return;
    this.pulses.set(name, performance.now() + PULSE_MS);
    this.buzz();
    this.emit();
    setTimeout(() => this.emit(), PULSE_MS + 5);
  }

  /** Light the ANALOG button like the LED on a real DualShock. */
  setAnalogLed(on) {
    this.el.querySelector('.tp-analog')?.classList.toggle('lit', !!on);
  }

  // ---------------- output ----------------

  emit() {
    let buttons = 0;
    let lx = 0, ly = 0;
    const now = performance.now();
    this.el.querySelectorAll('.on').forEach((x) => { if (!x.dataset.act) x.classList.remove('on'); });
    // analog styles send the left analog stick; D-pad/joystick styles send digital directions
    const out = this.opts.dir.startsWith('analog') ? 'analog' : 'dpad';
    const addDir = (p) => {
      if (out !== 'analog') for (const d of p.dirs) buttons |= BIT[d];
      if (out !== 'dpad') { lx += p.x; ly += p.y; }
    };
    for (const p of this.pointers.values()) {
      if (p.kind === 'btn') { buttons |= BIT[p.name]; p.el.classList.add('on'); }
      if (p.kind === 'dpad') for (const d of p.dirs) { buttons |= BIT[d]; p.el.querySelector(`[data-dir=${d}]`).classList.add('on'); }
      if (p.kind === 'joy') addDir(p);
      if (p.kind === 'gesture') {
        if (p.moved) addDir(p);
        if (p.hold) buttons |= BIT[this.opts.tapButton] || 0;
      }
    }
    for (const [name, until] of this.pulses) {
      if (until > now) buttons |= BIT[name];
      else this.pulses.delete(name);
    }
    this.onState({ buttons, lx: Math.max(-1, Math.min(1, lx)), ly: Math.max(-1, Math.min(1, ly)) });
  }
}
