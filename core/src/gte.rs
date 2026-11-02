//! Geometry Transformation Engine (COP2).

fn unr_table() -> [u8; 257] {
    let mut t = [0u8; 257];
    for (i, e) in t.iter_mut().enumerate() {
        let v = ((0x40000 / (i as i32 + 0x100)) + 1) / 2 - 0x101;
        *e = v.max(0) as u8;
    }
    t
}

pub struct Gte {
    // data registers
    v: [[i16; 3]; 3],
    rgbc: [u8; 4],
    otz: u16,
    ir: [i16; 4],
    sxy: [(i16, i16); 3],
    sz: [u16; 4],
    rgb_fifo: [[u8; 4]; 3],
    res1: u32,
    mac: [i32; 4],
    lzcs: u32,
    lzcr: u32,
    // control registers
    rt: [[i16; 3]; 3],
    tr: [i32; 3],
    llm: [[i16; 3]; 3],
    bk: [i32; 3],
    lcm: [[i16; 3]; 3],
    fc: [i32; 3],
    ofx: i32,
    ofy: i32,
    h: u16,
    dqa: i16,
    dqb: i32,
    zsf3: i16,
    zsf4: i16,
    flag: u32,
    unr: [u8; 257],
    // per-command state
    sf: u32,
    lm: bool,
}

#[inline]
fn lo(v: u32) -> i16 {
    v as i16
}
#[inline]
fn hi(v: u32) -> i16 {
    (v >> 16) as i16
}
#[inline]
fn pack(a: i16, b: i16) -> u32 {
    (a as u16 as u32) | ((b as u16 as u32) << 16)
}

impl Gte {
    pub fn new() -> Self {
        Gte {
            v: [[0; 3]; 3],
            rgbc: [0; 4],
            otz: 0,
            ir: [0; 4],
            sxy: [(0, 0); 3],
            sz: [0; 4],
            rgb_fifo: [[0; 4]; 3],
            res1: 0,
            mac: [0; 4],
            lzcs: 0,
            lzcr: 32,
            rt: [[0; 3]; 3],
            tr: [0; 3],
            llm: [[0; 3]; 3],
            bk: [0; 3],
            lcm: [[0; 3]; 3],
            fc: [0; 3],
            ofx: 0,
            ofy: 0,
            h: 0,
            dqa: 0,
            dqb: 0,
            zsf3: 0,
            zsf4: 0,
            flag: 0,
            unr: unr_table(),
            sf: 0,
            lm: false,
        }
    }

    // ---------------- register access ----------------

    pub fn read_data(&self, r: u32) -> u32 {
        match r {
            0 => pack(self.v[0][0], self.v[0][1]),
            1 => self.v[0][2] as i32 as u32,
            2 => pack(self.v[1][0], self.v[1][1]),
            3 => self.v[1][2] as i32 as u32,
            4 => pack(self.v[2][0], self.v[2][1]),
            5 => self.v[2][2] as i32 as u32,
            6 => u32::from_le_bytes(self.rgbc),
            7 => self.otz as u32,
            8..=11 => self.ir[(r - 8) as usize] as i32 as u32,
            12..=14 => {
                let (x, y) = self.sxy[(r - 12) as usize];
                pack(x, y)
            }
            15 => pack(self.sxy[2].0, self.sxy[2].1),
            16..=19 => self.sz[(r - 16) as usize] as u32,
            20..=22 => u32::from_le_bytes(self.rgb_fifo[(r - 20) as usize]),
            23 => self.res1,
            24..=27 => self.mac[(r - 24) as usize] as u32,
            28 | 29 => {
                let c = |v: i16| ((v as i32) >> 7).clamp(0, 0x1F) as u32;
                c(self.ir[1]) | (c(self.ir[2]) << 5) | (c(self.ir[3]) << 10)
            }
            30 => self.lzcs,
            _ => self.lzcr,
        }
    }

