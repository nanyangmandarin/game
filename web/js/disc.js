// Builds the track table for the core from ISO / BIN / CUE files.
// Track row: [number, file, start_frame, length, sector_size, is_audio, offset_lo, offset_hi]

const SYNC = [0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x00];

async function detectSectorSize(file) {
  const head = new Uint8Array(await file.slice(0, 16).arrayBuffer());
  const hasSync = SYNC.every((b, i) => head[i] === b);
  if (hasSync && file.size % 2352 === 0) return 2352;
  if (file.size % 2048 === 0) return 2048;
  if (file.size % 2352 === 0) return 2352;
  return hasSync ? 2352 : 2048;
}

function row(t) {
  const off = t.offset;
  return [t.number, t.file, t.start, t.length, t.size, t.audio ? 1 : 0, off % 0x100000000, Math.floor(off / 0x100000000)];
}

function msfToFrames(s) {
  const [m, sec, f] = s.split(':').map((x) => parseInt(x, 10));
  return (m * 60 + sec) * 75 + f;
}

function baseName(p) {
  return p.replace(/\\/g, '/').split('/').pop().toLowerCase();
}

/**
 * @param {File[]} picked files chosen by the user
 * @returns {{files: File[], tracks: number[]|null, exe: File|null, title: string, id: string}}
 */
export async function prepareGame(picked) {
  const byName = new Map(picked.map((f) => [f.name.toLowerCase(), f]));
  // a CUE referencing "game.bin" also matches "game.bin.ecm"
  for (const f of picked) if (/\.ecm$/i.test(f.name)) byName.set(f.name.toLowerCase().replace(/\.ecm$/, ''), f);
  const cue = picked.find((f) => /\.cue$/i.test(f.name));
  const exe = picked.find((f) => /\.(exe|psexe|psx)$/i.test(f.name));
  const unsupported = picked.find((f) => /\.(7z|zip|rar|ape|mds)$/i.test(f.name));
  const packed = picked.find((f) => /\.(chd|pbp)$/i.test(f.name));
  if (packed) {
    const container = packed.name.split('.').pop().toLowerCase();
    return { files: [packed], tracks: null, container, exe: null, title: packed.name.replace(/\.[^.]+$/, ''), id: idFrom(packed.name) };
  }

  if (cue) {
    const text = await cue.text();
    const title = cue.name.replace(/\.cue$/i, '');
    const files = cueFiles(text, byName);
    if (files.some((f) => /\.ecm$/i.test(f.name))) {
      // decoded sizes are only known after the emulator scans the ECM files
      return { files, tracks: null, container: 'cue-ecm', cue: text, exe: null, title, id: idFrom(cue.name) };
    }
    return { files, tracks: cueTracks(text, files.map((f) => f.size)), exe: null, title, id: idFrom(cue.name) };
  }
  const ecm = picked.find((f) => /\.ecm$/i.test(f.name));
  if (ecm) {
    const title = ecm.name.replace(/\.ecm$/i, '').replace(/\.[^.]+$/, '');
    return { files: [ecm], tracks: null, container: 'ecm', exe: null, title, id: idFrom(title) };
  }
  if (exe && !picked.some((f) => /\.(iso|bin|img)$/i.test(f.name))) {
    return { files: [], tracks: null, exe, title: exe.name, id: idFrom(exe.name) };
  }
  const img = picked.find((f) => /\.(iso|bin|img|mdf)$/i.test(f.name));
  if (!img) {
    if (unsupported) throw new Error(`Format ${unsupported.name.split('.').pop().toUpperCase()} belum didukung. Gunakan ISO, BIN/CUE, CHD, PBP, ECM, atau EXE.`);
    throw new Error('Pilih file ISO, BIN (+CUE), CHD, PBP, atau EXE.');
  }
  const size = await detectSectorSize(img);
  const frames = Math.floor(img.size / size);
  const tracks = row({ number: 1, file: 0, start: 150, length: frames, size, audio: false, offset: 0 });
  return { files: [img], tracks, exe: null, title: img.name.replace(/\.[^.]+$/, ''), id: idFrom(img.name) };
}

function idFrom(name) {
  return name.replace(/\.[^.]+$/, '').replace(/\s*\((disc|disk|cd)\s*\d+\)/i, '').trim();
}

/** Files referenced by a CUE sheet, in order. */
export function cueFiles(text, byName) {
  const files = [];
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.trim();
    if (!/^FILE\s/i.test(line)) continue;
    const m = line.match(/^FILE\s+"?(.+?)"?\s+(\w+)$/i);
    const name = m ? m[1] : line.slice(5);
    const f = byName.get(baseName(name));
    if (!f) throw new Error(`File "${baseName(name)}" yang dirujuk CUE tidak ikut dipilih. Pilih .cue bersama semua file .bin-nya.`);
    files.push(f);
  }
  return files;
}

/** Track table for a CUE sheet given each referenced file's (decoded) size in bytes. */
export function cueTracks(text, fileSizes) {
  const entries = []; // per track
  let cur = null;
  let fileIdx = -1;
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.trim();
    if (!line) continue;
    const kw = line.split(/\s+/)[0].toUpperCase();
    if (kw === 'FILE') {
      fileIdx++;
    } else if (kw === 'TRACK') {
      const parts = line.split(/\s+/);
      const type = (parts[2] || '').toUpperCase();
      const audio = type === 'AUDIO';
      const size = audio ? 2352 : parseInt(type.split('/')[1] || '2352', 10);
      cur = { number: parseInt(parts[1], 10), file: fileIdx, audio, size, idx0: null, idx1: 0, pregap: 0 };
      entries.push(cur);
    } else if (kw === 'INDEX' && cur) {
      const parts = line.split(/\s+/);
      const n = parseInt(parts[1], 10);
      const f = msfToFrames(parts[2]);
      if (n === 0) cur.idx0 = f;
      if (n === 1) cur.idx1 = f;
    } else if (kw === 'PREGAP' && cur) {
      cur.pregap = msfToFrames(line.split(/\s+/)[1]);
    }
  }
  if (!entries.length) throw new Error('CUE tidak berisi track.');

  const rows = [];
  let fileBase = 150; // absolute frame where the current file starts
  let pregapAcc = 0;
  for (let fi = 0; fi < fileSizes.length; fi++) {
    const inFile = entries.filter((e) => e.file === fi);
    const sizeOf = inFile[0]?.size || 2352;
    const fileFrames = Math.floor(fileSizes[fi] / sizeOf);
    for (let k = 0; k < inFile.length; k++) {
      const t = inFile[k];
      pregapAcc += t.pregap;
      const next = inFile[k + 1];
      const endInFile = next ? (next.idx0 ?? next.idx1) : fileFrames;
      rows.push(...row({
        number: t.number,
        file: fi,
        start: fileBase + pregapAcc + t.idx1,
        length: Math.max(0, endInFile - t.idx1),
        size: t.size,
        audio: t.audio,
        offset: t.idx1 * t.size,
      }));
    }
    fileBase += fileFrames;
  }
  return rows;
}
