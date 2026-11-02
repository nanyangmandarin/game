// Video output: Software (Canvas 2D, CPU only) or Hardware (WebGL, GPU) presentation.

const VS = `
attribute vec2 p;
varying vec2 uv;
void main(){ uv = vec2(p.x*0.5+0.5, 0.5-p.y*0.5); gl_Position = vec4(p,0.,1.); }`;

const FS = `
precision mediump float;
varying vec2 uv;
uniform sampler2D tex;
uniform vec2 texSize;
uniform vec2 outSize;
uniform int mode;      // 0 nearest/bilinear (sampler), 1 sharp bilinear, 2 CRT
uniform float scan;
void main(){
  vec2 c = uv;
  if (mode == 1 || mode == 2) {
    vec2 px = uv * texSize;
    vec2 scale = max(floor(outSize / texSize), vec2(1.0));
    vec2 f = fract(px);
    vec2 region = 0.5 - 0.5 / scale;
    vec2 off = (clamp(f - region, vec2(0.0), vec2(1.0) - 2.0*region) / (1.0 - 2.0*region)) - 0.5;
    c = (floor(px) + 0.5 + off) / texSize;
  }
  vec3 col = texture2D(tex, c).rgb;
  if (mode == 2) {
    float y = uv.y * texSize.y;
    float l = 0.5 + 0.5 * cos(6.2831853 * y);
    col *= mix(1.0, 0.55 + 0.45 * l, scan);
    float x = gl_FragCoord.x;
    vec3 mask = vec3(1.0);
    float m = mod(x, 3.0);
    if (m < 1.0) mask = vec3(1.0, 0.85, 0.85); else if (m < 2.0) mask = vec3(0.85, 1.0, 0.85); else mask = vec3(0.85, 0.85, 1.0);
    col *= mix(vec3(1.0), mask, scan) * (1.0 + 0.25*scan);
  }
  gl_FragColor = vec4(col, 1.0);
}`;

export class Presenter {
  constructor(host) {
    this.host = host;
    this.canvas = null;
    this.mode = null; // 'software' | 'gpu'
    this.opts = { aspect: '4:3', integer: false, filter: 'nearest', crt: 0.6, showVram: false };
    this.last = null;
    this.src = document.createElement('canvas'); // staging for 2D path
    this.srcCtx = this.src.getContext('2d');
    this.raf = 0;
    this.drawMs = 0;
    this.ro = new ResizeObserver(() => this.redraw());
    this.ro.observe(host);
  }

  /** Switch renderer. Returns the renderer actually in use (GPU falls back to software). */
  setRenderer(kind) {
    if (this.mode === kind && this.canvas) return this.mode;
    const fresh = () => {
      if (this.canvas) this.canvas.remove();
      this.canvas = document.createElement('canvas');
      this.canvas.className = 'screen';
      this.host.appendChild(this.canvas);
    };
    fresh();
    this.gl = null;
    this.ctx = null;
    if (kind === 'gpu' && this.initGL()) {
      this.mode = 'gpu';
    } else {
      if (kind === 'gpu') fresh(); // a canvas that tried WebGL can't switch to 2D
      this.ctx = this.canvas.getContext('2d', { alpha: false });
      this.mode = 'software';
    }
    this.resize();
    this.redraw();
    return this.mode;
  }

