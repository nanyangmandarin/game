//! Sound Processing Unit: 24 ADPCM voices, ADSR, noise, pitch modulation, reverb.

use serde::{Deserialize, Serialize};

use crate::cdrom::CdRom;

const SPU_RAM: usize = 512 * 1024;
const CYCLES_PER_SAMPLE: u32 = 768;

const POS: [i32; 5] = [0, 60, 115, 98, 122];
const NEG: [i32; 5] = [0, 0, -52, -55, -60];

#[derive(Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
enum Phase {
    #[default]
    Off,
    Attack,
    Decay,
    Sustain,
    Release,
}

#[derive(Clone, Copy, Default, Serialize, Deserialize)]
struct Voice {
    vol_l: u16,
    vol_r: u16,
    pitch: u16,
    start: u16,
    adsr_lo: u16,
    adsr_hi: u16,
    level: i32,
    repeat: u16,
    // runtime
    addr: u32,
    counter: u32,
    buf: [i16; 28],
    prev: i16,
    hist: [i32; 2],
    flags: u8,
    phase: Phase,
    env_wait: u32,
    out: i32,
    repeat_set: bool,
}

#[derive(Serialize, Deserialize)]
pub struct Spu {
    ram: Vec<u8>,
    v: [Voice; 24],
    main_l: u16,
    main_r: u16,
    rev_vol_l: i16,
    rev_vol_r: i16,
    pmon: u32,
    non: u32,
    eon: u32,
    endx: u32,
    rev_base: u32,
    rev_cur: u32,
    irq_addr: u16,
    xfer_addr: u32,
    cnt: u16,
    xfer_ctrl: u16,
    stat_irq: bool,
    cd_vol_l: i16,
    cd_vol_r: i16,
    ext_l: u16,
    ext_r: u16,
    rev: [u16; 32],
    regs_raw: Vec<u16>,
    cyc: u32,
    noise_timer: i32,
    noise_level: u16,
    rev_phase: bool,
    rev_in: (i32, i32),
    rev_out: (i32, i32),
    #[serde(skip)]
    pub out: Vec<i16>,
    pub master_volume: f32,
    pub reverb_enabled: bool,
    pub interp: bool,
    irq_pending: bool,
}

#[inline]
fn vol_fixed(v: u16) -> i32 {
    if v & 0x8000 == 0 {
        ((v << 1) as i16) as i32
    } else {
        // sweep mode: approximate with the maximum of the configured direction
        if v & 0x1000 != 0 { -0x7FFF } else { 0x7FFF }
    }
}

impl Spu {
    pub fn new() -> Self {
        Spu {
            ram: vec![0; SPU_RAM],
            v: [Voice::default(); 24],
            main_l: 0,
            main_r: 0,
            rev_vol_l: 0,
            rev_vol_r: 0,
            pmon: 0,
            non: 0,
            eon: 0,
            endx: 0,
            rev_base: 0,
            rev_cur: 0,
            irq_addr: 0,
            xfer_addr: 0,
            cnt: 0,
            xfer_ctrl: 0,
            stat_irq: false,
            cd_vol_l: 0,
            cd_vol_r: 0,
            ext_l: 0,
            ext_r: 0,
            rev: [0; 32],
            regs_raw: vec![0; 0x200],
            cyc: 0,
            noise_timer: 0,
            noise_level: 1,
            rev_phase: false,
            rev_in: (0, 0),
            rev_out: (0, 0),
            out: Vec::with_capacity(4096),
            master_volume: 1.0,
            reverb_enabled: true,
            interp: true,
            irq_pending: false,
        }
    }

    pub fn reset(&mut self) {
        let (mv, re, ip) = (self.master_volume, self.reverb_enabled, self.interp);
        *self = Spu::new();
        self.master_volume = mv;
        self.reverb_enabled = re;
        self.interp = ip;
    }

    // ---------------- registers ----------------

