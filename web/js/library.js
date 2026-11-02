// Game library persisted in OPFS (files) + IndexedDB (metadata), so games survive a refresh.
import { kv } from './storage.js';

export const opfsSupported = !!(navigator.storage && navigator.storage.getDirectory);

export function dirName(id) {
  return id.replace(/[\\/:*?"<>|\x00-\x1f]/g, '_').slice(0, 120) || 'game';
}

export async function listGames() {
  const list = (await kv.get('games')) || [];
  return list.sort((a, b) => (b.lastPlayed || 0) - (a.lastPlayed || 0));
}

export async function getGame(id) {
  return (await listGames()).find((g) => g.id === id) || null;
}

export async function putGame(meta) {
  const list = (await kv.get('games')) || [];
  const i = list.findIndex((g) => g.id === meta.id);
  if (i >= 0) list[i] = { ...list[i], ...meta }; else list.push(meta);
  await kv.set('games', list);
}

export async function touchGame(id) {
  const g = await getGame(id);
  if (g) await putGame({ ...g, lastPlayed: Date.now() });
  await kv.set('lastGame', id);
}

async function gamesDir() {
  const root = await navigator.storage.getDirectory();
  return root.getDirectoryHandle('games', { create: true });
}

/** Returns File objects for a stored game, in the original order. */
export async function gameFiles(meta) {
  const d = await (await gamesDir()).getDirectoryHandle(meta.dir);
  const out = [];
  for (const name of meta.files) {
    const fh = await d.getFileHandle(name);
    out.push(await fh.getFile());
  }
  return out;
}

export async function removeGame(id) {
  const g = await getGame(id);
  if (g && opfsSupported) {
    try { await (await gamesDir()).removeEntry(g.dir, { recursive: true }); } catch { /* already gone */ }
  }
  const list = ((await kv.get('games')) || []).filter((x) => x.id !== id);
  await kv.set('games', list);
  for (const k of await kv.keys()) {
    if (typeof k === 'string' && k.startsWith(`state:${id}:`)) await kv.del(k);
  }
}

export async function storageInfo() {
  if (!navigator.storage?.estimate) return null;
  const e = await navigator.storage.estimate();
  return { usage: e.usage || 0, quota: e.quota || 0 };
}

/**
 * Copy files into OPFS in a background worker.
 * @returns {Promise<void>}
 */
export async function importGame(meta, files, onProgress) {
  if (!opfsSupported) throw new Error('Browser ini tidak mendukung penyimpanan file (OPFS).');
  const size = files.reduce((a, f) => a + f.size, 0);
  const info = await storageInfo();
  if (info && info.quota - info.usage < size * 1.05) {
    throw new Error(`Ruang penyimpanan browser tidak cukup (butuh ${fmtSize(size)}, tersisa ${fmtSize(info.quota - info.usage)}).`);
  }
  try { await navigator.storage.persist?.(); } catch { /* optional */ }
  await new Promise((resolve, reject) => {
    const w = new Worker(new URL('./import-worker.js', import.meta.url));
    w.onmessage = (e) => {
      const m = e.data;
      if (m.type === 'progress') onProgress && onProgress(m.done / m.total);
      else if (m.type === 'done') { w.terminate(); resolve(); }
      else if (m.type === 'error') { w.terminate(); reject(new Error(m.message)); }
    };
    w.onerror = (e) => { w.terminate(); reject(new Error(e.message || 'Gagal menyalin game')); };
    w.postMessage({ dir: meta.dir, files });
  });
  await putGame({ ...meta, size, added: Date.now() });
}

export function fmtSize(n) {
  if (n >= 1 << 30) return (n / (1 << 30)).toFixed(2) + ' GB';
  if (n >= 1 << 20) return (n / (1 << 20)).toFixed(0) + ' MB';
  return Math.max(1, Math.round(n / 1024)) + ' KB';
}
