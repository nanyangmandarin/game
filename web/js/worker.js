// Emulator worker: owns the WASM core, paces frames, streams video/audio to the UI.
import init, { Emulator } from '../pkg/ps1core.js';

let wasm = null;
let emu = null;
let running = false;
let paused = false;
let speed = 1;           // 0 = unlimited
let ffActive = false;
let ffSpeed = 3;
let fullVram = false;
let frameskip = 0;
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
const perf = { emu: 0, render: 0, disc: 0, frames: 0, presented: 0 };

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
  if (present) {
    const t1 = performance.now();
    perf.presented++;
    if (emu.render(fullVram)) {
      const w = emu.frame_width(), h = emu.frame_height();
      const view = new Uint8Array(wasm.memory.buffer, emu.frame_ptr(), emu.frame_len());
      const copy = view.slice();
      postMessage({ type: 'frame', w, h, data: copy.buffer }, [copy.buffer]);
    }
    perf.render += performance.now() - t1;
  }
}

function loop() {
  timer = null;
  if (!running || paused) return;
  const now = performance.now();
  const s = ffActive ? ffSpeed : speed;
  if (s === 0) {
    // unlimited: run as many frames as fit in ~16ms
    const until = now + 15;
    let k = 0;
    while (performance.now() < until) {
      runOne(false);
      k++;
    }
    if (k) runOne(true);
    nextFrame = performance.now();
    timer = setTimeout(loop, 0);
  } else {
    let frameMs = 1000 / (fps() * s);
    // gentle audio-driven correction to avoid drift/underrun at 1x
    if (s === 1 && audioPort && audioEnabled) {
      if (audioFill > audioTarget * 0.75) frameMs *= 1.01;
      else if (audioFill < audioTarget * 0.25) frameMs *= 0.99;
    }
    if (now - nextFrame > 250) nextFrame = now;
    let ran = 0;
    while (nextFrame <= now && ran < 8) {
      const last = nextFrame + frameMs > now || ran === 7;
      runOne(last);
      nextFrame += frameMs;
      ran++;
    }
    timer = setTimeout(loop, Math.max(0, nextFrame - performance.now()));
  }
  if (now - statStart >= 1000) {
    const emuFps = statFrames * 1000 / (now - statStart);
    const n = Math.max(1, perf.frames);
    const st = emu.take_stats();
    postMessage({
      type: 'stats', fps: emuFps, speed: emuFps / fps() * 100,
      emuMs: perf.emu / n, renderMs: perf.render / Math.max(1, perf.presented), discMs: perf.disc / n,
      pixels: st[0] / n, sectors: st[2],
    });
    perf.emu = perf.render = perf.disc = perf.frames = perf.presented = 0;
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
  if (!timer) loop();
}

function kick() {
  if (running && !paused && !timer) {
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
  frameskip = s.frameskip ?? 0;
  audioEnabled = s.audioEnabled !== false;
  muteOnFF = s.muteOnFF !== false;
  audioTarget = Math.round(44100 * 2 * (s.latency ?? 100) / 1000);
  emu.set_dither(s.dither !== false);
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
  if (!emu && m.type !== 'init' && m.type !== 'audioPort') return;
  try {
    switch (m.type) {
      case 'init': {
        wasm = await init({ module_or_path: new URL('../pkg/ps1core_bg.wasm', import.meta.url) });
        emu = new Emulator();
        applySettings(m.settings);
        postMessage({ type: 'ready' });
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
        if (m.tracks) {
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
        postMessage({ type: 'booted', region: emu.disc_region(), resumed });
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
          emu.set_buttons(p, s.buttons);
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
