//! MDEC: macroblock decoder used for FMV playback.

use std::collections::VecDeque;

const ZIGZAG: [usize; 64] = [
    0, 1, 5, 6, 14, 15, 27, 28, 2, 4, 7, 13, 16, 26, 29, 42, 3, 8, 12, 17, 25, 30, 41, 43, 9, 11, 18, 24, 31, 40, 44,
    53, 10, 19, 23, 32, 39, 45, 52, 54, 20, 22, 33, 38, 46, 51, 55, 60, 21, 34, 37, 47, 50, 56, 59, 61, 35, 36, 48,
    49, 57, 58, 62, 63,
];

#[derive(PartialEq, Clone, Copy)]
enum State {
    Idle,
    Decode,
    QuantTable,
    ScaleTable,
}

pub struct Mdec {
    state: State,
    remaining: u32,
    depth: u32,
    signed: bool,
    bit15: bool,
    color_qt: bool,
    input: Vec<u16>,
    qt_y: [u8; 64],
    qt_uv: [u8; 64],
    scale: [i16; 64],
    zagzig: [usize; 64],
    out: VecDeque<u32>,
    dma_in: bool,
    dma_out: bool,
    table_words: Vec<u32>,
}

impl Mdec {
    pub fn new() -> Self {
        let mut zagzig = [0usize; 64];
        for (i, &z) in ZIGZAG.iter().enumerate() {
            zagzig[z] = i;
        }
        Mdec {
            state: State::Idle,
            remaining: 0,
            depth: 0,
            signed: false,
            bit15: false,
            color_qt: false,
            input: Vec::new(),
            qt_y: [1; 64],
            qt_uv: [1; 64],
            scale: [0; 64],
            zagzig,
            out: VecDeque::new(),
            dma_in: false,
            dma_out: false,
            table_words: Vec::new(),
        }
    }

    pub fn out_len(&self) -> usize {
        self.out.len()
    }

    pub fn status(&self) -> u32 {
        let mut s = 0u32;
        if self.out.is_empty() {
            s |= 1 << 31;
        }
        if self.state != State::Idle {
            s |= 1 << 29;
        }
        if self.dma_in && self.state != State::Idle {
            s |= 1 << 28;
        }
        if self.dma_out && !self.out.is_empty() {
            s |= 1 << 27;
        }
        s |= self.depth << 25;
        s |= (self.signed as u32) << 24;
        s |= (self.bit15 as u32) << 23;
        s |= self.remaining.wrapping_sub(1) & 0xFFFF;
        s
    }

    pub fn write_ctrl(&mut self, v: u32) {
        if v & (1 << 31) != 0 {
            self.state = State::Idle;
            self.remaining = 0;
            self.input.clear();
            self.out.clear();
        }
        self.dma_in = v & (1 << 30) != 0;
        self.dma_out = v & (1 << 29) != 0;
    }

    pub fn write_cmd(&mut self, w: u32) {
        match self.state {
            State::Idle => {
                let cmd = w >> 29;
                self.depth = (w >> 27) & 3;
                self.signed = w & (1 << 26) != 0;
                self.bit15 = w & (1 << 25) != 0;
                match cmd {
                    1 => {
                        self.remaining = w & 0xFFFF;
                        self.input.clear();
                        self.state = State::Decode;
                    }
                    2 => {
                        self.color_qt = w & 1 != 0;
                        self.remaining = if self.color_qt { 32 } else { 16 };
                        self.table_words.clear();
                        self.state = State::QuantTable;
                    }
                    3 => {
                        self.remaining = 32;
                        self.table_words.clear();
                        self.state = State::ScaleTable;
                    }
                    _ => {}
                }
                if self.remaining == 0 {
                    self.state = State::Idle;
                }
            }
            State::Decode => {
                self.input.push(w as u16);
                self.input.push((w >> 16) as u16);
                self.remaining -= 1;
                if self.remaining == 0 {
                    self.decode_all();
                    self.state = State::Idle;
                }
            }
            State::QuantTable | State::ScaleTable => {
                self.table_words.push(w);
                self.remaining -= 1;
                if self.remaining == 0 {
                    let bytes: Vec<u8> = self.table_words.iter().flat_map(|w| w.to_le_bytes()).collect();
                    if self.state == State::QuantTable {
                        self.qt_y.copy_from_slice(&bytes[0..64]);
                        if self.color_qt {
                            self.qt_uv.copy_from_slice(&bytes[64..128]);
                        }
                    } else {
                        for i in 0..64 {
                            self.scale[i] = i16::from_le_bytes([bytes[i * 2], bytes[i * 2 + 1]]);
                        }
                    }
                    self.state = State::Idle;
                }
            }
        }
    }

    pub fn read_data(&mut self) -> u32 {
        self.out.pop_front().unwrap_or(0)
    }

