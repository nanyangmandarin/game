// GPU thread: rasterizes the command stream recorded by the emulator worker and presents
// frames to the (Offscreen)canvas. Runs in parallel with CPU emulation on multi-core devices.
import init, { GpuBackend } from '../pkg/ps1core.js';
import { Painter } from './painter.js';
import { HwRenderer } from './hw.js';
import { Ring } from './ring.js';

const REC_FRAME_DONE = 3; // ctrl index: frames completed
const SYNC_DONE = 2;      // ctrl index: sync requests completed

let wasm = null;
let be = null;
let ring = null;
let vramShared = null;
let painter = null;
let fullVram = false;
let hwWanted = false;
const perf = { draw: 0, n: 0, last: performance.now() };

function hwFrame(show) {
  // replay the draw list even for skipped frames: later frames build on its results
  const n = be.hw_len();
  painter.render(new Uint32Array(wasm.memory.buffer, be.hw_ptr(), n), new Uint16Array(wasm.memory.buffer, be.vram_ptr(), 1024 * 512));
  be.hw_clear();
  if (show) painter.show(Array.from(be.display_info()));
}

function present() {
  if (!painter) return;
  const t = performance.now();
  const info = painter.gl && !fullVram ? be.display_info() : null;
  if (info && !info[4]) {
    painter.frameRaw(info, new Uint8Array(wasm.memory.buffer, be.vram_ptr(), 1024 * 512 * 2));
  } else if (be.render(fullVram)) {
    painter.frame(be.frame_width(), be.frame_height(), new Uint8ClampedArray(wasm.memory.buffer, be.frame_ptr(), be.frame_len()));
  }
  perf.draw += performance.now() - t;
  perf.n++;
}

function pump() {
  const until = performance.now() + 12;
  for (;;) {
    const chunk = ring.read();
    if (!chunk) {
      if (performance.now() > until) break;
      ring.waitData(4);
      continue;
    }
    be.feed(chunk);
    for (let ev; (ev = be.step()) !== 0;) {
      if (ev === 1) {
        if (painter instanceof HwRenderer) {
          const t = performance.now();
          hwFrame(be.present());
          perf.draw += performance.now() - t;
          perf.n++;
        } else if (be.present()) present();
        ring.signal(REC_FRAME_DONE);
      } else if (ev === 2) {
        vramShared.set(new Uint16Array(wasm.memory.buffer, be.vram_ptr(), 1024 * 512));
        ring.signal(SYNC_DONE);
      }
    }
  }
  const now = performance.now();
  if (now - perf.last > 1000) {
    postMessage({ type: 'gstats', drawMs: perf.n ? perf.draw / perf.n : 0 });
    perf.draw = perf.n = 0;
    perf.last = now;
  }
  setTimeout(pump, 0); // yield so canvas/resize messages are handled
}

onmessage = async (e) => {
  const m = e.data;
  try {
    switch (m.type) {
      case 'init':
        ring = new Ring(m.ringSab);
        vramShared = new Uint16Array(m.vramSab);
        wasm = await init({ module_or_path: new URL('../pkg/ps1core_bg.wasm', import.meta.url) });
        be = new GpuBackend();
        if (hwWanted) be.hw_enable(true);
        pump();
        break;
      case 'canvas':
        painter = null;
        if (m.canvas && m.kind === 'gpu' && (m.hwScale || 1) > 1) {
          try {
            painter = new HwRenderer(m.canvas, m.hwScale);
            painter.setOptions(m.opts || {});
            painter.setSize(m.w, m.h);
            be && be.hw_enable(true);
            hwWanted = true;
            postMessage({ type: 'renderer', mode: 'gpu', hd: painter.scale });
            break;
          } catch (err) {
            // the canvas may already hold a WebGL2 context: ask the page for a fresh one
            postMessage({ type: 'error', message: 'Resolusi tinggi tidak tersedia (' + (err.message || err) + '), memakai resolusi asli.' });
            postMessage({ type: 'renderer', mode: 'software', failed: true });
            painter = null;
            hwWanted = false;
            be && be.hw_enable(false);
            break;
          }
        }
        hwWanted = false;
        be && be.hw_enable(false);
        painter = m.canvas ? new Painter(m.canvas, m.kind) : null;
        if (painter && !painter.failed) {
          painter.setOptions(m.opts || {});
          painter.setSize(m.w, m.h);
        }
        postMessage({ type: 'renderer', mode: painter ? painter.mode : 'software', failed: !!painter?.failed });
        if (painter?.failed) painter = null;
        break;
      case 'resize':
        painter?.setSize(m.w, m.h);
        break;
      case 'present':
        painter?.setOptions(m.opts);
        break;
      case 'opts':
        fullVram = !!m.fullVram;
        break;
    }
  } catch (err) {
    postMessage({ type: 'error', message: 'GPU thread: ' + String(err?.message || err) });
  }
};