    pub fn write_data(&mut self, r: u32, v: u32) {
        match r {
            0 => self.v[0] = [lo(v), hi(v), self.v[0][2]],
            1 => self.v[0][2] = lo(v),
            2 => self.v[1] = [lo(v), hi(v), self.v[1][2]],
            3 => self.v[1][2] = lo(v),
            4 => self.v[2] = [lo(v), hi(v), self.v[2][2]],
            5 => self.v[2][2] = lo(v),
            6 => self.rgbc = v.to_le_bytes(),
            7 => self.otz = v as u16,
            8..=11 => self.ir[(r - 8) as usize] = lo(v),
            12..=14 => self.sxy[(r - 12) as usize] = (lo(v), hi(v)),
            15 => {
                self.sxy[0] = self.sxy[1];
                self.sxy[1] = self.sxy[2];
                self.sxy[2] = (lo(v), hi(v));
            }
            16..=19 => self.sz[(r - 16) as usize] = v as u16,
            20..=22 => self.rgb_fifo[(r - 20) as usize] = v.to_le_bytes(),
            23 => self.res1 = v,
            24..=27 => self.mac[(r - 24) as usize] = v as i32,
            28 => {
                self.ir[1] = ((v & 0x1F) << 7) as i16;
                self.ir[2] = (((v >> 5) & 0x1F) << 7) as i16;
                self.ir[3] = (((v >> 10) & 0x1F) << 7) as i16;
            }
            29 => {}
            30 => {
                self.lzcs = v;
                self.lzcr = if (v as i32) < 0 { v.leading_ones() } else { v.leading_zeros() };
            }
            _ => {}
        }
    }

    fn mat_read(m: &[[i16; 3]; 3], idx: u32) -> u32 {
        let f: Vec<i16> = m.iter().flatten().copied().collect();
        match idx {
            0 => pack(f[0], f[1]),
            1 => pack(f[2], f[3]),
            2 => pack(f[4], f[5]),
            3 => pack(f[6], f[7]),
            _ => f[8] as i32 as u32,
        }
    }

    fn mat_write(m: &mut [[i16; 3]; 3], idx: u32, v: u32) {
        let mut f: [i16; 9] = [0; 9];
        for (k, e) in m.iter().flatten().enumerate() {
            f[k] = *e;
        }
        match idx {
            0 => {
                f[0] = lo(v);
                f[1] = hi(v)
            }
            1 => {
                f[2] = lo(v);
                f[3] = hi(v)
            }
            2 => {
                f[4] = lo(v);
                f[5] = hi(v)
            }
            3 => {
                f[6] = lo(v);
                f[7] = hi(v)
            }
            _ => f[8] = lo(v),
        }
        for k in 0..9 {
            m[k / 3][k % 3] = f[k];
        }
    }

    pub fn read_ctrl(&self, r: u32) -> u32 {
        match r {
            0..=4 => Self::mat_read(&self.rt, r),
            5..=7 => self.tr[(r - 5) as usize] as u32,
            8..=12 => Self::mat_read(&self.llm, r - 8),
            13..=15 => self.bk[(r - 13) as usize] as u32,
            16..=20 => Self::mat_read(&self.lcm, r - 16),
            21..=23 => self.fc[(r - 21) as usize] as u32,
            24 => self.ofx as u32,
            25 => self.ofy as u32,
            26 => self.h as i16 as i32 as u32, // hardware sign-extends on read
            27 => self.dqa as i32 as u32,
            28 => self.dqb as u32,
            29 => self.zsf3 as i32 as u32,
            30 => self.zsf4 as i32 as u32,
            _ => self.flag,
        }
    }

