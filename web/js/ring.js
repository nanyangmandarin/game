// Single-producer / single-consumer ring of u32 words in a SharedArrayBuffer.
// ctrl[0] = write index, ctrl[1] = read index (both wrap as int32),
// ctrl[2] = sync requests completed, ctrl[3] = frames completed (consumer -> producer signals).
export const RING_WORDS = 1 << 21; // 8 MB

export function createRingBuffer() {
  return new SharedArrayBuffer(16 + RING_WORDS * 4);
}

export class Ring {
  constructor(sab) {
    this.ctrl = new Int32Array(sab, 0, 4);
    this.data = new Uint32Array(sab, 16, RING_WORDS);
  }

  /** Producer: append words, blocking (worker only) while the ring is full. */
  push(words) {
    const n = words.length;
    let off = 0;
    while (off < n) {
      const w = Atomics.load(this.ctrl, 0);
      const r = Atomics.load(this.ctrl, 1);
      const free = RING_WORDS - ((w - r) >>> 0);
      if (free === 0) {
        Atomics.wait(this.ctrl, 1, r, 100);
        continue;
      }
      const k = Math.min(free, n - off);
      const wi = w & (RING_WORDS - 1);
      const first = Math.min(k, RING_WORDS - wi);
      this.data.set(words.subarray(off, off + first), wi);
      if (k > first) this.data.set(words.subarray(off + first, off + k), 0);
      Atomics.store(this.ctrl, 0, (w + k) | 0);
      Atomics.notify(this.ctrl, 0);
      off += k;
    }
  }

  /** Consumer: copy out everything available, or null. */
  read() {
    const w = Atomics.load(this.ctrl, 0);
    const r = Atomics.load(this.ctrl, 1);
    const n = (w - r) >>> 0;
    if (n === 0) return null;
    const out = new Uint32Array(n);
    const ri = r & (RING_WORDS - 1);
    const first = Math.min(n, RING_WORDS - ri);
    out.set(this.data.subarray(ri, ri + first), 0);
    if (n > first) out.set(this.data.subarray(0, n - first), first);
    Atomics.store(this.ctrl, 1, (r + n) | 0);
    Atomics.notify(this.ctrl, 1);
    return out;
  }

  /** Consumer: sleep until the producer writes (or timeout). */
  waitData(ms) {
    const w = Atomics.load(this.ctrl, 0);
    if (((w - Atomics.load(this.ctrl, 1)) >>> 0) === 0) Atomics.wait(this.ctrl, 0, w, ms);
  }

  signal(i) {
    Atomics.add(this.ctrl, i, 1);
    Atomics.notify(this.ctrl, i);
  }

  /** Producer: wait until counter i reaches `target` (wrap-safe). */
  waitCounter(i, target, ms = 1000) {
    const t0 = performance.now();
    for (;;) {
      const v = Atomics.load(this.ctrl, i);
      if (((v - target) | 0) >= 0) return true;
      if (performance.now() - t0 > ms) return false;
      Atomics.wait(this.ctrl, i, v, 50);
    }
  }
}
