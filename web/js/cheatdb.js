// Online cheat database: libretro-database GameShark files, matched by disc serial
// (index shipped in data/ps1-cheats.json) or by title. Downloaded files are cached.
import { kv } from './storage.js';

const RAW = 'https://raw.githubusercontent.com/libretro/libretro-database/master/cht/Sony%20-%20PlayStation/';
let index = null;

export async function loadIndex() {
  if (!index) {
    const r = await fetch(new URL('../data/ps1-cheats.json', import.meta.url));
    index = await r.json();
  }
  return index;
}

function splitName(name) {
  const base = name.replace(/\s*\([^)]*\)/g, '').trim();
  const tags = [...name.matchAll(/\(([^)]*)\)/g)].flatMap((m) => m[1].split(',').map((t) => t.trim().toLowerCase()));
  return { base, tags: tags.map((t) => ({ pal: 'europe', eur: 'europe' }[t] || t)) };
}

export function normTitle(t) {
  return t.toLowerCase().replace(/&/g, 'and').replace(/[^a-z0-9]+/g, ' ').replace(/\s+/g, ' ').trim();
}

/** Find the best cheat file for a game: by serial first, then by title. */
export async function findCheatFile(serial, title) {
  const idx = await loadIndex();
  if (serial && idx.serials[serial] !== undefined) return idx.files[idx.serials[serial]];
  if (!title) return null;
  const want = splitName(title.replace(/\s*\((Disc|Disk) \d+\)/i, ''));
  const key = normTitle(want.base);
  const cands = idx.files.filter((f) => normTitle(splitName(f.replace(/\.cht$/, '')).base) === key);
  if (!cands.length) return null;
  const score = (f) => splitName(f).tags.filter((t) => want.tags.includes(t)).length;
  return cands.sort((a, b) => score(b) - score(a))[0];
}

/** Titles matching a free-text query (for manual selection). */
export async function searchCheatFiles(query, limit = 30) {
  const idx = await loadIndex();
  const words = normTitle(query).split(' ').filter(Boolean);
  if (!words.length) return [];
  return idx.files.filter((f) => { const n = normTitle(f); return words.every((w) => n.includes(w)); }).slice(0, limit);
}

/** Parse a RetroArch .cht file into [{name, code}] with one "AAAAAAAA VVVV" pair per line. */
export function parseCht(text) {
  const desc = {}, code = {};
  for (const line of text.split(/\r?\n/)) {
    const m = line.match(/^\s*cheat(\d+)_(desc|code)\s*=\s*"(.*)"\s*$/);
    if (!m) continue;
    (m[2] === 'desc' ? desc : code)[m[1]] = m[3];
  }
  const out = [];
  for (const k of Object.keys(code).sort((a, b) => a - b)) {
    const parts = code[k].split(/[+\s]+/).filter(Boolean);
    if (parts.length < 2 || parts.length % 2) continue;
    const lines = [];
    let ok = true;
    for (let i = 0; i < parts.length; i += 2) {
      if (!/^[0-9A-Fa-f]{8}$/.test(parts[i]) || !/^[0-9A-Fa-f]{4}$/.test(parts[i + 1])) { ok = false; break; }
      lines.push(`${parts[i].toUpperCase()} ${parts[i + 1].toUpperCase()}`);
    }
    if (ok) out.push({ name: (desc[k] || `Cheat ${+k + 1}`).trim(), code: lines.join('\n') });
  }
  return out;
}

/** Download (or read from cache) and parse a cheat file. */
export async function fetchCheatFile(file, { refresh = false } = {}) {
  const key = `chtcache:${file}`;
  if (!refresh) {
    const cached = await kv.get(key);
    if (cached) return cached;
  }
  const r = await fetch(RAW + encodeURIComponent(file), { mode: 'cors' });
  if (!r.ok) throw new Error(`HTTP ${r.status}`);
  const list = parseCht(await r.text());
  await kv.set(key, list);
  return list;
}