    pub fn read16(&mut self, off: u32) -> u16 {
        let r = off - 0xC00;
        if r < 0x180 {
            let n = (r >> 4) as usize;
            let v = &self.v[n];
            return match r & 0xF {
                0x0 => v.vol_l,
                0x2 => v.vol_r,
                0x4 => v.pitch,
                0x6 => v.start,
                0x8 => v.adsr_lo,
                0xA => v.adsr_hi,
                0xC => v.level as u16,
                _ => v.repeat,
            };
        }
        match off {
            0xD80 => self.main_l,
            0xD82 => self.main_r,
            0xD84 => self.rev_vol_l as u16,
            0xD86 => self.rev_vol_r as u16,
            0xD9C => self.endx as u16,
            0xD9E => (self.endx >> 16) as u16,
            0xDA2 => (self.rev_base / 8) as u16,
            0xDA4 => self.irq_addr,
            0xDA6 => (self.xfer_addr / 8) as u16,
            0xDAA => self.cnt,
            0xDAC => self.xfer_ctrl,
            0xDAE => {
                let mode = (self.cnt >> 4) & 3;
                (self.cnt & 0x3F) | ((self.stat_irq as u16) << 6) | (((mode & 2) as u16) << 6)
            }
            0xDB0 => self.cd_vol_l as u16,
            0xDB2 => self.cd_vol_r as u16,
            0xDB4 => self.ext_l,
            0xDB6 => self.ext_r,
            0xDB8 => self.main_l & 0x7FFF,
            0xDBA => self.main_r & 0x7FFF,
            0xDC0..=0xDFF => self.rev[((off - 0xDC0) / 2) as usize],
            0xE00..=0xE5F => {
                let n = ((off - 0xE00) / 4) as usize;
                self.v[n].out as u16
            }
            _ => self.regs_raw[((off - 0xC00) / 2) as usize & 0x1FF],
        }
    }

    pub fn write16(&mut self, off: u32, val: u16) {
        self.regs_raw[((off - 0xC00) / 2) as usize & 0x1FF] = val;
        let r = off - 0xC00;
        if r < 0x180 {
            let n = (r >> 4) as usize;
            let v = &mut self.v[n];
            match r & 0xF {
                0x0 => v.vol_l = val,
                0x2 => v.vol_r = val,
                0x4 => v.pitch = val,
                0x6 => v.start = val,
                0x8 => v.adsr_lo = val,
                0xA => v.adsr_hi = val,
                0xC => v.level = val as i16 as i32,
                _ => {
                    v.repeat = val;
                    v.repeat_set = true;
                }
            }
            return;
        }
        match off {
            0xD80 => self.main_l = val,
            0xD82 => self.main_r = val,
            0xD84 => self.rev_vol_l = val as i16,
            0xD86 => self.rev_vol_r = val as i16,
            0xD88 => self.key_on(val as u32),
            0xD8A => self.key_on((val as u32) << 16),
            0xD8C => self.key_off(val as u32),
            0xD8E => self.key_off((val as u32) << 16),
            0xD90 => self.pmon = (self.pmon & 0xFFFF_0000) | val as u32,
            0xD92 => self.pmon = (self.pmon & 0xFFFF) | ((val as u32) << 16),
            0xD94 => self.non = (self.non & 0xFFFF_0000) | val as u32,
            0xD96 => self.non = (self.non & 0xFFFF) | ((val as u32) << 16),
            0xD98 => self.eon = (self.eon & 0xFFFF_0000) | val as u32,
            0xD9A => self.eon = (self.eon & 0xFFFF) | ((val as u32) << 16),
            0xDA2 => {
                self.rev_base = (val as u32) * 8;
                self.rev_cur = self.rev_base;
            }
            0xDA4 => self.irq_addr = val,
            0xDA6 => self.xfer_addr = (val as u32) * 8,
            0xDA8 => self.fifo_write(val),
            0xDAA => {
                self.cnt = val;
                if val & 0x40 == 0 {
                    self.stat_irq = false;
                }
            }
            0xDAC => self.xfer_ctrl = val,
            0xDB0 => self.cd_vol_l = val as i16,
            0xDB2 => self.cd_vol_r = val as i16,
            0xDB4 => self.ext_l = val,
            0xDB6 => self.ext_r = val,
            0xDC0..=0xDFF => self.rev[((off - 0xDC0) / 2) as usize] = val,
            _ => {}
        }
    }