    pub fn write_ctrl(&mut self, r: u32, v: u32) {
        match r {
            0..=4 => Self::mat_write(&mut self.rt, r, v),
            5..=7 => self.tr[(r - 5) as usize] = v as i32,
            8..=12 => Self::mat_write(&mut self.llm, r - 8, v),
            13..=15 => self.bk[(r - 13) as usize] = v as i32,
            16..=20 => Self::mat_write(&mut self.lcm, r - 16, v),
            21..=23 => self.fc[(r - 21) as usize] = v as i32,
            24 => self.ofx = v as i32,
            25 => self.ofy = v as i32,
            26 => self.h = v as u16,
            27 => self.dqa = v as i16,
            28 => self.dqb = v as i32,
            29 => self.zsf3 = v as i16,
            30 => self.zsf4 = v as i16,
            _ => {
                self.flag = v & 0x7FFF_F000;
                self.update_flag_summary();
            }
        }
    }

    fn update_flag_summary(&mut self) {
        if self.flag & 0x7F87_E000 != 0 {
            self.flag |= 0x8000_0000;
        } else {
            self.flag &= 0x7FFF_FFFF;
        }
    }

    // ---------------- arithmetic helpers ----------------

    /// Check 44-bit overflow for MAC1..3 and return the value shifted by sf.
    fn mac_set(&mut self, i: usize, v: i64) -> i64 {
        self.check_mac(i, v);
        let r = v >> (self.sf * 12);
        self.mac[i] = r as i32;
        r
    }

    fn check_mac(&mut self, i: usize, v: i64) {
        if v > 0x7FF_FFFF_FFFF {
            self.flag |= 1 << (31 - i);
        } else if v < -0x800_0000_0000 {
            self.flag |= 1 << (28 - i);
        }
    }

    fn mac0_set(&mut self, v: i64) -> i64 {
        if v > 0x7FFF_FFFF {
            self.flag |= 1 << 16;
        } else if v < -0x8000_0000 {
            self.flag |= 1 << 15;
        }
        self.mac[0] = v as i32;
        v
    }

    fn ir_set(&mut self, i: usize, v: i64, lm: bool) {
        let min = if lm { 0 } else { -0x8000 };
        let r = if v < min {
            self.flag |= 1 << (25 - i);
            min
        } else if v > 0x7FFF {
            self.flag |= 1 << (25 - i);
            0x7FFF
        } else {
            v
        };
        self.ir[i] = r as i16;
    }

    fn ir0_set(&mut self, v: i64) {
        let r = if v < 0 {
            self.flag |= 1 << 12;
            0
        } else if v > 0x1000 {
            self.flag |= 1 << 12;
            0x1000
        } else {
            v
        };
        self.ir[0] = r as i16;
    }

    fn mac_ir(&mut self, i: usize, v: i64, lm: bool) {
        let m = self.mac_set(i, v);
        self.ir_set(i, m, lm);
    }

    fn push_sz(&mut self, v: i64) {
        let r = if v < 0 {
            self.flag |= 1 << 18;
            0
        } else if v > 0xFFFF {
            self.flag |= 1 << 18;
            0xFFFF
        } else {
            v
        };
        self.sz[0] = self.sz[1];
        self.sz[1] = self.sz[2];
        self.sz[2] = self.sz[3];
        self.sz[3] = r as u16;
    }

    fn push_sxy(&mut self, x: i64, y: i64) {
        let cx = |s: &mut Self, v: i64, bit: u32| -> i16 {
            if v < -0x400 {
                s.flag |= 1 << bit;
                -0x400
            } else if v > 0x3FF {
                s.flag |= 1 << bit;
                0x3FF
            } else {
                v as i16
            }
        };
        let sx = cx(self, x, 14);
        let sy = cx(self, y, 13);
        self.sxy[0] = self.sxy[1];
        self.sxy[1] = self.sxy[2];
        self.sxy[2] = (sx, sy);
    }

    fn push_color(&mut self) {
        let c = |s: &mut Self, v: i32, bit: u32| -> u8 {
            let v = v >> 4;
            if v < 0 {
                s.flag |= 1 << bit;
                0
            } else if v > 0xFF {
                s.flag |= 1 << bit;
                0xFF
            } else {
                v as u8
            }
        };
        let r = c(self, self.mac[1], 21);
        let g = c(self, self.mac[2], 20);
        let b = c(self, self.mac[3], 19);
        self.rgb_fifo[0] = self.rgb_fifo[1];
        self.rgb_fifo[1] = self.rgb_fifo[2];
        self.rgb_fifo[2] = [r, g, b, self.rgbc[3]];
    }

