// Memory card image helpers: directory listing, delete, import formats.
export const CARD_SIZE = 131072;

let sjis = null;
try { sjis = new TextDecoder('shift-jis'); } catch { sjis = null; }

function checksum(card, frame) {
  const o = frame * 128;
  let x = 0;
  for (let i = 0; i < 127; i++) x ^= card[o + i];
  card[o + 127] = x;
}

/** List save files on a card image. */
export function listSaves(card) {
  const out = [];
  for (let i = 1; i < 16; i++) {
    const o = i * 128;
    if (card[o] !== 0x51) continue;
    const size = card[o + 4] | (card[o + 5] << 8) | (card[o + 6] << 16);
    let name = '';
    for (let k = 0; k < 20 && card[o + 0x0a + k]; k++) name += String.fromCharCode(card[o + 0x0a + k]);
    const blk = i * 8192;
    let title = name;
    if (card[blk] === 0x53 && card[blk + 1] === 0x43) {
      const bytes = card.slice(blk + 4, blk + 4 + 64);
      const end = bytes.indexOf(0);
      const t = sjis ? sjis.decode(bytes.subarray(0, end < 0 ? 64 : end)) : '';
      if (t) title = t.normalize('NFKC');
    }
    out.push({ block: i, blocks: Math.max(1, Math.round(size / 8192)), name, title });
  }
  return out;
}

export function freeBlocks(card) {
  let n = 0;
  for (let i = 1; i < 16; i++) if ((card[i * 128] & 0xf0) === 0xa0) n++;
  return n;
}

/** Delete a save (marks its block chain as free). */
export function deleteSave(card, first) {
  let i = first;
  let guard = 0;
  while (i >= 1 && i < 16 && guard++ < 16) {
    const o = i * 128;
    const st = card[o];
    if (st === 0x51 || st === 0x52 || st === 0x53) card[o] = 0xa0 | (st & 0x0f);
    const next = card[o + 8] | (card[o + 9] << 8);
    checksum(card, i);
    if (next === 0xffff) break;
    i = next + 1;
  }
}

/** Accepts raw (.mcr/.mcd/.srm/.ps) and DexDrive (.gme) images. */
export function importCard(buf) {
  const u = new Uint8Array(buf);
  if (u.length === CARD_SIZE) return u;
  if (u.length === CARD_SIZE + 3904) return u.slice(3904);
  if (u.length > CARD_SIZE && u[0] === 0x4d && u[1] === 0x43) return u.slice(0, CARD_SIZE);
  // VGS / other headers: find "MC" at a 128-aligned offset
  for (let o = 0; o + CARD_SIZE <= u.length; o += 64) {
    if (u[o] === 0x4d && u[o + 1] === 0x43) return u.slice(o, o + CARD_SIZE);
  }
  throw new Error('File memory card tidak dikenali (harus 128 KB .mcr/.mcd/.gme).');
}
