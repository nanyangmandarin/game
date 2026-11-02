// Video output manager. Preferred path: the canvas is transferred to the emulator worker
// (OffscreenCanvas) so frames are drawn right where they are produced: no cross-thread hop,
// no per-frame copy, steadier frame pacing. Fallback: frames are posted to the main thread.
import { Painter } from './painter.js';

export const offscreenSupported =
  typeof HTMLCanvasElement !== 'undefined' && 'transferControlToOffscreen' in HTMLCanvasElement.prototype &&
  typeof OffscreenCanvas !== 'undefined';

export class Presenter {
  constructor(host, worker) {
    this.host = host;
    this.worker = worker;
    this.canvas = null;
    this.painter = null; // main-thread fallback only
    this.offscreen = false;
    this.mode = null; // renderer actually in use: 'software' | 'gpu'
    this.kind = null; // renderer requested
    this.opts = {};
    this.drawMs = 0;
    this.onModeChange = null;
    this.snapWaiters = [];
    this.ro = new ResizeObserver(() => this.resize());
    this.ro.observe(host);
  }

  size() {
    const r = this.host.getBoundingClientRect();
    const dpr = window.devicePixelRatio || 1;
    return [Math.max(1, Math.round(r.width * dpr)), Math.max(1, Math.round(r.height * dpr))];
  }

  /** Switch renderer ('software' | 'gpu'). */
  setRenderer(kind, force = false, hwScale = this.hwScale || 1) {
    if (this.kind === kind && this.hwScale === hwScale && this.canvas && !force) return this.mode;
    this.kind = kind;
    this.hwScale = hwScale;
    this.hd = 0;
    if (this.canvas) this.canvas.remove();
    this.canvas = document.createElement('canvas');
    this.canvas.className = 'screen';
    this.host.appendChild(this.canvas);
    const [w, h] = this.size();
    if (offscreenSupported) {
      const off = this.canvas.transferControlToOffscreen();
      this.offscreen = true;
      this.painter = null;
      this.mode = kind; // confirmed asynchronously by the worker
      this.worker.postMessage({ type: 'canvas', canvas: off, kind, w, h, opts: this.opts, hwScale: kind === 'gpu' ? hwScale : 1 }, [off]);
    } else {
      this.offscreen = false;
      this.canvas.width = w;
      this.canvas.height = h;
      this.painter = new Painter(this.canvas, kind);
      if (this.painter.failed) return this.setRenderer('software', true);
      this.painter.setOptions(this.opts);
      this.mode = this.painter.mode;
      this.worker.postMessage({ type: 'canvas', canvas: null });
    }
    return this.mode;
  }

  /** Called with the worker's 'renderer' reply. */
  confirmMode(mode, failed, hd) {
    if (failed) this.setRenderer('software', true, 1);
    else { this.mode = mode; this.hd = hd || 0; }
    this.onModeChange && this.onModeChange(this.mode);
  }

  setOptions(o) {
    Object.assign(this.opts, o);
    if (this.offscreen) this.worker.postMessage({ type: 'present', opts: this.opts });
    else if (this.painter) this.painter.setOptions(this.opts);
  }

  resize() {
    if (!this.canvas) return;
    const [w, h] = this.size();
    if (this.offscreen) this.worker.postMessage({ type: 'resize', w, h });
    else if (this.painter) this.painter.setSize(w, h);
  }

  redraw() { this.resize(); }

  /** Fallback path: frame posted from the worker. */
  frame(w, h, buffer) {
    if (!this.painter) return;
    this.painter.frame(w, h, new Uint8ClampedArray(buffer));
    this.drawMs = this.painter.drawMs;
  }

  /** Latest frame as {w, h, data}. */
  snapshot() {
    if (!this.offscreen) return Promise.resolve(this.painter ? this.painter.snapshot() : null);
    return new Promise((res) => {
      this.snapWaiters.push(res);
      this.worker.postMessage({ type: 'snapshot' });
    });
  }

  gotSnapshot(snap) {
    const w = this.snapWaiters.shift();
    w && w(snap);
  }

  async screenshot() {
    const f = await this.snapshot();
    if (!f) return null;
    const c = document.createElement('canvas');
    c.width = f.w;
    c.height = f.h;
    c.getContext('2d').putImageData(new ImageData(new Uint8ClampedArray(f.data.buffer || f.data), f.w, f.h), 0, 0);
    return new Promise((res) => c.toBlob(res, 'image/png'));
  }
}