    fn divide(&mut self) -> u32 {
        let n = self.h as u32;
        let d = self.sz[3] as u32;
        if n < d * 2 {
            let shift = (d as u16).leading_zeros();
            let n = n << shift;
            let d = d << shift;
            let u = self.unr[(((d & 0x7FFF) + 0x40) >> 7) as usize] as i32 + 0x101;
            let d2 = ((0x0200_0080 - (d as i32) * u) >> 8) as i32;
            let recip = ((u * d2 + 0x80) >> 8) as u32;
            let r = ((n as u64 * recip as u64 + 0x8000) >> 16) as u32;
            r.min(0x1FFFF)
        } else {
            self.flag |= 1 << 17;
            0x1FFFF
        }
    }

    // ---------------- commands ----------------

    pub fn command(&mut self, instr: u32) {
        self.flag = 0;
        self.sf = (instr >> 19) & 1;
        self.lm = instr & (1 << 10) != 0;
        match instr & 0x3F {
            0x01 => self.rtp(0, true),
            0x06 => self.nclip(),
            0x0C => self.op(),
            0x10 => self.dpcs(false),
            0x11 => self.intpl(),
            0x12 => self.mvmva(instr),
            0x13 => self.ncd(0),
            0x14 => self.cdp(),
            0x16 => {
                self.ncd(0);
                self.ncd(1);
                self.ncd(2)
            }
            0x1B => self.ncc(0),
            0x1C => self.cc(),
            0x1E => self.nc(0),
            0x20 => {
                self.nc(0);
                self.nc(1);
                self.nc(2)
            }
            0x28 => self.sqr(),
            0x29 => self.dcpl(),
            0x2A => {
                self.dpcs(true);
                self.dpcs(true);
                self.dpcs(true)
            }
            0x2D => self.avsz3(),
            0x2E => self.avsz4(),
            0x30 => {
                self.rtp(0, false);
                self.rtp(1, false);
                self.rtp(2, true)
            }
            0x3D => self.gpf(),
            0x3E => self.gpl(),
            0x3F => {
                self.ncc(0);
                self.ncc(1);
                self.ncc(2)
            }
            _ => {}
        }
        self.update_flag_summary();
    }

    fn rtp(&mut self, vi: usize, last: bool) {
        let v = self.v[vi];
        let lm = self.lm;
        let mut z_full = 0i64;
        for i in 0..3 {
            let r = self.rt[i];
            let mut acc = (self.tr[i] as i64) << 12;
            acc += r[0] as i64 * v[0] as i64;
            self.check_mac(i + 1, acc);
            acc += r[1] as i64 * v[1] as i64;
            self.check_mac(i + 1, acc);
            acc += r[2] as i64 * v[2] as i64;
            let m = self.mac_set(i + 1, acc);
            if i == 2 {
                z_full = acc;
                // IR3 saturation flag is computed from MAC3>>12 regardless of sf.
                let clamp_min = if lm { 0 } else { -0x8000 };
                let m12 = acc >> 12;
                let r = m.clamp(clamp_min, 0x7FFF);
                if m12 < -0x8000 || m12 > 0x7FFF {
                    self.flag |= 1 << 22;
                }
                self.ir[3] = r as i16;
            } else {
                self.ir_set(i + 1, m, lm);
            }
        }
        self.push_sz(z_full >> 12);
        let hd = self.divide() as i64;
        let x = self.mac0_set(hd * self.ir[1] as i64 + self.ofx as i64);
        let y = self.mac0_set(hd * self.ir[2] as i64 + self.ofy as i64);
        self.push_sxy(x >> 16, y >> 16);
        if last {
            let p = self.mac0_set(hd * self.dqa as i64 + self.dqb as i64);
            self.ir0_set(p >> 12);
        }
    }

