import { Presenter } from './presenter.js';
import { prepareGame } from './disc.js';
import { kv } from './storage.js';
import { listSaves, freeBlocks, deleteSave, importCard } from './memcard.js';
import {
  Input, BUTTONS, BUTTON_LABELS, AXES, AXIS_LABELS, DEFAULT_KEYS, DEFAULT_PAD,
  DEFAULT_HOTKEYS, HOTKEY_LABELS, keyLabel,
} from './input.js';

const $ = (s) => document.querySelector(s);

// ---------------- settings ----------------
const DEFAULTS = {
  speed: 1, ffSpeed: 3, pauseOnBlur: true, showFps: false,
  renderer: 'software', aspect: '4:3', integer: false, filter: 'nearest', crt: 60, frameskip: 0, dither: true, showVram: false,
  audioEnabled: true, volume: 100, latency: 100, reverb: true, interpolation: true, muteOnFF: true,
  padType: [2, 1], gamepadIndex: [0, -1], deadzone: 15, rumble: true,
  keys: structuredClone(DEFAULT_KEYS), padMap: [{ ...DEFAULT_PAD }, { ...DEFAULT_PAD }],
  hotkeys: { ...DEFAULT_HOTKEYS },
  cardMode: ['shared', 'shared'],
};

function loadSettings() {
  try {
    const s = JSON.parse(localStorage.getItem('ps1-settings') || '{}');
    return { ...structuredClone(DEFAULTS), ...s, hotkeys: { ...DEFAULT_HOTKEYS, ...(s.hotkeys || {}) } };
  } catch {
    return structuredClone(DEFAULTS);
  }
}
let settings = loadSettings();
function saveSettings() {
  try { localStorage.setItem('ps1-settings', JSON.stringify(settings)); } catch { /* storage unavailable */ }
}

// ---------------- UI helpers ----------------
function toast(msg, err = false, ms = 3500) {
  const d = document.createElement('div');
  d.textContent = msg;
  if (err) d.className = 'err';
  $('#toast').appendChild(d);
  setTimeout(() => d.remove(), ms);
}

// ---------------- core worker ----------------
const worker = new Worker(new URL('./worker.js', import.meta.url), { type: 'module' });
const presenter = new Presenter($('#screenHost'));
let ready = false;
let running = false;
let paused = false;
let userPaused = false;
let gameId = 'shared';
let gameTitle = '';
let biosName = null;
let lastStats = null;
const pendingCardReq = new Map();

function post(msg, transfer) { worker.postMessage(msg, transfer || []); }
function pushSettings() { post({ type: 'settings', settings }); }

worker.onmessage = async (e) => {
  const m = e.data;
  switch (m.type) {
    case 'ready':
      ready = true;
      await restoreBios();
      for (let s = 0; s < 2; s++) await loadCardIntoCore(s, 'shared');
      break;
    case 'frame':
      presenter.frame(m.w, m.h, m.data);
      if (settings.showFps && lastStats) {
        const s = lastStats;
        $('#overlay').textContent =
          `${s.fps.toFixed(1)} FPS · ${Math.round(s.speed)}% · ${m.w}×${m.h} · ${presenter.mode === 'gpu' ? 'GPU' : 'CPU'}\n` +
          `emulasi ${s.emuMs.toFixed(1)} ms · render ${s.renderMs.toFixed(1)} ms · disc ${s.discMs.toFixed(1)} ms · layar ${presenter.drawMs.toFixed(1)} ms`;
      }
      break;
    case 'stats':
      lastStats = m;
      break;
    case 'booted':
      toast(`Berjalan: ${gameTitle}${m.region !== '-' ? ' · ' + m.region : ''}`);
      break;
    case 'memcard':
      await kv.set(cardKey(m.slot, m.gameId), new Uint8Array(m.data));
      break;
    case 'memcardData': {
      const cb = pendingCardReq.get(m.id);
      pendingCardReq.delete(m.id);
      cb && cb(new Uint8Array(m.data));
      break;
    }
    case 'rumble':
      input.rumble(m.port, m.small, m.large);
      break;
    case 'analog':
      toast(`Port ${m.port + 1}: mode ${m.on ? 'ANALOG' : 'DIGITAL'}`);
      break;
    case 'error':
      toast('Error: ' + m.message, true, 6000);
      break;
  }
};
worker.onerror = (e) => toast('Worker gagal dimuat: ' + (e.message || 'pastikan diakses lewat server http'), true, 8000);
post({ type: 'init', settings });

