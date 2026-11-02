// High-resolution hardware renderer (WebGL2). Replays the draw list recorded by the GPU
// thread at N x internal resolution. Textures/CLUTs are read from the authoritative
// software VRAM (uploaded once per frame), so texturing stays accurate.
import { Painter } from './painter.js';

const HW_TRI = 1, HW_LINE = 2, HW_FILL = 3, HW_COPY = 4, HW_UPLOAD = 5;
const STRIDE = 11; // words per vertex: x y r g b u v | p0 p1 p2 p3

const VS_DRAW = `#version 300 es
layout(location=0) in vec2 aPos;
layout(location=1) in vec3 aCol;
layout(location=2) in vec2 aUV;
layout(location=3) in uvec4 aP;
out vec3 vCol; out vec2 vUV; flat out uvec4 vP;
void main() {
  vCol = aCol; vUV = aUV; vP = aP;
  gl_Position = vec4(aPos.x / 512.0 - 1.0, aPos.y / 256.0 - 1.0, 0.0, 1.0);
}`;

const FS_DRAW = `#version 300 es
precision highp float; precision highp int; precision highp usampler2D;
uniform usampler2D uVram;
uniform int uPass; // 0 all texels, 1 opaque texels only, 2 semi-transparent texels only
in vec3 vCol; in vec2 vUV; flat in uvec4 vP;
out vec4 o;
uint vram(uint x, uint y) { return texelFetch(uVram, ivec2(int(x & 1023u), int(y & 511u)), 0).r; }
void main() {
  uint p0 = vP.x;
  vec3 c = vCol;
  if (((p0 >> 12u) & 1u) == 1u) {
    uint tpx = p0 & 15u, tpy = (p0 >> 4u) & 1u, depth = (p0 >> 8u) & 3u;
    uint cx = vP.y & 0xFFFFu, cy = vP.y >> 16u;
    uint mx = vP.z & 31u, my = (vP.z >> 8u) & 31u, ox = (vP.z >> 16u) & 31u, oy = (vP.z >> 24u) & 31u;
    uint u = uint(int(floor(vUV.x)) & 255), v = uint(int(floor(vUV.y)) & 255); // texcoords wrap like on the PS1
    u = (u & ~(mx * 8u)) | ((ox & mx) * 8u);
    v = (v & ~(my * 8u)) | ((oy & my) * 8u);
    uint bx = tpx * 64u, by = tpy * 256u + v;
    uint t;
    if (depth == 0u) { uint w = vram(bx + (u >> 2u), by); t = vram(cx + ((w >> ((u & 3u) * 4u)) & 15u), cy); }
    else if (depth == 1u) { uint w = vram(bx + (u >> 1u), by); t = vram(cx + ((w >> ((u & 1u) * 8u)) & 255u), cy); }
    else { t = vram(bx + u, by); }
    if (t == 0u) discard;
    bool stp = (t & 0x8000u) != 0u;
    if (uPass == 1 && stp) discard;
    if (uPass == 2 && !stp) discard;
    vec3 tc = vec3(float(t & 31u), float((t >> 5u) & 31u), float((t >> 10u) & 31u)) * 8.0;
    c = ((p0 >> 13u) & 1u) == 1u ? tc : min(tc * vCol / 128.0, vec3(255.0));
  }
  o = vec4(c / 255.0, 1.0);
}`;

// region quad in native VRAM coordinates
const VS_QUAD = `#version 300 es
layout(location=0) in vec2 aPos;
out vec2 vNat;
uniform vec4 uRect; // x, y, w, h (native)
void main() {
  vNat = uRect.xy + aPos * uRect.zw;
  gl_Position = vec4(vNat.x / 512.0 - 1.0, vNat.y / 256.0 - 1.0, 0.0, 1.0);
}`;

