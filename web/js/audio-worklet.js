// Ring-buffer audio output. Receives interleaved stereo Float32 chunks over a MessagePort.
class PsxAudio extends AudioWorkletProcessor {
  constructor() {
    super();
    this.size = 44100 * 2; // 1 second stereo
    this.buf = new Float32Array(this.size);
    this.r = 0;
    this.w = 0;
    this.count = 0;
    this.maxFill = 8820; // ~100ms (in stereo frames*2)
    this.src = null;
    this.tick = 0;
    this.last = [0, 0];
    this.port.onmessage = (e) => {
      if (e.data && e.data.port) {
        this.src = e.data.port;
        this.src.onmessage = (m) => this.push(m.data);
      } else if (e.data && e.data.latency) {
        this.maxFill = Math.round(44100 * 2 * e.data.latency / 1000);
      }
    };
  }

  push(chunk) {
    for (let i = 0; i < chunk.length; i++) {
      if (this.count >= this.size) break;
      this.buf[this.w] = chunk[i];
      this.w = (this.w + 1) % this.size;
      this.count++;
    }
    // drop oldest when too far ahead (keeps latency bounded)
    while (this.count > this.maxFill * 2) {
      this.r = (this.r + 2) % this.size;
      this.count -= 2;
    }
  }

  process(_inputs, outputs) {
    const out = outputs[0];
    const L = out[0], R = out[1] || out[0];
    for (let i = 0; i < L.length; i++) {
      if (this.count >= 2) {
        this.last[0] = this.buf[this.r];
        this.last[1] = this.buf[(this.r + 1) % this.size];
        this.r = (this.r + 2) % this.size;
        this.count -= 2;
      } else {
        // underrun: fade last sample to avoid clicks
        this.last[0] *= 0.95;
        this.last[1] *= 0.95;
      }
      L[i] = this.last[0];
      R[i] = this.last[1];
    }
    if (this.src && ++this.tick % 16 === 0) this.src.postMessage({ fill: this.count });
    return true;
  }
}
registerProcessor('psx-audio', PsxAudio);
