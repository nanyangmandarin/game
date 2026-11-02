// Emulator worker: owns the WASM core, paces frames, streams video/audio to the UI.
import init, { Emulator } from '../pkg/ps1core.js';
import { Painter } from './painter.js';
import { Ring, createRingBuffer } from './ring.js';

let painter = null; // draws directly when the page transferred an OffscreenCanvas

// ---- multi-threaded GPU (needs cross-origin isolation for SharedArrayBuffer) ----
const REC_FRAME = 3;
let gpuWorker = null;
let ring = null;
let vramShared = null;
let syncTarget = 0;
let framesSent = 0;
let gpuDrawMs = null;

function startGpuThread() {
  const ringSab = createRingBuffer();
  const vramSab = new SharedArrayBuffer(1024 * 512 * 2);
  ring = new Ring(ringSab);
  vramShared = new Uint16Array(vramSab);
  gpuWorker = new Worker(new URL('./gpu-worker.js', import.meta.url), { type: 'module' });
  gpuWorker.onmessage = (e) => {
    const m = e.data;
    if (m.type === 'gstats') gpuDrawMs = m.drawMs;
    else postMessage(m); // renderer / error -> page
  };
  gpuWorker.onerror = (e) => postMessage({ type: 'error', message: 'GPU thread gagal: ' + (e.message || '') });
  gpuWorker.postMessage({ type: 'init', ringSab, vramSab });
}

/** Called by the core when it needs current VRAM (VRAM->CPU reads, save states, snapshots). */
function gpuSync(out, vram) {
  ring.push(out); // ends with a SYNC record
  syncTarget = (syncTarget + 1) | 0;
  if (!ring.waitCounter(2, syncTarget, 2000)) {
    postMessage({ type: 'error', message: 'GPU thread tidak merespons (sync).' });
    return;
  }
  vram.set(vramShared);
}

/** Send this frame's recorded GPU commands plus a frame-end marker to the GPU thread. */
function flushGpu(present) {
  const n = emu.gpu_out_len();
  if (n) ring.push(new Uint32Array(wasm.memory.buffer, emu.gpu_out_ptr(), n));
  emu.gpu_out_clear();
  ring.push(new Uint32Array([(REC_FRAME << 24) | 1, present ? 1 : 0]));
  framesSent = (framesSent + 1) | 0;
  // keep at most 2 frames in flight so input latency stays low
  ring.waitCounter(3, (framesSent - 2) | 0, 200);
}

let wasm = null;
let emu = null;
let running = false;
let paused = false;
let speed = 1;           // 0 = unlimited
let ffActive = false;
let ffSpeed = 3;
let fullVram = false;
let frameskip = 0;
let analogToDpad = true;
let audioPort = null;
let audioFill = 0;
let audioEnabled = true;
let muteOnFF = true;
let audioTarget = 8820; // floats (stereo-interleaved) ~100ms
let nextFrame = 0;
let timer = null;
let lastRumble = [0, 0];
let frameCount = 0;
let statFrames = 0;
let statStart = 0;
let gameId = 'shared';
const perf = { emu: 0, render: 0, disc: 0, frames: 0, presented: 0, lastShow: 0, late: 0, maxGap: 0 };

// ---------------- disc access (synchronous, block cached) ----------------
const reader = new FileReaderSync();
let discFiles = [];
const BLOCK = 128 * 1024;
const cache = new Map();

function readDisc(file, offset, len) {
  const t0 = performance.now();
  try { return readDiscInner(file, offset, len); } finally { perf.disc += performance.now() - t0; }
}