const FS_UPLOAD = `#version 300 es
precision highp float; precision highp usampler2D;
uniform usampler2D uVram;
in vec2 vNat; out vec4 o;
void main() {
  uint t = texelFetch(uVram, ivec2(int(floor(vNat.x)) & 1023, int(floor(vNat.y)) & 511), 0).r;
  o = vec4(float(t & 31u), float((t >> 5u) & 31u), float((t >> 10u) & 31u), 31.0) / 31.0;
}`;

const FS_COPY = `#version 300 es
precision highp float;
uniform sampler2D uSrc; uniform vec4 uRect;
in vec2 vNat; out vec4 o;
void main() { o = texture(uSrc, (vNat - uRect.xy) / uRect.zw); }`;

const VS_PRESENT = `#version 300 es
layout(location=0) in vec2 aPos;
out vec2 vUV;
void main() { vUV = vec2(aPos.x, aPos.y); gl_Position = vec4(aPos.x * 2.0 - 1.0, 1.0 - aPos.y * 2.0, 0.0, 1.0); }`;

const FS_PRESENT = `#version 300 es
precision highp float; precision highp usampler2D;
uniform sampler2D uTarget; uniform usampler2D uVram;
uniform vec4 uRegion;   // hi-res pixels: x, y, w, h
uniform vec2 uTargetSize;
uniform int uDepth24; uniform vec4 uDisp; // native display x, y, w, h (24-bit path)
uniform float uScan; uniform vec2 uOut;
in vec2 vUV; out vec4 o;
uint vram(int x, int y) { return texelFetch(uVram, ivec2(x & 1023, y & 511), 0).r; }
void main() {
  vec3 c;
  if (uDepth24 == 1) {
    ivec2 p = ivec2(floor(vUV * uDisp.zw));
    int y = int(uDisp.y) + p.y;
    int b = int(uDisp.x) * 2 + p.x * 3;
    uint w0 = vram(b >> 1, y), w1 = vram((b + 1) >> 1, y), w2 = vram((b + 2) >> 1, y);
    float r = float((b & 1) == 0 ? (w0 & 255u) : (w0 >> 8u));
    float g = float(((b + 1) & 1) == 0 ? (w1 & 255u) : (w1 >> 8u));
    float bb = float(((b + 2) & 1) == 0 ? (w2 & 255u) : (w2 >> 8u));
    c = vec3(r, g, bb) / 255.0;
  } else {
    c = texture(uTarget, (uRegion.xy + vUV * uRegion.zw) / uTargetSize).rgb;
  }
  if (uScan > 0.0) {
    float l = 0.5 + 0.5 * cos(6.2831853 * vUV.y * uDisp.w);
    c *= mix(1.0, 0.6 + 0.4 * l, uScan) * (1.0 + 0.2 * uScan);
  }
  o = vec4(c, 1.0);
}`;

function compile(gl, vs, fs) {
  const sh = (t, s) => {
    const o = gl.createShader(t);
    gl.shaderSource(o, s);
    gl.compileShader(o);
    if (!gl.getShaderParameter(o, gl.COMPILE_STATUS)) throw new Error(gl.getShaderInfoLog(o));
    return o;
  };
  const p = gl.createProgram();
  gl.attachShader(p, sh(gl.VERTEX_SHADER, vs));
  gl.attachShader(p, sh(gl.FRAGMENT_SHADER, fs));
  gl.linkProgram(p);
  if (!gl.getProgramParameter(p, gl.LINK_STATUS)) throw new Error(gl.getProgramInfoLog(p));
  const u = {};
  const n = gl.getProgramParameter(p, gl.ACTIVE_UNIFORMS);
  for (let i = 0; i < n; i++) {
    const name = gl.getActiveUniform(p, i).name;
    u[name] = gl.getUniformLocation(p, name);
  }
  return { p, u };
}