    fn nclip(&mut self) {
        let [(x0, y0), (x1, y1), (x2, y2)] = self.sxy;
        let (x0, y0, x1, y1, x2, y2) =
            (x0 as i64, y0 as i64, x1 as i64, y1 as i64, x2 as i64, y2 as i64);
        let v = x0 * y1 + x1 * y2 + x2 * y0 - x0 * y2 - x1 * y0 - x2 * y1;
        self.mac0_set(v);
    }

    fn op(&mut self) {
        let d = [self.rt[0][0] as i64, self.rt[1][1] as i64, self.rt[2][2] as i64];
        let ir = [self.ir[1] as i64, self.ir[2] as i64, self.ir[3] as i64];
        let lm = self.lm;
        self.mac_ir(1, ir[2] * d[1] - ir[1] * d[2], lm);
        self.mac_ir(2, ir[0] * d[2] - ir[2] * d[0], lm);
        self.mac_ir(3, ir[1] * d[0] - ir[0] * d[1], lm);
    }

    fn sqr(&mut self) {
        let lm = self.lm;
        for i in 1..4 {
            let v = self.ir[i] as i64;
            self.mac_ir(i, v * v, lm);
        }
    }

    fn avsz3(&mut self) {
        let s = self.sz[1] as i64 + self.sz[2] as i64 + self.sz[3] as i64;
        let v = self.mac0_set(self.zsf3 as i64 * s);
        self.set_otz(v >> 12);
    }

    fn avsz4(&mut self) {
        let s = self.sz.iter().map(|&z| z as i64).sum::<i64>();
        let v = self.mac0_set(self.zsf4 as i64 * s);
        self.set_otz(v >> 12);
    }

    fn set_otz(&mut self, v: i64) {
        self.otz = if v < 0 {
            self.flag |= 1 << 18;
            0
        } else if v > 0xFFFF {
            self.flag |= 1 << 18;
            0xFFFF
        } else {
            v as u16
        };
    }

    /// Generic matrix * vector + translation (with flag checks between partial sums).
    fn mat_vec(&mut self, m: &[[i16; 3]; 3], v: [i64; 3], t: [i64; 3], lm: bool) {
        for i in 0..3 {
            let mut acc = t[i] << 12;
            acc += m[i][0] as i64 * v[0];
            self.check_mac(i + 1, acc);
            acc += m[i][1] as i64 * v[1];
            self.check_mac(i + 1, acc);
            acc += m[i][2] as i64 * v[2];
            self.mac_ir(i + 1, acc, lm);
        }
    }

    fn mvmva(&mut self, instr: u32) {
        let mx = (instr >> 17) & 3;
        let vsel = (instr >> 15) & 3;
        let cv = (instr >> 13) & 3;
        let lm = self.lm;
        let m: [[i16; 3]; 3] = match mx {
            0 => self.rt,
            1 => self.llm,
            2 => self.lcm,
            _ => {
                let r = (self.rgbc[0] as i16) << 4;
                [
                    [-r, r, self.ir[0]],
                    [self.rt[0][2]; 3],
                    [self.rt[1][1]; 3],
                ]
            }
        };
        let v: [i64; 3] = match vsel {
            0..=2 => {
                let v = self.v[vsel as usize];
                [v[0] as i64, v[1] as i64, v[2] as i64]
            }
            _ => [self.ir[1] as i64, self.ir[2] as i64, self.ir[3] as i64],
        };
        let t: [i64; 3] = match cv {
            0 => [self.tr[0] as i64, self.tr[1] as i64, self.tr[2] as i64],
            1 => [self.bk[0] as i64, self.bk[1] as i64, self.bk[2] as i64],
            2 => [self.fc[0] as i64, self.fc[1] as i64, self.fc[2] as i64],
            _ => [0; 3],
        };
        if cv == 2 {
            // Hardware bug: FC translation only affects flags; result omits first column.
            for i in 0..3 {
                let first = (t[i] << 12) + m[i][0] as i64 * v[0];
                self.check_mac(i + 1, first);
                let f = first >> (self.sf * 12);
                self.ir_set(i + 1, f, false);
                let acc = m[i][1] as i64 * v[1] + m[i][2] as i64 * v[2];
                self.check_mac(i + 1, acc);
                self.mac_ir(i + 1, acc, lm);
            }
        } else {
            self.mat_vec(&m, v, t, lm);
        }
    }