    fn check_irq(&mut self, addr: u32) {
        if self.cnt & 0x40 != 0 && !self.stat_irq && (addr & !7) == (self.irq_addr as u32) * 8 {
            self.stat_irq = true;
            self.irq_pending = true;
        }
    }

    fn fifo_write(&mut self, val: u16) {
        let a = (self.xfer_addr as usize) & (SPU_RAM - 2);
        self.check_irq(a as u32);
        self.ram[a..a + 2].copy_from_slice(&val.to_le_bytes());
        self.xfer_addr = (self.xfer_addr + 2) & (SPU_RAM as u32 - 1);
    }

    pub fn dma_write(&mut self, w: u32) {
        self.fifo_write(w as u16);
        self.fifo_write((w >> 16) as u16);
    }

    pub fn dma_read(&mut self) -> u32 {
        let mut r = 0u32;
        for k in 0..2 {
            let a = (self.xfer_addr as usize) & (SPU_RAM - 2);
            let h = u16::from_le_bytes([self.ram[a], self.ram[a + 1]]) as u32;
            r |= h << (16 * k);
            self.xfer_addr = (self.xfer_addr + 2) & (SPU_RAM as u32 - 1);
        }
        r
    }

    fn key_on(&mut self, bits: u32) {
        for n in 0..24 {
            if bits & (1 << n) != 0 {
                let v = &mut self.v[n];
                v.addr = (v.start as u32) * 8;
                v.counter = 0;
                v.hist = [0, 0];
                v.prev = 0;
                v.phase = Phase::Attack;
                v.level = 0;
                v.env_wait = 0;
                v.repeat_set = false;
                self.endx &= !(1 << n);
                self.decode_block(n);
            }
        }
    }

    fn key_off(&mut self, bits: u32) {
        for n in 0..24 {
            if bits & (1 << n) != 0 && self.v[n].phase != Phase::Off {
                self.v[n].phase = Phase::Release;
                self.v[n].env_wait = 0;
            }
        }
    }

    fn decode_block(&mut self, n: usize) {
        let addr = (self.v[n].addr as usize) & (SPU_RAM - 16);
        self.check_irq(addr as u32);
        let hdr = self.ram[addr];
        let flags = self.ram[addr + 1];
        let mut shift = (hdr & 0xF) as i32;
        if shift > 12 {
            shift = 9;
        }
        let filter = ((hdr >> 4) as usize).min(4);
        let v = &mut self.v[n];
        if flags & 4 != 0 && !v.repeat_set {
            v.repeat = (addr / 8) as u16;
        }
        v.flags = flags;
        let (mut h0, mut h1) = (v.hist[0], v.hist[1]);
        for i in 0..28 {
            let b = self.ram[addr + 2 + i / 2];
            let nib = if i & 1 != 0 { b >> 4 } else { b & 0xF };
            let mut s = ((((nib as u16) << 12) as i16) as i32) >> shift;
            s += (h0 * POS[filter] + h1 * NEG[filter] + 32) >> 6;
            let s = s.clamp(-32768, 32767);
            h1 = h0;
            h0 = s;
            v.buf[i] = s as i16;
        }
        v.hist = [h0, h1];
    }

    // ---------------- envelope ----------------

    fn env_step(v: &mut Voice, shift: u32, step: i32, exp: bool, dec: bool) {
        if v.env_wait > 0 {
            v.env_wait -= 1;
            return;
        }
        let mut cycles = 1u32 << shift.saturating_sub(11);
        let mut st = step << 11u32.saturating_sub(shift);
        if exp && !dec && v.level > 0x6000 {
            cycles *= 4;
        }
        if exp && dec {
            st = (st * v.level) >> 15;
        }
        v.env_wait = cycles - 1;
        v.level = (v.level + st).clamp(0, 0x7FFF);
    }

