// Keyboard + Gamepad input with rebindable mappings.

export const BUTTONS = ['select', 'l3', 'r3', 'start', 'up', 'right', 'down', 'left', 'l2', 'r2', 'l1', 'r1', 'triangle', 'circle', 'cross', 'square'];
export const BUTTON_LABELS = {
  up: 'D-Pad Atas', down: 'D-Pad Bawah', left: 'D-Pad Kiri', right: 'D-Pad Kanan',
  cross: '✕ Cross', circle: '○ Circle', square: '□ Square', triangle: '△ Triangle',
  l1: 'L1', r1: 'R1', l2: 'L2', r2: 'R2', l3: 'L3', r3: 'R3', start: 'Start', select: 'Select',
};
export const AXES = ['lx-', 'lx+', 'ly-', 'ly+', 'rx-', 'rx+', 'ry-', 'ry+'];
export const AXIS_LABELS = {
  'lx-': 'Stik Kiri ←', 'lx+': 'Stik Kiri →', 'ly-': 'Stik Kiri ↑', 'ly+': 'Stik Kiri ↓',
  'rx-': 'Stik Kanan ←', 'rx+': 'Stik Kanan →', 'ry-': 'Stik Kanan ↑', 'ry+': 'Stik Kanan ↓',
};

export const DEFAULT_KEYS = [
  {
    up: 'KeyW', down: 'KeyS', left: 'KeyA', right: 'KeyD',
    triangle: 'KeyI', square: 'KeyJ', cross: 'KeyK', circle: 'KeyL',
    l1: 'KeyQ', r1: 'KeyE', l2: 'Digit1', r2: 'Digit3', l3: 'KeyZ', r3: 'KeyX',
    start: 'Enter', select: 'Backspace',
    'lx-': 'ArrowLeft', 'lx+': 'ArrowRight', 'ly-': 'ArrowUp', 'ly+': 'ArrowDown',
    'rx-': 'Numpad4', 'rx+': 'Numpad6', 'ry-': 'Numpad8', 'ry+': 'Numpad2',
  },
  {},
];

// Standard Gamepad mapping indices
export const DEFAULT_PAD = {
  cross: 0, circle: 1, square: 2, triangle: 3, l1: 4, r1: 5, l2: 6, r2: 7,
  select: 8, start: 9, l3: 10, r3: 11, up: 12, down: 13, left: 14, right: 15,
};

export const DEFAULT_HOTKEYS = {
  pause: 'KeyP', reset: 'F3', saveState: 'F5', loadState: 'F8', rewind: 'KeyR', fastForward: 'Tab', fullscreen: 'F2', screenshot: 'F9', analog: 'F4', menu: 'Escape',
};
export const HOTKEY_LABELS = {
  pause: 'Pause / Lanjut', reset: 'Reset', saveState: 'Quick save state', loadState: 'Quick load state', rewind: 'Rewind (tahan)', fastForward: 'Fast-forward (tahan)', fullscreen: 'Layar penuh',
  screenshot: 'Screenshot', analog: 'Toggle Analog (Port 1)', menu: 'Buka/tutup menu',
};

export function keyLabel(code) {
  if (code === null || code === undefined || code === '') return '—';
  if (typeof code === 'number') return 'Tombol ' + code;
  const arrows = { ArrowUp: '↑', ArrowDown: '↓', ArrowLeft: '←', ArrowRight: '→' };
  if (arrows[code]) return arrows[code];
  return code.replace(/^Key/, '').replace(/^Digit/, '').replace(/^Numpad/, 'Num ');
}

export class Input {
  constructor(getSettings, onChange, onHotkey) {
    this.getSettings = getSettings;
    this.onChange = onChange;
    this.onHotkey = onHotkey;
    this.down = new Set();
    this.capture = null;
    this.prev = '';
    this.prevPadButtons = new Map();
    this.touch = { buttons: 0, lx: 0, ly: 0 };
    window.addEventListener('keydown', (e) => this.key(e, true));
    window.addEventListener('keyup', (e) => this.key(e, false));
    window.addEventListener('blur', () => this.down.clear());
    const loop = () => { this.poll(); requestAnimationFrame(loop); };
    requestAnimationFrame(loop);
  }