export class HwRenderer {
  /** Throws if WebGL2 is unavailable. */
  constructor(canvas, scale) {
    this.canvas = canvas;
    const gl = canvas.getContext('webgl2', { alpha: false, antialias: false, depth: false, stencil: false, preserveDrawingBuffer: false, powerPreference: 'high-performance' });
    if (!gl) throw new Error('WebGL2 tidak tersedia');
    this.gl = gl;
    const max = gl.getParameter(gl.MAX_TEXTURE_SIZE);
    this.scale = Math.max(1, Math.min(scale, Math.floor(max / 1024)));
    this.W = 1024 * this.scale;
    this.H = 512 * this.scale;
    this.opts = { aspect: '4:3', integer: false, filter: 'nearest', crt: 0.6, showVram: false };
    this.mode = 'gpu';
    this.failed = false;
    this.drawMs = 0;
    this.last = null; // display info of the last presented frame

    this.draw = compile(gl, VS_DRAW, FS_DRAW);
    this.upload = compile(gl, VS_QUAD, FS_UPLOAD);
    this.copy = compile(gl, VS_QUAD, FS_COPY);
    this.present = compile(gl, VS_PRESENT, FS_PRESENT);

    // native VRAM (R16UI) used for texturing
    this.vramTex = gl.createTexture();
    gl.bindTexture(gl.TEXTURE_2D, this.vramTex);
    gl.texStorage2D(gl.TEXTURE_2D, 1, gl.R16UI, 1024, 512);
    this.nearest(gl.TEXTURE_2D);

    // hi-res render target
    this.target = gl.createTexture();
    gl.bindTexture(gl.TEXTURE_2D, this.target);
    gl.texStorage2D(gl.TEXTURE_2D, 1, gl.RGBA8, this.W, this.H);
    this.nearest(gl.TEXTURE_2D);
    this.fbo = gl.createFramebuffer();
    gl.bindFramebuffer(gl.FRAMEBUFFER, this.fbo);
    gl.framebufferTexture2D(gl.FRAMEBUFFER, gl.COLOR_ATTACHMENT0, gl.TEXTURE_2D, this.target, 0);
    gl.clearColor(0, 0, 0, 1);
    gl.clear(gl.COLOR_BUFFER_BIT);

    this.temp = gl.createTexture(); // scratch for VRAM->VRAM copies
    this.tempSize = [0, 0];

    // geometry
    this.vao = gl.createVertexArray();
    this.vbo = gl.createBuffer();
    gl.bindVertexArray(this.vao);
    gl.bindBuffer(gl.ARRAY_BUFFER, this.vbo);
    const B = STRIDE * 4;
    gl.enableVertexAttribArray(0); gl.vertexAttribPointer(0, 2, gl.FLOAT, false, B, 0);
    gl.enableVertexAttribArray(1); gl.vertexAttribPointer(1, 3, gl.FLOAT, false, B, 8);
    gl.enableVertexAttribArray(2); gl.vertexAttribPointer(2, 2, gl.FLOAT, false, B, 20);
    gl.enableVertexAttribArray(3); gl.vertexAttribIPointer(3, 4, gl.UNSIGNED_INT, B, 28);
    this.quadVao = gl.createVertexArray();
    gl.bindVertexArray(this.quadVao);
    const qb = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, qb);
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([0, 0, 1, 0, 0, 1, 1, 1]), gl.STATIC_DRAW);
    gl.enableVertexAttribArray(0);
    gl.vertexAttribPointer(0, 2, gl.FLOAT, false, 0, 0);
    gl.bindVertexArray(null);

    this.cap = 4096;
    this.buf = new ArrayBuffer(this.cap * STRIDE * 4);
    this.f32 = new Float32Array(this.buf);
    this.u32 = new Uint32Array(this.buf);
    this.n = 0; // vertices in the current batch
  }

  nearest(t) {
    const gl = this.gl;
    gl.texParameteri(t, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
    gl.texParameteri(t, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
    gl.texParameteri(t, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
    gl.texParameteri(t, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
  }

  setSize(w, h) {
    if (this.canvas.width !== w || this.canvas.height !== h) {
      this.canvas.width = Math.max(1, w | 0);
      this.canvas.height = Math.max(1, h | 0);
    }
    if (this.last) this.show(this.last);
  }

  setOptions(o) {
    Object.assign(this.opts, o);
    if (this.last) this.show(this.last);
  }

  viewport(fw, fh) {
    return Painter.prototype.viewport.call(this, fw, fh);
  }

  // ---------------- batching ----------------

  vertex(x, y, rgb, u, v, p0, p1, p2) {
    if (this.n >= this.cap) this.grow();
    const o = this.n * STRIDE;
    this.f32[o] = x; this.f32[o + 1] = y;
    this.f32[o + 2] = rgb & 255; this.f32[o + 3] = (rgb >> 8) & 255; this.f32[o + 4] = (rgb >> 16) & 255;
    this.f32[o + 5] = u; this.f32[o + 6] = v;
    this.u32[o + 7] = p0; this.u32[o + 8] = p1; this.u32[o + 9] = p2; this.u32[o + 10] = 0;
    this.n++;
  }

  grow() {
    this.cap *= 2;
    const nb = new ArrayBuffer(this.cap * STRIDE * 4);
    new Uint32Array(nb).set(this.u32);
    this.buf = nb;
    this.f32 = new Float32Array(nb);
    this.u32 = new Uint32Array(nb);
  }

  blend(on, mode) {
    const gl = this.gl;
    if (!on) { gl.disable(gl.BLEND); return; }
    gl.enable(gl.BLEND);
    switch (mode) {
      case 0: gl.blendColor(0, 0, 0, 0.5); gl.blendEquation(gl.FUNC_ADD); gl.blendFunc(gl.CONSTANT_ALPHA, gl.CONSTANT_ALPHA); break;
      case 1: gl.blendEquation(gl.FUNC_ADD); gl.blendFunc(gl.ONE, gl.ONE); break;
      case 2: gl.blendEquation(gl.FUNC_REVERSE_SUBTRACT); gl.blendFunc(gl.ONE, gl.ONE); break;
      default: gl.blendColor(0, 0, 0, 0.25); gl.blendEquation(gl.FUNC_ADD); gl.blendFunc(gl.CONSTANT_ALPHA, gl.ONE); break;
    }
  }

  flush() {
    if (!this.n || !this.key) { this.n = 0; return; }
    const gl = this.gl;
    const k = this.key;
    gl.useProgram(this.draw.p);
    gl.bindVertexArray(this.vao);
    gl.bindBuffer(gl.ARRAY_BUFFER, this.vbo);
    gl.bufferData(gl.ARRAY_BUFFER, this.u32.subarray(0, this.n * STRIDE), gl.STREAM_DRAW);
    const S = this.scale;
    gl.enable(gl.SCISSOR_TEST);
    gl.scissor(k.x1 * S, k.y1 * S, (k.x2 - k.x1 + 1) * S, (k.y2 - k.y1 + 1) * S);
    if (k.semi && k.textured) {
      // per-texel semi-transparency: opaque texels first, then blended ones
      this.blend(false);
      gl.uniform1i(this.draw.u.uPass, 1);
      gl.drawArrays(gl.TRIANGLES, 0, this.n);
      this.blend(true, k.mode);
      gl.uniform1i(this.draw.u.uPass, 2);
      gl.drawArrays(gl.TRIANGLES, 0, this.n);
    } else {
      this.blend(k.semi, k.mode);
      gl.uniform1i(this.draw.u.uPass, 0);
      gl.drawArrays(gl.TRIANGLES, 0, this.n);
    }
    gl.disable(gl.SCISSOR_TEST);
    this.blend(false);
    this.n = 0;
  }

  setKey(semi, textured, mode, a1, a2) {
    const x1 = a1 & 0x3FF, y1 = (a1 >>> 16) & 0x1FF, x2 = a2 & 0x3FF, y2 = (a2 >>> 16) & 0x1FF;
    const k = this.key;
    if (k && k.semi === semi && k.mode === mode && (!semi || k.textured === textured) && k.x1 === x1 && k.y1 === y1 && k.x2 === x2 && k.y2 === y2) return;
    this.flush();
    this.key = { semi, textured, mode, x1, y1, x2, y2 };
  }

  // ---------------- per frame ----------------

  /**
   * Replay one frame's draw list.
   * @param {Uint32Array} list  recorded draw list (wasm memory view)
   * @param {Uint16Array} vram  authoritative native VRAM (wasm memory view)
   */
  render(list, vram) {
    const gl = this.gl;
    const t0 = performance.now();
    gl.bindTexture(gl.TEXTURE_2D, this.vramTex);
    gl.pixelStorei(gl.UNPACK_ALIGNMENT, 2);
    gl.texSubImage2D(gl.TEXTURE_2D, 0, 0, 0, 1024, 512, gl.RED_INTEGER, gl.UNSIGNED_SHORT, vram);
    gl.bindFramebuffer(gl.FRAMEBUFFER, this.fbo);
    gl.viewport(0, 0, this.W, this.H);
    gl.useProgram(this.draw.p);
    gl.activeTexture(gl.TEXTURE0);
    gl.bindTexture(gl.TEXTURE_2D, this.vramTex);
    gl.uniform1i(this.draw.u.uVram, 0);
    this.key = null;
    this.n = 0;
    let i = 0;
    const L = list.length;
    while (i < L) {
      const tag = list[i];
      if (tag === HW_TRI) {
        const p0 = list[i + 1], p1 = list[i + 2], p2 = list[i + 3];
        this.setKey(((p0 >> 14) & 1) === 1, ((p0 >> 12) & 1) === 1, (p0 >> 16) & 3, list[i + 4], list[i + 5]);
        for (let k = 0; k < 3; k++) {
          const o = i + 6 + k * 4;
          const uv = list[o + 3];
          this.vertex(list[o] | 0, list[o + 1] | 0, list[o + 2], ((uv & 0xFFFF) << 16) >> 16, uv >> 16 << 16 >> 16, p0, p1, p2);
        }
        i += 18;
      } else if (tag === HW_LINE) {
        const p0 = list[i + 1];
        this.setKey(((p0 >> 14) & 1) === 1, false, (p0 >> 16) & 3, list[i + 2], list[i + 3]);
        this.lineQuad(list[i + 4] | 0, list[i + 5] | 0, list[i + 6], list[i + 7] | 0, list[i + 8] | 0, list[i + 9]);
        i += 10;
      } else if (tag === HW_FILL) {
        this.flush();
        this.key = null;
        const [x, y, w, h, c] = [list[i + 1], list[i + 2], list[i + 3], list[i + 4], list[i + 5]];
        const S = this.scale;
        gl.enable(gl.SCISSOR_TEST);
        gl.scissor(x * S, y * S, w * S, h * S);
        gl.clearColor((c & 255) / 255, ((c >> 8) & 255) / 255, ((c >> 16) & 255) / 255, 1);
        gl.clear(gl.COLOR_BUFFER_BIT);
        gl.disable(gl.SCISSOR_TEST);
        i += 6;
      } else if (tag === HW_COPY) {
        this.flush();
        this.key = null;
        this.copyRect(list[i + 1], list[i + 2], list[i + 3], list[i + 4], list[i + 5], list[i + 6]);
        i += 7;
      } else if (tag === HW_UPLOAD) {
        this.flush();
        this.key = null;
        this.quad(this.upload, list[i + 1], list[i + 2], list[i + 3], list[i + 4], () => {
          gl.activeTexture(gl.TEXTURE0);
          gl.bindTexture(gl.TEXTURE_2D, this.vramTex);
          gl.uniform1i(this.upload.u.uVram, 0);
        });
        i += 5;
      } else {
        break; // unknown record: stop rather than misparse
      }
    }
    this.flush();
    gl.bindVertexArray(null);
    this.drawMs = this.drawMs * 0.9 + (performance.now() - t0) * 0.1;
  }

  lineQuad(x0, y0, c0, x1, y1, c1) {
    // 1 native pixel wide quad covering both end pixels
    let dx = x1 - x0, dy = y1 - y0;
    const len = Math.hypot(dx, dy) || 1;
    dx /= len; dy /= len;
    const nx = -dy * 0.5, ny = dx * 0.5;
    const ax = x0 + 0.5 - dx * 0.5, ay = y0 + 0.5 - dy * 0.5;
    const bx = x1 + 0.5 + dx * 0.5, by = y1 + 0.5 + dy * 0.5;
    const v = (x, y, c) => this.vertex(x, y, c, 0, 0, 0, 0, 0);
    v(ax + nx, ay + ny, c0); v(bx + nx, by + ny, c1); v(ax - nx, ay - ny, c0);
    v(bx + nx, by + ny, c1); v(ax - nx, ay - ny, c0); v(bx - nx, by - ny, c1);
  }

  quad(prog, x, y, w, h, bind) {
    const gl = this.gl;
    gl.useProgram(prog.p);
    bind();
    gl.uniform4f(prog.u.uRect, x, y, w, h);
    gl.bindVertexArray(this.quadVao);
    gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);
    gl.bindVertexArray(this.vao);
    gl.useProgram(this.draw.p);
  }

  copyRect(sx, sy, dx, dy, w, h) {
    const gl = this.gl;
    const S = this.scale;
    const W = Math.min(w, 1024 - sx) * S, H = Math.min(h, 512 - sy) * S;
    if (W <= 0 || H <= 0) return;
    gl.bindTexture(gl.TEXTURE_2D, this.temp);
    if (this.tempSize[0] !== W || this.tempSize[1] !== H) {
      gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA8, W, H, 0, gl.RGBA, gl.UNSIGNED_BYTE, null);
      this.nearest(gl.TEXTURE_2D);
      this.tempSize = [W, H];
    }
    gl.copyTexSubImage2D(gl.TEXTURE_2D, 0, 0, 0, sx * S, sy * S, W, H);
    this.quad(this.copy, dx, dy, W / S, H / S, () => {
      gl.activeTexture(gl.TEXTURE0);
      gl.bindTexture(gl.TEXTURE_2D, this.temp);
      gl.uniform1i(this.copy.u.uSrc, 0);
    });
  }

  /** Present the display area. info = [x, y, w, h, depth24, enabled] */
  show(info) {
    this.last = info;
    const gl = this.gl;
    const [x, y, w, h, d24, on] = info;
    gl.bindFramebuffer(gl.FRAMEBUFFER, null);
    gl.viewport(0, 0, this.canvas.width, this.canvas.height);
    gl.clearColor(0, 0, 0, 1);
    gl.clear(gl.COLOR_BUFFER_BIT);
    if (!on || !w || !h) return;
    const [vx, vy, vw, vh] = this.viewport(w, h);
    gl.viewport(vx, this.canvas.height - vy - vh, vw, vh);
    const S = this.scale;
    const p = this.present;
    gl.useProgram(p.p);
    gl.activeTexture(gl.TEXTURE0);
    gl.bindTexture(gl.TEXTURE_2D, this.target);
    const filt = this.opts.filter === 'nearest' ? gl.NEAREST : gl.LINEAR;
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, filt);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, filt);
    gl.uniform1i(p.u.uTarget, 0);
    gl.activeTexture(gl.TEXTURE1);
    gl.bindTexture(gl.TEXTURE_2D, this.vramTex);
    gl.uniform1i(p.u.uVram, 1);
    gl.activeTexture(gl.TEXTURE0);
    gl.uniform4f(p.u.uRegion, x * S, y * S, w * S, h * S);
    gl.uniform2f(p.u.uTargetSize, this.W, this.H);
    gl.uniform1i(p.u.uDepth24, d24 ? 1 : 0);
    gl.uniform4f(p.u.uDisp, x, y, w, h);
    gl.uniform1f(p.u.uScan, this.opts.filter === 'crt' ? this.opts.crt : 0);
    gl.bindVertexArray(this.quadVao);
    gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);
    gl.bindVertexArray(null);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
  }

  snapshot() {
    return null; // screenshots use the native software frame
  }
}
