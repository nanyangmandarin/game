// On-screen touch controller (multi-touch, slide between buttons, haptics).
import { BUTTONS } from './input.js';

const BIT = Object.fromEntries(BUTTONS.map((b, i) => [b, 1 << i]));

export function isTouchDevice() {
  return (window.matchMedia && matchMedia('(pointer: coarse)').matches) || navigator.maxTouchPoints > 0;
}

export class TouchPad {
  /**
   * @param {HTMLElement} host
   * @param {(state:{buttons:number,lx:number,ly:number}) => void} onState
   * @param {(action:string, down:boolean) => void} onAction
   */
  constructor(host, onState, onAction) {
    this.onState = onState;
    this.onAction = onAction;
    this.pointers = new Map(); // pointerId -> {kind, name, el}
    this.haptics = true;
    this.el = document.createElement('div');
    this.el.id = 'touch';
    this.el.innerHTML = `
      <div class="tp tp-shoulder tp-left"><button data-b="l2">L2</button><button data-b="l1">L1</button></div>
      <div class="tp tp-shoulder tp-right"><button data-b="r1">R1</button><button data-b="r2">R2</button></div>
      <div class="tp tp-dpad" data-dpad>
        <span class="u" data-dir="up"></span><span class="r" data-dir="right"></span>
        <span class="d" data-dir="down"></span><span class="l" data-dir="left"></span><i></i>
      </div>
      <div class="tp tp-stick" data-stick><div class="knob"></div></div>
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
      </div>`;
    host.appendChild(this.el);
    this.knob = this.el.querySelector('.knob');
    const opt = { passive: false };
    this.el.addEventListener('pointerdown', (e) => this.down(e), opt);
    this.el.addEventListener('pointermove', (e) => this.move(e), opt);
    for (const t of ['pointerup', 'pointercancel', 'lostpointercapture']) this.el.addEventListener(t, (e) => this.up(e), opt);
    this.el.addEventListener('contextmenu', (e) => e.preventDefault());
  }

  setVisible(v) {
    this.el.classList.toggle('hidden', !v);
    if (!v) { this.pointers.clear(); this.emit(); }
  }

  setOptions({ opacity, haptics, stick }) {
    this.el.style.setProperty('--tp-opacity', String((opacity ?? 60) / 100));
    this.haptics = haptics !== false;
    this.el.classList.toggle('with-stick', !!stick);
  }

  buzz() {
    if (this.haptics && navigator.vibrate) navigator.vibrate(8);
  }

  target(e) {
    return e.target.closest('[data-b],[data-dpad],[data-stick],[data-act]');
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
    if (t.dataset.dpad !== undefined) {
      const p = { kind: 'dpad', el: t, dirs: [] };
      this.pointers.set(e.pointerId, p);
      this.updateDpad(p, e);
    } else if (t.dataset.stick !== undefined) {
      const p = { kind: 'stick', el: t, x: 0, y: 0 };
      this.pointers.set(e.pointerId, p);
      this.updateStick(p, e);
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
    else if (p.kind === 'stick') this.updateStick(p, e);
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
    if (p.kind === 'stick') this.knob.style.transform = '';
    this.emit();
  }

  updateDpad(p, e) {
    const r = p.el.getBoundingClientRect();
    const dx = e.clientX - (r.left + r.width / 2);
    const dy = e.clientY - (r.top + r.height / 2);
    const dead = r.width * 0.12;
    const dirs = [];
    if (Math.hypot(dx, dy) > dead) {
      const a = Math.atan2(dy, dx) * 180 / Math.PI; // -180..180, 0 = right, 90 = down
      if (a > -67.5 && a < 67.5) dirs.push('right');
      if (a > 22.5 && a < 157.5) dirs.push('down');
      if (a > 112.5 || a < -112.5) dirs.push('left');
      if (a > -157.5 && a < -22.5) dirs.push('up');
    }
    if (dirs.join() !== p.dirs.join()) {
      if (dirs.length && dirs.some((d) => !p.dirs.includes(d))) this.buzz();
      p.dirs = dirs;
    }
  }

  updateStick(p, e) {
    const r = p.el.getBoundingClientRect();
    const rad = r.width / 2;
    let dx = (e.clientX - (r.left + rad)) / rad;
    let dy = (e.clientY - (r.top + rad)) / rad;
    const m = Math.hypot(dx, dy);
    if (m > 1) { dx /= m; dy /= m; }
    p.x = dx;
    p.y = dy;
    this.knob.style.transform = `translate(${dx * rad * 0.6}px, ${dy * rad * 0.6}px)`;
  }

  emit() {
    let buttons = 0;
    let lx = 0, ly = 0;
    this.el.querySelectorAll('.on').forEach((x) => { if (!x.dataset.act) x.classList.remove('on'); });
    for (const p of this.pointers.values()) {
      if (p.kind === 'btn') { buttons |= BIT[p.name]; p.el.classList.add('on'); }
      if (p.kind === 'dpad') for (const d of p.dirs) { buttons |= BIT[d]; p.el.querySelector(`[data-dir=${d}]`).classList.add('on'); }
      if (p.kind === 'stick') { lx = p.x; ly = p.y; }
    }
    this.onState({ buttons, lx, ly });
  }
}