// ---------------- audio ----------------
let audioCtx = null;
async function ensureAudio() {
  if (audioCtx) {
    if (audioCtx.state === 'suspended') audioCtx.resume();
    return;
  }
  try {
    audioCtx = new AudioContext({ sampleRate: 44100, latencyHint: 'interactive' });
    await audioCtx.audioWorklet.addModule(new URL('./audio-worklet.js', import.meta.url));
    const node = new AudioWorkletNode(audioCtx, 'psx-audio', { outputChannelCount: [2] });
    node.connect(audioCtx.destination);
    const ch = new MessageChannel();
    node.port.postMessage({ port: ch.port1 }, [ch.port1]);
    node.port.postMessage({ latency: settings.latency });
    post({ type: 'audioPort', port: ch.port2 }, [ch.port2]);
    audioCtx._node = node;
  } catch (err) {
    console.warn(err);
    toast('Audio tidak tersedia di browser ini.', true);
  }
}
['pointerdown', 'keydown'].forEach((ev) => window.addEventListener(ev, () => ensureAudio(), { once: false, passive: true }));

// ---------------- BIOS ----------------
async function restoreBios() {
  const b = await kv.get('bios');
  if (b && b.data) {
    post({ type: 'bios', data: b.data.buffer.slice(0) });
    biosName = b.name;
  }
  refreshBiosUi();
}

function refreshBiosUi() {
  const t = biosName ? `${biosName} ✓` : 'belum ada';
  $('#biosStatus').textContent = t;
  $('#biosStatus').className = biosName ? 'ok' : 'warn';
  $('#biosInfo').textContent = biosName || 'Belum ada BIOS';
}

async function setBios(file) {
  const buf = new Uint8Array(await file.arrayBuffer());
  if (buf.length !== 524288) {
    toast(`BIOS tidak valid: ukuran ${buf.length} byte (harus 524288).`, true, 6000);
    return;
  }
  await kv.set('bios', { name: file.name, data: buf });
  biosName = file.name;
  post({ type: 'bios', data: buf.buffer.slice(0) });
  refreshBiosUi();
  toast('BIOS tersimpan di browser ini.');
}

$('#btnBios').onclick = () => $('#fileBios').click();
$('#btnBios2').onclick = () => $('#fileBios').click();
$('#btnBiosDel').onclick = async () => {
  await kv.del('bios');
  biosName = null;
  refreshBiosUi();
  toast('BIOS dihapus dari browser.');
};
$('#fileBios').onchange = (e) => { const f = e.target.files[0]; if (f) setBios(f); e.target.value = ''; };

// ---------------- memory cards ----------------
function cardKey(slot, id) {
  const mode = settings.cardMode[slot];
  return mode === 'pergame' && id && id !== 'shared' ? `card:${slot}:${id}` : `card:${slot}:shared`;
}

async function loadCardIntoCore(slot, id) {
  const data = await kv.get(cardKey(slot, id));
  if (data) post({ type: 'memcardSet', slot, data: data.buffer.slice(0) });
  else post({ type: 'memcardFormat', slot });
}

function fetchCard(slot) {
  return new Promise((res) => {
    const id = Math.random().toString(36).slice(2);
    pendingCardReq.set(id, res);
    post({ type: 'memcardGet', slot, id });
  });
}

async function writeCard(slot, data) {
  post({ type: 'memcardSet', slot, data: data.buffer.slice(0) });
  await kv.set(cardKey(slot, gameId), data);
}