function readDiscInner(file, offset, len) {
  const f = discFiles[file];
  if (!f) return new Uint8Array(0);
  const out = new Uint8Array(len);
  let pos = 0;
  while (pos < len) {
    const abs = offset + pos;
    const bi = Math.floor(abs / BLOCK);
    const key = file + ':' + bi;
    let blk = cache.get(key);
    if (!blk) {
      const start = bi * BLOCK;
      if (start >= f.size) break;
      blk = new Uint8Array(reader.readAsArrayBuffer(f.slice(start, Math.min(f.size, start + BLOCK))));
      cache.set(key, blk);
      if (cache.size > 128) cache.delete(cache.keys().next().value);
    } else {
      cache.delete(key);
      cache.set(key, blk);
    }
    const o = abs - bi * BLOCK;
    const n = Math.min(len - pos, blk.length - o);
    if (n <= 0) break;
    out.set(blk.subarray(o, o + n), pos);
    pos += n;
  }
  return pos === len ? out : out.subarray(0, pos);
}

// ---------------- memory cards / save states ----------------
function flushCards() {
  for (let s = 0; s < 2; s++) {
    if (emu.memcard_take_dirty(s)) {
      const data = emu.memcard_get(s);
      postMessage({ type: 'memcard', slot: s, gameId, data }, [data.buffer]);
    }
  }
}

async function gzip(u8) {
  if (typeof CompressionStream === 'undefined') return { data: u8, gz: false };
  const s = new Blob([u8]).stream().pipeThrough(new CompressionStream('gzip'));
  return { data: new Uint8Array(await new Response(s).arrayBuffer()), gz: true };
}

async function gunzip(u8, gz) {
  if (!gz) return u8;
  const s = new Blob([u8]).stream().pipeThrough(new DecompressionStream('gzip'));
  return new Uint8Array(await new Response(s).arrayBuffer());
}

// ---------------- frame loop ----------------
function fps() {
  return emu && emu.is_pal() ? 50 : 59.94;
}

function runOne(present) {
  const t0 = performance.now();
  emu.run_frame();
  perf.emu += performance.now() - t0;
  perf.frames++;
  frameCount++;
  statFrames++;

  // audio
  const n = emu.audio_len();
  if (audioPort && n > 0 && audioEnabled && !(ffActive && muteOnFF)) {
    const src = new Int16Array(wasm.memory.buffer, emu.audio_ptr(), n);
    const f = new Float32Array(n);
    for (let i = 0; i < n; i++) f[i] = src[i] / 32768;
    audioPort.postMessage(f, [f.buffer]);
  }

  // rumble
  for (let p = 0; p < 2; p++) {
    const r = emu.rumble(p);
    if (r !== lastRumble[p]) {
      lastRumble[p] = r;
      postMessage({ type: 'rumble', port: p, small: (r >> 8) & 0xff, large: r & 0xff });
    }
  }

  // memory cards: flush writes about twice a second
  if (frameCount % 30 === 0) flushCards();

  if (present && frameskip > 0 && frameCount % (frameskip + 1) !== 0) present = false;
  if (gpuWorker) {
    const t1 = performance.now();
    if (present) {
      perf.presented++;
      if (perf.lastShow) {
        const gap = t1 - perf.lastShow;
        if (gap > 1500 / fps()) perf.late++;
        if (gap > perf.maxGap) perf.maxGap = gap;
      }
      perf.lastShow = t1;
    }
    flushGpu(present);
    perf.render += performance.now() - t1;
    return;
  }
  if (present) {
    const t1 = performance.now();
    perf.presented++;
    // frame pacing: a presented frame arriving >1.5 frame periods after the previous one is "late"
    if (perf.lastShow) {
      const gap = t1 - perf.lastShow;
      if (gap > 1500 / fps()) perf.late++;
      if (gap > perf.maxGap) perf.maxGap = gap;
    }
    perf.lastShow = t1;
    const info = painter && painter.gl && !fullVram ? emu.display_info() : null;
    if (info && !info[4]) {
      // GPU presentation straight from 16-bit VRAM: no CPU colour conversion or copy.
      painter.frameRaw(info, new Uint8Array(wasm.memory.buffer, emu.vram_ptr(), 1024 * 512 * 2));
    } else if (emu.render(fullVram)) {
      const w = emu.frame_width(), h = emu.frame_height();
      const view = new Uint8ClampedArray(wasm.memory.buffer, emu.frame_ptr(), emu.frame_len());
      if (painter) {
        painter.frame(w, h, view);
      } else {
        const copy = view.slice();
        postMessage({ type: 'frame', w, h, data: copy.buffer }, [copy.buffer]);
      }
    }
    perf.render += performance.now() - t1;
  }
}