    fn decode_block(&self, pos: &mut usize, qt: &[u8; 64], blk: &mut [i32; 64]) -> bool {
        let inp = &self.input;
        blk.fill(0);
        while *pos < inp.len() && inp[*pos] == 0xFE00 {
            *pos += 1;
        }
        if *pos >= inp.len() {
            return false;
        }
        let mut n = inp[*pos];
        *pos += 1;
        let q = (n >> 10) as i32;
        let sx = |n: u16| (((n & 0x3FF) << 6) as i16 >> 6) as i32;
        let mut k = 0usize;
        let mut val = sx(n) * qt[0] as i32;
        loop {
            if q == 0 {
                val = sx(n) * 2;
            }
            val = val.clamp(-0x400, 0x3FF);
            if q > 0 {
                blk[self.zagzig[k]] = val;
            } else {
                blk[k] = val;
            }
            if *pos >= inp.len() {
                break;
            }
            n = inp[*pos];
            *pos += 1;
            if n == 0xFE00 {
                break;
            }
            k += ((n >> 10) + 1) as usize;
            if k > 63 {
                break;
            }
            val = (sx(n) * qt[k] as i32 * q + 4) / 8;
        }
        self.idct(blk);
        true
    }

    fn idct(&self, blk: &mut [i32; 64]) {
        let mut tmp = [0i64; 64];
        let src = *blk;
        for x in 0..8 {
            for y in 0..8 {
                let mut sum = 0i64;
                for z in 0..8 {
                    sum += src[y + z * 8] as i64 * (self.scale[x + z * 8] as i64 / 8);
                }
                tmp[x + y * 8] = (sum + 0xFFF) / 0x2000;
            }
        }
        for x in 0..8 {
            for y in 0..8 {
                let mut sum = 0i64;
                for z in 0..8 {
                    sum += tmp[y + z * 8] * (self.scale[x + z * 8] as i64 / 8);
                }
                blk[x + y * 8] = ((sum + 0xFFF) / 0x2000) as i32;
            }
        }
    }

    fn decode_all(&mut self) {
        let mut pos = 0usize;
        let qt_y = self.qt_y;
        let qt_uv = self.qt_uv;
        let mut cr = [0i32; 64];
        let mut cb = [0i32; 64];
        let mut y = [[0i32; 64]; 4];
        let words: Vec<u32>;
        let mut outw: Vec<u32> = Vec::new();
        if self.depth >= 2 {
            loop {
                if !self.decode_block(&mut pos, &qt_uv, &mut cr) {
                    break;
                }
                if !self.decode_block(&mut pos, &qt_uv, &mut cb) {
                    break;
                }
                let mut ok = true;
                for b in y.iter_mut() {
                    if !self.decode_block(&mut pos, &qt_y, b) {
                        ok = false;
                        break;
                    }
                }
                if !ok {
                    break;
                }
                let mut rgb = [[0u8; 3]; 256];
                for (bi, b) in y.iter().enumerate() {
                    let (xx, yy) = ((bi & 1) * 8, (bi >> 1) * 8);
                    for py in 0..8 {
                        for px in 0..8 {
                            let ci = (px + xx) / 2 + ((py + yy) / 2) * 8;
                            let r0 = cr[ci] as f32;
                            let b0 = cb[ci] as f32;
                            let g = (-0.3437 * b0 + -0.7143 * r0) as i32;
                            let r = (1.402 * r0) as i32;
                            let bb = (1.772 * b0) as i32;
                            let l = b[px + py * 8];
                            let mut c = [(l + r).clamp(-128, 127), (l + g).clamp(-128, 127), (l + bb).clamp(-128, 127)];
                            if !self.signed {
                                for e in c.iter_mut() {
                                    *e += 128;
                                }
                            }
                            rgb[(py + yy) * 16 + px + xx] = [c[0] as u8, c[1] as u8, c[2] as u8];
                        }
                    }
                }
                if self.depth == 2 {
                    let bytes: Vec<u8> = rgb.iter().flatten().copied().collect();
                    for ch in bytes.chunks(4) {
                        outw.push(u32::from_le_bytes([ch[0], ch[1], ch[2], ch[3]]));
                    }
                } else {
                    let b15 = if self.bit15 { 0x8000u32 } else { 0 };
                    let px15 = |c: [u8; 3]| -> u32 {
                        (c[0] as u32 >> 3) | ((c[1] as u32 >> 3) << 5) | ((c[2] as u32 >> 3) << 10) | b15
                    };
                    for pair in rgb.chunks(2) {
                        outw.push(px15(pair[0]) | (px15(pair[1]) << 16));
                    }
                }
            }
        } else {
            let mut blk = [0i32; 64];
            while self.decode_block(&mut pos, &qt_y, &mut blk) {
                let lum: Vec<u8> = blk
                    .iter()
                    .map(|&v| {
                        let v = v.clamp(-128, 127);
                        (if self.signed { v } else { v + 128 }) as u8
                    })
                    .collect();
                if self.depth == 1 {
                    for ch in lum.chunks(4) {
                        outw.push(u32::from_le_bytes([ch[0], ch[1], ch[2], ch[3]]));
                    }
                } else {
                    for ch in lum.chunks(8) {
                        let mut w = 0u32;
                        for (i, &b) in ch.iter().enumerate() {
                            w |= ((b >> 4) as u32) << (i * 4);
                        }
                        outw.push(w);
                    }
                }
            }
        }
        words = outw;
        self.out.extend(words);
        self.input.clear();
    }
}