    fn envelope(v: &mut Voice) {
        let lo = v.adsr_lo as u32;
        let hi = v.adsr_hi as u32;
        match v.phase {
            Phase::Off => v.level = 0,
            Phase::Attack => {
                Self::env_step(v, (lo >> 10) & 0x1F, 7 - ((lo >> 8) & 3) as i32, lo & 0x8000 != 0, false);
                if v.level >= 0x7FFF {
                    v.phase = Phase::Decay;
                    v.env_wait = 0;
                }
            }
            Phase::Decay => {
                Self::env_step(v, (lo >> 4) & 0xF, -8, true, true);
                let sl = (((lo & 0xF) + 1) * 0x800).min(0x7FFF) as i32;
                if v.level <= sl {
                    v.phase = Phase::Sustain;
                    v.env_wait = 0;
                }
            }
            Phase::Sustain => {
                let dec = hi & 0x4000 != 0;
                let s = ((hi >> 6) & 3) as i32;
                let step = if dec { -8 + s } else { 7 - s };
                Self::env_step(v, (hi >> 8) & 0x1F, step, hi & 0x8000 != 0, dec);
            }
            Phase::Release => {
                Self::env_step(v, hi & 0x1F, -8, hi & 0x20 != 0, true);
                if v.level <= 0 {
                    v.phase = Phase::Off;
                }
            }
        }
    }

    // ---------------- sample generation ----------------

    fn noise_tick(&mut self) {
        let step = ((self.cnt >> 8) & 3) as i32 + 4;
        let shift = ((self.cnt >> 10) & 0xF) as u32;
        self.noise_timer -= step;
        let n = self.noise_level;
        let parity = ((n >> 15) ^ (n >> 12) ^ (n >> 11) ^ (n >> 10) ^ 1) & 1;
        if self.noise_timer < 0 {
            self.noise_level = (n << 1) | parity;
            self.noise_timer += (0x20000 >> shift) as i32;
            if self.noise_timer < 0 {
                self.noise_timer += (0x20000 >> shift) as i32;
            }
        }
    }