// Wake up on display refresh (vsync) when the worker supports requestAnimationFrame,
// with a timer fallback (hidden tabs, older browsers). setTimeout alone is clamped to
// >=4ms by browsers, which made frame delivery uneven.
const hasRaf = typeof self.requestAnimationFrame === 'function';
let rafId = 0;
function schedule(delayMs) {
  if (timer || rafId) return;
  if (hasRaf) rafId = requestAnimationFrame(() => { rafId = 0; if (timer) { clearTimeout(timer); timer = null; } loop(); });
  timer = setTimeout(() => {
    timer = null;
    if (rafId) { cancelAnimationFrame(rafId); rafId = 0; }
    loop();
  }, hasRaf ? Math.max(40, delayMs) : Math.max(0, delayMs));
}

function loop() {
  if (!running || paused) return;
  const now = performance.now();
  const s = ffActive ? ffSpeed : speed;
  if (s === 0) {
    // unlimited: run as many frames as fit in ~15ms, present the last one
    const until = now + 15;
    let k = 0;
    while (performance.now() < until) { runOne(false); k++; }
    if (k) runOne(true);
    nextFrame = performance.now();
    timer = setTimeout(() => { timer = null; loop(); }, 0);
  } else {
    let frameMs = 1000 / (fps() * s);
    // gentle audio-driven correction to avoid drift/underrun at 1x
    if (s === 1 && audioPort && audioEnabled) {
      if (audioFill > audioTarget * 0.75) frameMs *= 1.01;
      else if (audioFill < audioTarget * 0.25) frameMs *= 0.99;
    }
    // Too far behind (slow device): drop the debt and slow down smoothly
    // instead of bursting several frames and skipping on screen.
    if (now - nextFrame > frameMs * 3) nextFrame = now;
    const maxRun = Math.ceil(s) + 1;
    let ran = 0;
    while (nextFrame <= now + 1.5 && ran < maxRun) {
      ran++;
      const last = nextFrame + frameMs > now + 1.5 || ran === maxRun;
      runOne(last);
      nextFrame += frameMs;
    }
    schedule(nextFrame - performance.now());
  }
  if (now - statStart >= 1000) {
    const emuFps = statFrames * 1000 / (now - statStart);
    const n = Math.max(1, perf.frames);
    const st = emu.take_stats();
    postMessage({
      type: 'stats', fps: emuFps, speed: emuFps / fps() * 100,
      emuMs: perf.emu / n, renderMs: perf.render / Math.max(1, perf.presented), discMs: perf.disc / n,
      pixels: st[0] / n, sectors: st[2], drawMs: gpuWorker ? gpuDrawMs : painter ? painter.drawMs : null, threaded: !!gpuWorker,
      analog: [emu.analog_mode(0), emu.analog_mode(1)], late: perf.late, maxGap: perf.maxGap,
    });
    perf.emu = perf.render = perf.disc = perf.frames = perf.presented = perf.late = perf.maxGap = 0;
    statFrames = 0;
    statStart = now;
  }
}

function start() {
  running = true;
  paused = false;
  nextFrame = performance.now();
  statStart = nextFrame;
  statFrames = 0;
  if (!timer && !rafId) loop();
}

function kick() {
  if (running && !paused && !timer && !rafId) {
    nextFrame = performance.now();
    loop();
  }
}