  /** Capture the next key or gamepad button for rebinding. cb(kind, code) */
  captureNext(cb) {
    this.capture = cb;
  }

  key(e, isDown) {
    if (this.capture && isDown) {
      e.preventDefault();
      const cb = this.capture;
      this.capture = null;
      cb('key', e.code === 'Escape' ? null : e.code);
      return;
    }
    const tag = (e.target && e.target.tagName) || '';
    if (tag === 'INPUT' || tag === 'SELECT' || tag === 'TEXTAREA') return;
    const s = this.getSettings();
    const hk = Object.entries(s.hotkeys).find(([, c]) => c === e.code);
    if (hk) {
      e.preventDefault();
      if (!e.repeat || hk[0] === 'fastForward' || hk[0] === 'rewind') this.onHotkey(hk[0], isDown);
      return;
    }
    const mapped = s.keys.some((m) => Object.values(m).includes(e.code));
    if (mapped) e.preventDefault();
    if (isDown) this.down.add(e.code); else this.down.delete(e.code);
  }

  poll() {
    const s = this.getSettings();
    const pads = navigator.getGamepads ? Array.from(navigator.getGamepads()) : [];
    // rebinding with a gamepad button
    if (this.capture) {
      for (const gp of pads) {
        if (!gp) continue;
        const i = gp.buttons.findIndex((b) => b.pressed);
        if (i >= 0) {
          const cb = this.capture;
          this.capture = null;
          cb('pad', i);
          return;
        }
      }
    }
    const state = [0, 1].map((p) => {
      let buttons = 0;
      const axes = { lx: 0, ly: 0, rx: 0, ry: 0 };
      const km = s.keys[p] || {};
      BUTTONS.forEach((b, bit) => { if (km[b] && this.down.has(km[b])) buttons |= 1 << bit; });
      for (const a of AXES) {
        if (km[a] && this.down.has(km[a])) {
          const k = a.slice(0, 2);
          axes[k] += a.endsWith('+') ? 1 : -1;
        }
      }
      const gi = s.gamepadIndex[p];
      const gp = gi >= 0 ? pads[gi] : null;
      if (gp) {
        const pm = s.padMap[p] || DEFAULT_PAD;
        BUTTONS.forEach((b, bit) => {
          const idx = pm[b];
          const btn = idx !== undefined ? gp.buttons[idx] : null;
          if (btn && (btn.pressed || btn.value > 0.5)) buttons |= 1 << bit;
        });
        const dz = s.deadzone / 100;
        const ax = (i) => { const v = gp.axes[i] || 0; return Math.abs(v) < dz ? 0 : (v - Math.sign(v) * dz) / (1 - dz); };
        axes.lx += ax(0); axes.ly += ax(1); axes.rx += ax(2); axes.ry += ax(3);
        // Guide/Home button toggles analog mode
        const home = gp.buttons[16]?.pressed;
        const key = 'home' + p;
        if (home && !this.prevPadButtons.get(key)) this.onHotkey('analogPort' + p, true);
        this.prevPadButtons.set(key, !!home);
      }
      if (p === 0) {
        buttons |= this.touch.buttons;
        axes.lx += this.touch.lx;
        axes.ly += this.touch.ly;
      }
      const conv = (v) => Math.max(0, Math.min(255, Math.round(128 + Math.max(-1, Math.min(1, v)) * 127.5)));
      return { buttons, lx: conv(axes.lx), ly: conv(axes.ly), rx: conv(axes.rx), ry: conv(axes.ry) };
    });
    const sig = JSON.stringify(state);
    if (sig !== this.prev) {
      this.prev = sig;
      this.onChange(state);
    }
  }

  setTouch(t) {
    this.touch = t;
  }

  rumble(port, small, large) {
    const s = this.getSettings();
    if (!s.rumble) return;
    const gi = s.gamepadIndex[port];
    const gp = gi >= 0 && navigator.getGamepads ? navigator.getGamepads()[gi] : null;
    const act = gp && gp.vibrationActuator;
    if (!act) return;
    if (!small && !large) { act.reset?.(); return; }
    act.playEffect?.('dual-rumble', { duration: 200, strongMagnitude: large / 255, weakMagnitude: small / 255 }).catch(() => {});
  }
}