    fn light(&mut self, vi: usize) {
        let lm = self.lm;
        let v = self.v[vi];
        let llm = self.llm;
        self.mat_vec(&llm, [v[0] as i64, v[1] as i64, v[2] as i64], [0; 3], lm);
        self.bk_lcm();
    }

    fn bk_lcm(&mut self) {
        let lm = self.lm;
        let lcm = self.lcm;
        let ir = [self.ir[1] as i64, self.ir[2] as i64, self.ir[3] as i64];
        let bk = [self.bk[0] as i64, self.bk[1] as i64, self.bk[2] as i64];
        self.mat_vec(&lcm, ir, bk, lm);
    }

    /// MAC = in + (FC - in) * IR0, followed by color FIFO push.
    fn interpolate(&mut self, inp: [i64; 3]) {
        let lm = self.lm;
        for i in 0..3 {
            let d = ((self.fc[i] as i64) << 12) - inp[i];
            let m = self.mac_set(i + 1, d);
            self.ir_set(i + 1, m, false);
        }
        for i in 0..3 {
            let v = self.ir[i + 1] as i64 * self.ir[0] as i64 + inp[i];
            self.mac_ir(i + 1, v, lm);
        }
        self.push_color();
    }

    fn color_mul(&self) -> [i64; 3] {
        [
            ((self.rgbc[0] as i64) * self.ir[1] as i64) << 4,
            ((self.rgbc[1] as i64) * self.ir[2] as i64) << 4,
            ((self.rgbc[2] as i64) * self.ir[3] as i64) << 4,
        ]
    }

    fn nc(&mut self, vi: usize) {
        self.light(vi);
        self.push_color();
    }

    fn ncc(&mut self, vi: usize) {
        self.light(vi);
        self.color_only();
    }

    fn color_only(&mut self) {
        let lm = self.lm;
        let c = self.color_mul();
        for i in 0..3 {
            self.mac_ir(i + 1, c[i], lm);
        }
        self.push_color();
    }

    fn ncd(&mut self, vi: usize) {
        self.light(vi);
        let c = self.color_mul();
        self.interpolate(c);
    }

    fn cc(&mut self) {
        self.bk_lcm();
        self.color_only();
    }

    fn cdp(&mut self) {
        self.bk_lcm();
        let c = self.color_mul();
        self.interpolate(c);
    }

    fn dcpl(&mut self) {
        let c = self.color_mul();
        self.interpolate(c);
    }

    fn dpcs(&mut self, use_fifo: bool) {
        let src = if use_fifo { self.rgb_fifo[0] } else { self.rgbc };
        let inp = [(src[0] as i64) << 16, (src[1] as i64) << 16, (src[2] as i64) << 16];
        self.interpolate(inp);
    }

    fn intpl(&mut self) {
        let inp = [
            (self.ir[1] as i64) << 12,
            (self.ir[2] as i64) << 12,
            (self.ir[3] as i64) << 12,
        ];
        self.interpolate(inp);
    }

    fn gpf(&mut self) {
        let lm = self.lm;
        let ir0 = self.ir[0] as i64;
        for i in 1..4 {
            let v = self.ir[i] as i64 * ir0;
            self.mac_ir(i, v, lm);
        }
        self.push_color();
    }

    fn gpl(&mut self) {
        let lm = self.lm;
        let ir0 = self.ir[0] as i64;
        let sh = self.sf * 12;
        for i in 1..4 {
            let v = ((self.mac[i] as i64) << sh) + self.ir[i] as i64 * ir0;
            self.mac_ir(i, v, lm);
        }
        self.push_color();
    }
}