  initGL() {
    const gl = this.canvas.getContext('webgl', { alpha: false, antialias: false, preserveDrawingBuffer: false, powerPreference: 'high-performance' });
    if (!gl) return false;
    const sh = (t, s) => { const o = gl.createShader(t); gl.shaderSource(o, s); gl.compileShader(o); if (!gl.getShaderParameter(o, gl.COMPILE_STATUS)) throw new Error(gl.getShaderInfoLog(o)); return o; };
    try {
      const prog = gl.createProgram();
      gl.attachShader(prog, sh(gl.VERTEX_SHADER, VS));
      gl.attachShader(prog, sh(gl.FRAGMENT_SHADER, FS));
      gl.linkProgram(prog);
      if (!gl.getProgramParameter(prog, gl.LINK_STATUS)) return false;
      gl.useProgram(prog);
      const buf = gl.createBuffer();
      gl.bindBuffer(gl.ARRAY_BUFFER, buf);
      gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 1, -1, -1, 1, 1, 1]), gl.STATIC_DRAW);
      const loc = gl.getAttribLocation(prog, 'p');
      gl.enableVertexAttribArray(loc);
      gl.vertexAttribPointer(loc, 2, gl.FLOAT, false, 0, 0);
      this.tex = gl.createTexture();
      gl.bindTexture(gl.TEXTURE_2D, this.tex);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
      this.u = {
        texSize: gl.getUniformLocation(prog, 'texSize'),
        outSize: gl.getUniformLocation(prog, 'outSize'),
        mode: gl.getUniformLocation(prog, 'mode'),
        scan: gl.getUniformLocation(prog, 'scan'),
      };
      this.gl = gl;
      return true;
    } catch (e) {
      console.warn('WebGL init failed, falling back to software', e);
      return false;
    }
  }

  setOptions(o) {
    Object.assign(this.opts, o);
    this.redraw();
  }

  resize() {
    const r = this.host.getBoundingClientRect();
    const dpr = window.devicePixelRatio || 1;
    const w = Math.max(1, Math.round(r.width * dpr));
    const h = Math.max(1, Math.round(r.height * dpr));
    if (this.canvas.width !== w || this.canvas.height !== h) {
      this.canvas.width = w;
      this.canvas.height = h;
    }
  }

  /** Destination rectangle (device pixels) honoring aspect ratio and integer scaling. */
  viewport(fw, fh) {
    const W = this.canvas.width, H = this.canvas.height;
    let ar;
    switch (this.opts.showVram ? 'native' : this.opts.aspect) {
      case '16:9': ar = 16 / 9; break;
      case 'stretch': ar = W / H; break;
      case 'native': ar = fw / fh; break;
      default: ar = 4 / 3;
    }
    let w, h;
    if (this.opts.integer && !this.opts.showVram) {
      const k = Math.max(1, Math.floor(Math.min(W / (fh * ar), H / fh)));
      h = fh * k;
      w = Math.round(h * ar);
    } else if (W / H > ar) {
      h = H; w = Math.round(H * ar);
    } else {
      w = W; h = Math.round(W / ar);
    }
    return [Math.floor((W - w) / 2), Math.floor((H - h) / 2), w, h];
  }

  /** Queue a frame; only the newest one is drawn, once per display refresh. */
  frame(w, h, buffer) {
    this.last = { w, h, data: new Uint8ClampedArray(buffer) };
    if (!this.raf) {
      this.raf = requestAnimationFrame(() => {
        this.raf = 0;
        const t = performance.now();
        this.draw();
        this.drawMs = this.drawMs * 0.9 + (performance.now() - t) * 0.1;
      });
    }
  }

  redraw() {
    if (!this.canvas) return;
    this.resize();
    this.draw();
  }

  draw() {
    const f = this.last;
    if (this.gl) return this.drawGL(f);
    const ctx = this.ctx;
    ctx.fillStyle = '#000';
    ctx.fillRect(0, 0, this.canvas.width, this.canvas.height);
    if (!f) return;
    if (this.src.width !== f.w || this.src.height !== f.h) {
      this.src.width = f.w;
      this.src.height = f.h;
    }
    this.srcCtx.putImageData(new ImageData(f.data, f.w, f.h), 0, 0);
    const [x, y, w, h] = this.viewport(f.w, f.h);
    ctx.imageSmoothingEnabled = this.opts.filter !== 'nearest';
    ctx.imageSmoothingQuality = 'high';
    ctx.drawImage(this.src, 0, 0, f.w, f.h, x, y, w, h);
    if (this.opts.filter === 'crt' && this.opts.crt > 0) {
      // lightweight CPU scanlines
      ctx.fillStyle = `rgba(0,0,0,${0.35 * this.opts.crt})`;
      const step = h / f.h;
      if (step >= 2) for (let yy = y + step / 2; yy < y + h; yy += step) ctx.fillRect(x, Math.floor(yy), w, Math.max(1, Math.floor(step / 2)));
    }
  }

  drawGL(f) {
    const gl = this.gl;
    gl.viewport(0, 0, this.canvas.width, this.canvas.height);
    gl.clearColor(0, 0, 0, 1);
    gl.clear(gl.COLOR_BUFFER_BIT);
    if (!f) return;
    gl.bindTexture(gl.TEXTURE_2D, this.tex);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, f.w, f.h, 0, gl.RGBA, gl.UNSIGNED_BYTE, new Uint8Array(f.data.buffer));
    const filt = this.opts.filter === 'nearest' ? gl.NEAREST : gl.LINEAR;
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, filt);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, filt);
    const [x, y, w, h] = this.viewport(f.w, f.h);
    gl.viewport(x, this.canvas.height - y - h, w, h);
    gl.uniform2f(this.u.texSize, f.w, f.h);
    gl.uniform2f(this.u.outSize, w, h);
    const mode = { nearest: 0, smooth: 0, sharp: 1, crt: 2 }[this.opts.filter] ?? 0;
    gl.uniform1i(this.u.mode, mode);
    gl.uniform1f(this.u.scan, this.opts.crt);
    gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);
  }

  screenshot() {
    this.draw(); // re-render in the same task so the WebGL buffer is valid
    return new Promise((res) => this.canvas.toBlob(res, 'image/png'));
  }
}