    fn generate(&mut self, cd: &mut CdRom) {
        self.noise_tick();
        let mut sum_l = 0i32;
        let mut sum_r = 0i32;
        let mut rev_l = 0i32;
        let mut rev_r = 0i32;
        let mut prev_out = 0i32;
        for n in 0..24 {
            if self.v[n].phase == Phase::Off && self.v[n].level == 0 {
                self.v[n].out = 0;
                prev_out = 0;
                continue;
            }
            let (idx, frac, prev, cur) = {
                let v = &self.v[n];
                let idx = (v.counter >> 12) as usize;
                let cur = v.buf[idx.min(27)] as i32;
                let prev = if idx == 0 { v.prev as i32 } else { v.buf[idx - 1] as i32 };
                (idx, (v.counter & 0xFFF) as i32, prev, cur)
            };
            let _ = idx;
            let mut sample = if self.interp { prev + (((cur - prev) * frac) >> 12) } else { cur };
            if self.non & (1 << n) != 0 {
                sample = self.noise_level as i16 as i32;
            }
            {
                let v = &mut self.v[n];
                Self::envelope(v);
                sample = (sample * v.level) >> 15;
                v.out = sample;
            }

            let vl = vol_fixed(self.v[n].vol_l);
            let vr = vol_fixed(self.v[n].vol_r);
            let l = (sample * vl) >> 15;
            let r = (sample * vr) >> 15;
            sum_l += l;
            sum_r += r;
            if self.eon & (1 << n) != 0 {
                rev_l += l;
                rev_r += r;
            }

            // advance pitch counter
            let mut step = self.v[n].pitch as i32;
            if n > 0 && self.pmon & (1 << n) != 0 {
                let f = prev_out + 0x8000;
                step = (step * f) >> 15;
            }
            let step = step.clamp(0, 0x3FFF) as u32;
            prev_out = self.v[n].out;
            self.v[n].counter += step;
            while self.v[n].counter >= 28 << 12 {
                self.v[n].counter -= 28 << 12;
                let last = self.v[n].buf[27];
                self.v[n].prev = last;
                let flags = self.v[n].flags;
                if flags & 1 != 0 {
                    self.endx |= 1 << n;
                    if flags & 2 != 0 {
                        self.v[n].addr = (self.v[n].repeat as u32) * 8;
                    } else {
                        self.v[n].phase = Phase::Off;
                        self.v[n].level = 0;
                        self.v[n].addr = (self.v[n].repeat as u32) * 8;
                    }
                } else {
                    self.v[n].addr += 16;
                }
                self.decode_block(n);
            }
        }

        // CD audio
        let (cdl, cdr) = if self.cnt & 1 != 0 {
            let (l, r) = cd.audio.pop_front().unwrap_or((0, 0));
            ((l as i32 * self.cd_vol_l as i32) >> 15, (r as i32 * self.cd_vol_r as i32) >> 15)
        } else {
            cd.audio.pop_front();
            (0, 0)
        };
        sum_l += cdl;
        sum_r += cdr;
        if self.cnt & 4 != 0 {
            rev_l += cdl;
            rev_r += cdr;
        }

        // reverb at 22.05 kHz
        if self.reverb_enabled && self.cnt & 0x80 != 0 {
            self.rev_phase = !self.rev_phase;
            if self.rev_phase {
                self.rev_in = (rev_l.clamp(-32768, 32767), rev_r.clamp(-32768, 32767));
                self.rev_out = self.reverb(self.rev_in.0, self.rev_in.1);
            }
            sum_l += self.rev_out.0;
            sum_r += self.rev_out.1;
        }

        if self.cnt & 0x4000 == 0 {
            // muted
            sum_l = 0;
            sum_r = 0;
        }
        let ml = vol_fixed(self.main_l);
        let mr = vol_fixed(self.main_r);
        let mut l = ((sum_l.clamp(-32768, 32767) * ml) >> 15) as f32 * self.master_volume;
        let mut r = ((sum_r.clamp(-32768, 32767) * mr) >> 15) as f32 * self.master_volume;
        l = l.clamp(-32768.0, 32767.0);
        r = r.clamp(-32768.0, 32767.0);
        if self.out.len() < 44100 {
            self.out.push(l as i16);
            self.out.push(r as i16);
        }
    }

    // ---------------- reverb ----------------

    fn rev_addr(&self, off: u32) -> usize {
        let base = self.rev_base;
        let size = (SPU_RAM as u32).saturating_sub(base).max(2);
        let rel = (self.rev_cur - base + off) % size;
        ((base + rel) as usize) & (SPU_RAM - 2)
    }

    fn rr(&self, off: u32) -> i32 {
        let a = self.rev_addr(off);
        i16::from_le_bytes([self.ram[a], self.ram[a + 1]]) as i32
    }

    fn rw(&mut self, off: u32, v: i32) {
        if self.cnt & 0x80 == 0 {
            return;
        }
        let a = self.rev_addr(off);
        let v = v.clamp(-32768, 32767) as i16;
        self.ram[a..a + 2].copy_from_slice(&v.to_le_bytes());
    }