// ---------------- settings ----------------
function applySettings(s) {
  if (!emu || !s) return;
  speed = s.speed ?? 1;
  ffSpeed = s.ffSpeed ?? 3;
  fullVram = !!s.showVram;
  if (gpuWorker) gpuWorker.postMessage({ type: 'opts', fullVram });
  frameskip = s.frameskip ?? 0;
  analogToDpad = s.analogToDpad !== false;
  audioEnabled = s.audioEnabled !== false;
  muteOnFF = s.muteOnFF !== false;
  audioTarget = Math.round(44100 * 2 * (s.latency ?? 100) / 1000);
  emu.set_dither(s.dither !== false);
  emu.set_fast_boot(s.fastBoot !== false);
  emu.set_volume((s.volume ?? 100) / 100);
  emu.set_reverb(s.reverb !== false);
  emu.set_interpolation(s.interpolation !== false);
  for (let p = 0; p < 2; p++) {
    const t = s.padType?.[p];
    if (t !== undefined) emu.set_pad_type(p, t);
  }
  for (let slot = 0; slot < 2; slot++) {
    emu.memcard_set_present(slot, (s.cardMode?.[slot] ?? 'shared') !== 'none');
  }
}

// ---------------- messages ----------------
onmessage = async (e) => {
  const m = e.data;
  if (!emu && !['init', 'audioPort', 'canvas', 'resize', 'present'].includes(m.type)) return;
  try {
    switch (m.type) {
      case 'init': {
        const wantThread = m.settings?.gpuThread !== 'off' && m.canOffscreen &&
          self.crossOriginIsolated && typeof SharedArrayBuffer !== 'undefined' && (navigator.hardwareConcurrency || 1) >= 2;
        if (wantThread) startGpuThread(); // before awaiting, so the canvas message can be forwarded
        wasm = await init({ module_or_path: new URL('../pkg/ps1core_bg.wasm', import.meta.url) });
        emu = new Emulator();
        if (gpuWorker) emu.enable_gpu_thread(gpuSync);
        applySettings(m.settings);
        postMessage({ type: 'ready', threaded: !!gpuWorker });
        break;
      }
      case 'canvas':
        if (gpuWorker) {
          gpuWorker.postMessage(m, m.canvas ? [m.canvas] : []);
          if (m.canvas) gpuWorker.postMessage({ type: 'opts', fullVram });
        } else if (m.canvas) {
          painter = new Painter(m.canvas, m.kind);
          if (!painter.failed) {
            painter.setOptions(m.opts || {});
            painter.setSize(m.w, m.h);
          }
          postMessage({ type: 'renderer', mode: painter.mode, failed: painter.failed });
          if (painter.failed) painter = null;
        } else {
          painter = null;
        }
        break;
      case 'resize':
        if (gpuWorker) gpuWorker.postMessage(m);
        else if (painter) painter.setSize(m.w, m.h);
        break;
      case 'present':
        if (gpuWorker) gpuWorker.postMessage(m);
        else if (painter) painter.setOptions(m.opts);
        break;
      case 'snapshot': {
        let snap = painter ? painter.snapshot() : null;
        if (!snap && emu && emu.render(false)) {
          // raw GPU path keeps no RGBA copy: produce one on demand
          const v = new Uint8ClampedArray(wasm.memory.buffer, emu.frame_ptr(), emu.frame_len());
          snap = { w: emu.frame_width(), h: emu.frame_height(), data: v.slice() };
        }
        postMessage({ type: 'snapshot', snap }, snap ? [snap.data.buffer] : []);
        break;
      }
      case 'audioPort':
        audioPort = m.port;
        audioPort.onmessage = (ev) => { if (typeof ev.data?.fill === 'number') audioFill = ev.data.fill; };
        break;
      case 'settings':
        applySettings(m.settings);
        kick();
        break;
      case 'bios':
        emu.load_bios(new Uint8Array(m.data));
        postMessage({ type: 'bios', ok: true });
        break;
      case 'memcardSet':
        emu.memcard_set(m.slot, new Uint8Array(m.data));
        break;
      case 'memcardGet': {
        const data = emu.memcard_get(m.slot);
        postMessage({ type: 'memcardData', slot: m.slot, id: m.id, data }, [data.buffer]);
        break;
      }
      case 'memcardFormat':
        emu.memcard_format(m.slot);
        break;
      case 'boot': {
        // m: { files: File[], tracks: Uint32Array|null, exe: ArrayBuffer|null, gameId }
        gameId = m.gameId || 'shared';
        cache.clear();
        discFiles = m.files || [];
        emu.reset();
        let discCount = 1;
        if (m.container) {
          discCount = emu.insert_disc_image(readDisc, m.container, discFiles[0].size, m.disc || 0);
        } else if (m.tracks) {
          emu.insert_disc(readDisc, new Uint32Array(m.tracks));
        } else {
          emu.eject();
        }
        if (m.exe) emu.load_exe(new Uint8Array(m.exe));
        if (m.cards) {
          for (let s = 0; s < 2; s++) {
            if (m.cards[s]) emu.memcard_set(s, new Uint8Array(m.cards[s]));
            else emu.memcard_format(s);
          }
        }
        let resumed = false;
        if (m.state) {
          try {
            emu.load_state(await gunzip(new Uint8Array(m.state.data), m.state.gz));
            resumed = true;
          } catch (err) {
            postMessage({ type: 'error', message: 'Gagal memuat progres: ' + (err?.message || err) });
          }
        }
        postMessage({ type: 'booted', region: emu.disc_region(), resumed, discCount });
        start();
        break;
      }
      case 'saveState': {
        flushCards();
        const raw = emu.save_state();
        const { data, gz } = await gzip(raw);
        postMessage({ type: 'stateData', id: m.id, data, gz }, [data.buffer]);
        break;
      }
      case 'loadState': {
        try {
          emu.load_state(await gunzip(new Uint8Array(m.data), m.gz));
          postMessage({ type: 'stateLoaded', id: m.id, ok: true });
        } catch (err) {
          postMessage({ type: 'stateLoaded', id: m.id, ok: false, message: String(err?.message || err) });
        }
        kick();
        break;
      }
      case 'swapDisc': {
        // multi-disc games: new image, the drive reports an opened lid once
        cache.clear();
        discFiles = m.files || [];
        let discCount = 1;
        if (m.container) discCount = emu.swap_disc_image(readDisc, m.container, discFiles[0].size, m.disc || 0);
        else if (m.tracks) { emu.insert_disc(readDisc, new Uint32Array(m.tracks)); emu.open_lid(); }
        postMessage({ type: 'discSwapped', region: emu.disc_region(), discCount, disc: m.disc || 0 });
        kick();
        break;
      }
      case 'reset':
        emu.reset();
        kick();
        break;
      case 'pause':
        paused = !!m.paused;
        if (!paused) kick();
        break;
      case 'stop':
        running = false;
        break;
      case 'ff':
        ffActive = !!m.on;
        break;
      case 'input':
        for (let p = 0; p < 2; p++) {
          const s = m.pads[p];
          if (!s) continue;
          let buttons = s.buttons;
          // A pad in digital mode ignores the sticks (like a real DualShock with the ANALOG
          // light off), so let the left stick drive the D-pad instead.
          if (analogToDpad && !emu.analog_mode(p)) {
            const dx = s.lx - 128, dy = s.ly - 128;
            const t = 56; // ~45% deflection
            if (dx > t) buttons |= 1 << 5;  // right
            if (dx < -t) buttons |= 1 << 7; // left
            if (dy > t) buttons |= 1 << 6;  // down
            if (dy < -t) buttons |= 1 << 4; // up
          }
          emu.set_buttons(p, buttons);
          emu.set_axes(p, s.lx, s.ly, s.rx, s.ry);
        }
        break;
      case 'analogToggle': {
        const on = !emu.analog_mode(m.port);
        emu.set_analog_mode(m.port, on);
        postMessage({ type: 'analog', port: m.port, on });
        break;
      }
      case 'step':
        if (paused) runOne(true);
        break;
    }
  } catch (err) {
    postMessage({ type: 'error', message: String(err?.message || err) });
  }
};