function download(name, data, type = 'application/octet-stream') {
  const url = URL.createObjectURL(data instanceof Blob ? data : new Blob([data], { type }));
  const a = document.createElement('a');
  a.href = url;
  a.download = name;
  a.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

let importSlot = 0;
$('#fileCard').onchange = async (e) => {
  const f = e.target.files[0];
  e.target.value = '';
  if (!f) return;
  try {
    const data = importCard(await f.arrayBuffer());
    await writeCard(importSlot, data);
    toast(`Memory card slot ${importSlot + 1} diimpor dari ${f.name}`);
    renderCards();
  } catch (err) {
    toast(err.message, true);
  }
};

async function renderCards() {
  const host = $('#cards');
  host.innerHTML = '';
  for (let slot = 0; slot < 2; slot++) {
    const data = await fetchCard(slot);
    const saves = listSaves(data);
    const el = document.createElement('div');
    el.className = 'card';
    const scope = settings.cardMode[slot] === 'pergame' && gameId !== 'shared' ? `per-game: ${gameId}` : 'bersama';
    el.innerHTML = `
      <h3>Slot ${slot + 1}</h3>
      <div class="row"><label>Mode</label>
        <select data-slot="${slot}" class="cardMode">
          <option value="shared">Satu kartu untuk semua game</option>
          <option value="pergame">Kartu terpisah per game</option>
          <option value="none">Tidak terpasang</option>
        </select></div>
      <p class="hint">Kartu aktif: ${scope} · ${freeBlocks(data)} blok kosong dari 15</p>
      <ul>${saves.length ? '' : '<li><span class="muted">Kartu kosong</span></li>'}</ul>
      <div class="btns">
        <button type="button" data-act="import">Impor…</button>
        <button type="button" data-act="export">Ekspor .mcr</button>
        <button type="button" data-act="format" class="danger">Format</button>
      </div>`;
    const ul = el.querySelector('ul');
    for (const s of saves) {
      const li = document.createElement('li');
      li.innerHTML = `<span title="${s.name}"></span><small>${s.blocks} blok</small><button type="button" class="danger">Hapus</button>`;
      li.querySelector('span').textContent = s.title || s.name;
      li.querySelector('button').onclick = async () => {
        if (!confirm(`Hapus save "${s.title || s.name}"?`)) return;
        deleteSave(data, s.block);
        await writeCard(slot, data);
        renderCards();
      };
      ul.appendChild(li);
    }
    const sel = el.querySelector('select');
    sel.value = settings.cardMode[slot];
    sel.onchange = async () => {
      settings.cardMode[slot] = sel.value;
      saveSettings();
      pushSettings();
      await loadCardIntoCore(slot, gameId);
      renderCards();
    };
    el.querySelector('[data-act=import]').onclick = () => { importSlot = slot; $('#fileCard').click(); };
    el.querySelector('[data-act=export]').onclick = () => download(`${(settings.cardMode[slot] === 'pergame' ? gameId : 'shared')}_slot${slot + 1}.mcr`, data);
    el.querySelector('[data-act=format]').onclick = async () => {
      if (!confirm(`Format memory card slot ${slot + 1}? Semua save akan hilang.`)) return;
      post({ type: 'memcardFormat', slot });
      const fresh = await fetchCard(slot);
      await kv.set(cardKey(slot, gameId), fresh);
      renderCards();
    };
    host.appendChild(el);
  }
}

// ---------------- game loading ----------------
async function openGame(files) {
  if (!ready) { toast('Emulator masih memuat…'); return; }
  try {
    const g = await prepareGame(files);
    if (!biosName && !g.exe) {
      toast('Pilih BIOS dulu sebelum menjalankan game (Pengaturan → Sistem).', true, 6000);
      return;
    }
    await ensureAudio();
    gameId = g.id;
    gameTitle = g.title;
    document.title = `${g.title} — PS1 Web Emulator`;
    const cards = [];
    for (let s = 0; s < 2; s++) {
      const d = settings.cardMode[s] === 'none' ? null : await kv.get(cardKey(s, gameId));
      cards.push(d ? d.buffer.slice(0) : null);
    }
    const exe = g.exe ? await g.exe.arrayBuffer() : null;
    post({ type: 'boot', files: g.files, tracks: g.tracks, exe, gameId, cards });
    running = true;
    setPaused(false);
    $('#welcome').classList.add('hidden');
    for (const id of ['#btnPause', '#btnReset', '#btnFF', '#btnShot']) $(id).disabled = false;
  } catch (err) {
    toast(err.message || String(err), true, 6000);
  }
}

$('#btnOpen').onclick = () => $('#fileGame').click();
$('#fileGame').onchange = (e) => { const f = Array.from(e.target.files); e.target.value = ''; if (f.length) openGame(f); };

const stage = $('#stage');
stage.addEventListener('dragover', (e) => { e.preventDefault(); stage.classList.add('dragging'); });
stage.addEventListener('dragleave', () => stage.classList.remove('dragging'));
stage.addEventListener('drop', (e) => {
  e.preventDefault();
  stage.classList.remove('dragging');
  const files = Array.from(e.dataTransfer.files);
  const bios = files.length === 1 && files[0].size === 524288 && /\.(bin|rom)$/i.test(files[0].name);
  if (bios) setBios(files[0]); else if (files.length) openGame(files);
});

// ---------------- controls ----------------
function setPaused(p) {
  paused = p;
  post({ type: 'pause', paused: p });
  $('#btnPause').textContent = p ? 'Lanjut' : 'Pause';
}
$('#btnPause').onclick = () => { userPaused = !paused; setPaused(!paused); };
$('#btnReset').onclick = () => { post({ type: 'reset' }); toast('Reset'); };
let ffLatched = false;
$('#btnFF').onclick = () => { ffLatched = !ffLatched; post({ type: 'ff', on: ffLatched }); $('#btnFF').classList.toggle('primary', ffLatched); };
$('#btnShot').onclick = screenshot;
$('#btnFull').onclick = toggleFullscreen;
$('#btnSettings').onclick = openSettings;

async function screenshot() {
  const blob = await presenter.screenshot();
  if (blob) download(`${gameTitle || 'ps1'}_${Date.now()}.png`, blob);
}

function toggleFullscreen() {
  if (document.fullscreenElement) document.exitFullscreen();
  else document.documentElement.requestFullscreen?.().catch(() => {});
}
document.addEventListener('fullscreenchange', () => {
  document.body.classList.toggle('fullscreen', !!document.fullscreenElement);
});

document.addEventListener('visibilitychange', () => {
  if (!running || !settings.pauseOnBlur || userPaused) return;
  setPaused(document.hidden);
});

// ---------------- input ----------------
const input = new Input(
  () => settings,
  (pads) => post({ type: 'input', pads }),
  (action, down) => {
    if (action === 'fastForward') { if (running) post({ type: 'ff', on: down || ffLatched }); return; }
    if (!down) return;
    switch (action) {
      case 'pause': if (running) { userPaused = !paused; setPaused(!paused); } break;
      case 'reset': if (running) post({ type: 'reset' }); break;
      case 'fullscreen': toggleFullscreen(); break;
      case 'screenshot': if (running) screenshot(); break;
      case 'analog': post({ type: 'analogToggle', port: 0 }); break;
      case 'analogPort0': post({ type: 'analogToggle', port: 0 }); break;
      case 'analogPort1': post({ type: 'analogToggle', port: 1 }); break;
      case 'menu': { const d = $('#settings'); if (d.open) d.close(); else openSettings(); break; }
    }
  },
);

// ---------------- settings dialog ----------------
const dlg = $('#settings');
let pausedForMenu = false;

function openSettings() {
  syncForm();
  renderBinds();
  renderHotkeys();
  if (running && !paused) { setPaused(true); pausedForMenu = true; }
  dlg.showModal();
  if ($('.tabs .active').dataset.tab === 'memcard') renderCards();
}
dlg.addEventListener('close', () => {
  input.capture = null;
  if (pausedForMenu) { setPaused(false); pausedForMenu = false; }
});

document.querySelectorAll('.tabs button').forEach((b) => {
  b.onclick = () => {
    document.querySelectorAll('.tabs button').forEach((x) => x.classList.toggle('active', x === b));
    document.querySelectorAll('.panel').forEach((p) => p.classList.toggle('active', p.dataset.panel === b.dataset.tab));
    if (b.dataset.tab === 'memcard') renderCards();
    if (b.dataset.tab === 'controller') fillGamepads();
  };
});

function syncForm() {
  document.querySelectorAll('[data-key]').forEach((el) => {
    const v = settings[el.dataset.key];
    if (el.type === 'checkbox') el.checked = !!v;
    else el.value = String(v);
  });
  $('#volOut').textContent = settings.volume + '%';
  syncPort();
  updateRendererHint();
}

document.querySelectorAll('[data-key]').forEach((el) => {
  el.addEventListener('input', () => {
    const k = el.dataset.key;
    settings[k] = el.type === 'checkbox' ? el.checked : (el.dataset.num !== undefined ? Number(el.value) : el.value);
    saveSettings();
    applySettings();
  });
});

function updateRendererHint() {
  const h = $('#rendererHint');
  if (settings.renderer === 'gpu' && presenter.mode !== 'gpu') {
    h.textContent = 'WebGL tidak tersedia di perangkat ini, jadi otomatis memakai renderer Software (CPU).';
  } else if (presenter.mode === 'gpu') {
    h.textContent = 'GPU dipakai untuk menampilkan, menskalakan, dan memfilter gambar (sharp bilinear, CRT). Emulasi tetap di CPU.';
  } else {
    h.textContent = 'Seluruh proses berjalan di CPU (Canvas 2D). Cocok untuk perangkat tanpa GPU atau driver bermasalah.';
  }
}

function applySettings() {
  const used = presenter.setRenderer(settings.renderer);
  presenter.setOptions({
    aspect: settings.aspect, integer: settings.integer, filter: settings.filter,
    crt: settings.crt / 100, showVram: settings.showVram,
  });
  $('#overlay').classList.toggle('hidden', !settings.showFps);
  $('#volOut').textContent = settings.volume + '%';
  if (audioCtx && audioCtx._node) audioCtx._node.port.postMessage({ latency: settings.latency });
  if (used !== settings.renderer) updateRendererHint(); else updateRendererHint();
  pushSettings();
}

// controller tab
function curPort() { return Number($('#port').value); }
function syncPort() {
  const p = curPort();
  $('#padType').value = String(settings.padType[p]);
  fillGamepads();
  renderBinds();
}
$('#port').onchange = syncPort;
$('#padType').onchange = () => {
  settings.padType[curPort()] = Number($('#padType').value);
  saveSettings();
  pushSettings();
};
function fillGamepads() {
  const sel = $('#gamepadIndex');
  const pads = navigator.getGamepads ? Array.from(navigator.getGamepads()) : [];
  sel.innerHTML = '<option value="-1">Tidak ada (keyboard saja)</option>';
  for (let i = 0; i < 4; i++) {
    const gp = pads[i];
    const o = document.createElement('option');
    o.value = String(i);
    o.textContent = gp ? `#${i + 1}: ${gp.id.slice(0, 48)}` : `#${i + 1} (belum terdeteksi — tekan tombol di gamepad)`;
    sel.appendChild(o);
  }
  sel.value = String(settings.gamepadIndex[curPort()]);
}
$('#gamepadIndex').onchange = () => {
  settings.gamepadIndex[curPort()] = Number($('#gamepadIndex').value);
  saveSettings();
};
window.addEventListener('gamepadconnected', (e) => {
  toast(`Gamepad terhubung: ${e.gamepad.id.slice(0, 40)}`);
  if (dlg.open) fillGamepads();
});

function bindButton(label, getVal, onSet) {
  const b = document.createElement('button');
  b.type = 'button';
  b.textContent = keyLabel(getVal());
  b.onclick = () => {
    document.querySelectorAll('.listening').forEach((x) => x.classList.remove('listening'));
    b.classList.add('listening');
    b.textContent = 'Tekan tombol…';
    input.captureNext((kind, code) => {
      b.classList.remove('listening');
      onSet(kind, code);
      saveSettings();
      b.textContent = keyLabel(getVal());
    });
  };
  return b;
}

function renderBinds() {
  const p = curPort();
  const tb = $('#bindTable tbody');
  tb.innerHTML = '';
  settings.keys[p] = settings.keys[p] || {};
  settings.padMap[p] = settings.padMap[p] || { ...DEFAULT_PAD };
  const rows = [...['up', 'down', 'left', 'right', 'cross', 'circle', 'square', 'triangle', 'l1', 'r1', 'l2', 'r2', 'l3', 'r3', 'start', 'select'], ...AXES];
  for (const id of rows) {
    const tr = document.createElement('tr');
    const isAxis = AXES.includes(id);
    const td0 = document.createElement('td');
    td0.textContent = isAxis ? AXIS_LABELS[id] : BUTTON_LABELS[id];
    const td1 = document.createElement('td');
    td1.appendChild(bindButton(id, () => settings.keys[p][id], (kind, code) => {
      if (kind === 'key') {
        if (code) for (const k of Object.keys(settings.keys[p])) if (settings.keys[p][k] === code) delete settings.keys[p][k];
        if (code) settings.keys[p][id] = code; else delete settings.keys[p][id];
      }
    }));
    const td2 = document.createElement('td');
    if (!isAxis) {
      td2.appendChild(bindButton(id, () => settings.padMap[p][id], (kind, code) => {
        if (kind === 'pad') settings.padMap[p][id] = code;
        else if (code === null) delete settings.padMap[p][id];
      }));
    } else {
      td2.innerHTML = '<span class="muted">stik analog</span>';
    }
    tr.append(td0, td1, td2);
    tb.appendChild(tr);
  }
}
$('#bindReset').onclick = () => {
  const p = curPort();
  settings.keys[p] = structuredClone(DEFAULT_KEYS[p] || {});
  settings.padMap[p] = { ...DEFAULT_PAD };
  saveSettings();
  renderBinds();
};

function renderHotkeys() {
  const tb = $('#hotkeyTable tbody');
  tb.innerHTML = '';
  for (const id of Object.keys(DEFAULT_HOTKEYS)) {
    const tr = document.createElement('tr');
    const td0 = document.createElement('td');
    td0.textContent = HOTKEY_LABELS[id];
    const td1 = document.createElement('td');
    td1.appendChild(bindButton(id, () => settings.hotkeys[id], (kind, code) => {
      if (kind === 'key') settings.hotkeys[id] = code || '';
    }));
    tr.append(td0, td1);
    tb.appendChild(tr);
  }
}

applySettings();
refreshBiosUi();