    fn reverb(&mut self, lin: i32, rin: i32) -> (i32, i32) {
        let g = |i: usize| self.rev[i] as i16 as i32;
        let a = |s: &Self, i: usize| (s.rev[i] as u32) * 8;
        let m = |x: i32, y: i32| (x * y) >> 15;
        let (d_apf1, d_apf2) = (a(self, 0), a(self, 1));
        let (v_iir, v_comb1, v_comb2, v_comb3, v_comb4, v_wall, v_apf1, v_apf2) =
            (g(2), g(3), g(4), g(5), g(6), g(7), g(8), g(9));
        let (m_lsame, m_rsame, m_lcomb1, m_rcomb1, m_lcomb2, m_rcomb2) =
            (a(self, 10), a(self, 11), a(self, 12), a(self, 13), a(self, 14), a(self, 15));
        let (d_lsame, d_rsame, m_ldiff, m_rdiff, m_lcomb3, m_rcomb3, m_lcomb4, m_rcomb4) =
            (a(self, 16), a(self, 17), a(self, 18), a(self, 19), a(self, 20), a(self, 21), a(self, 22), a(self, 23));
        let (d_ldiff, d_rdiff, m_lapf1, m_rapf1, m_lapf2, m_rapf2) =
            (a(self, 24), a(self, 25), a(self, 26), a(self, 27), a(self, 28), a(self, 29));
        let (v_lin, v_rin) = (g(30), g(31));

        let li = m(lin, v_lin);
        let ri = m(rin, v_rin);
        let sub2 = |x: u32| x.wrapping_sub(2);

        let ls = m(li + m(self.rr(d_lsame), v_wall) - self.rr(sub2(m_lsame)), v_iir) + self.rr(sub2(m_lsame));
        let rs = m(ri + m(self.rr(d_rsame), v_wall) - self.rr(sub2(m_rsame)), v_iir) + self.rr(sub2(m_rsame));
        let ld = m(li + m(self.rr(d_rdiff), v_wall) - self.rr(sub2(m_ldiff)), v_iir) + self.rr(sub2(m_ldiff));
        let rd = m(ri + m(self.rr(d_ldiff), v_wall) - self.rr(sub2(m_rdiff)), v_iir) + self.rr(sub2(m_rdiff));
        self.rw(m_lsame, ls);
        self.rw(m_rsame, rs);
        self.rw(m_ldiff, ld);
        self.rw(m_rdiff, rd);

        let mut lo = m(v_comb1, self.rr(m_lcomb1)) + m(v_comb2, self.rr(m_lcomb2)) + m(v_comb3, self.rr(m_lcomb3)) + m(v_comb4, self.rr(m_lcomb4));
        let mut ro = m(v_comb1, self.rr(m_rcomb1)) + m(v_comb2, self.rr(m_rcomb2)) + m(v_comb3, self.rr(m_rcomb3)) + m(v_comb4, self.rr(m_rcomb4));

        let t = self.rr(m_lapf1.wrapping_sub(d_apf1));
        lo -= m(v_apf1, t);
        self.rw(m_lapf1, lo);
        lo = m(lo, v_apf1) + t;
        let t = self.rr(m_rapf1.wrapping_sub(d_apf1));
        ro -= m(v_apf1, t);
        self.rw(m_rapf1, ro);
        ro = m(ro, v_apf1) + t;

        let t = self.rr(m_lapf2.wrapping_sub(d_apf2));
        lo -= m(v_apf2, t);
        self.rw(m_lapf2, lo);
        lo = m(lo, v_apf2) + t;
        let t = self.rr(m_rapf2.wrapping_sub(d_apf2));
        ro -= m(v_apf2, t);
        self.rw(m_rapf2, ro);
        ro = m(ro, v_apf2) + t;

        let base = self.rev_base;
        let mut next = (self.rev_cur + 2) & 0x7FFFE;
        if next < base {
            next = base;
        }
        self.rev_cur = next;
        (m(lo.clamp(-32768, 32767), self.rev_vol_l as i32), m(ro.clamp(-32768, 32767), self.rev_vol_r as i32))
    }

    /// Returns true when an SPU IRQ was raised.
    pub fn tick(&mut self, cyc: u32, cd: &mut CdRom) -> bool {
        self.cyc += cyc;
        while self.cyc >= CYCLES_PER_SAMPLE {
            self.cyc -= CYCLES_PER_SAMPLE;
            self.generate(cd);
        }
        std::mem::take(&mut self.irq_pending)
    }

    pub fn take_audio(&mut self) -> Vec<i16> {
        std::mem::take(&mut self.out)
    }
}
