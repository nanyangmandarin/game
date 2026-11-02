//! Memory map, interrupt controller and DMA.

use serde::{Deserialize, Serialize};

use crate::cdrom::CdRom;
use crate::gpu::{Gpu, EV_HBLANK_END, EV_HBLANK_START, EV_VBLANK_END, EV_VBLANK_START};
use crate::mdec::Mdec;
use crate::pad::Sio0;
use crate::spu::Spu;
use crate::timers::Timers;

pub const IRQ_VBLANK: u32 = 0;
pub const IRQ_GPU: u32 = 1;
pub const IRQ_CDROM: u32 = 2;
pub const IRQ_DMA: u32 = 3;
pub const IRQ_PAD: u32 = 7;
pub const IRQ_SPU: u32 = 9;

pub const RAM_SIZE: usize = 2 * 1024 * 1024;
pub const BIOS_SIZE: usize = 512 * 1024;

#[derive(Default, Clone, Copy, Serialize, Deserialize)]
struct DmaChannel {
    madr: u32,
    bcr: u32,
    chcr: u32,
}

#[derive(Serialize, Deserialize)]
pub struct Bus {
    pub ram: Vec<u8>,
    #[serde(skip)]
    pub bios: Vec<u8>,
    scratch: Vec<u8>,
    pub i_stat: u32,
    pub i_mask: u32,
    dma: [DmaChannel; 7],
    dpcr: u32,
    dicr: u32,
    pub timers: Timers,
    pub gpu: Gpu,
    pub cd: CdRom,
    pub spu: Spu,
    pub pad: Sio0,
    pub mdec: Mdec,
    mem_ctrl: [u32; 9],
    ram_size_reg: u32,
    cache_ctrl: u32,
    pub frame_done: bool,
    pub cycles: u64,
}

impl Bus {
    pub fn new() -> Self {
        Bus {
            ram: vec![0; RAM_SIZE],
            bios: vec![0; BIOS_SIZE],
            scratch: vec![0; 1024],
            i_stat: 0,
            i_mask: 0,
            dma: [DmaChannel::default(); 7],
            dpcr: 0x0765_4321,
            dicr: 0,
            timers: Timers::new(),
            gpu: Gpu::new(),
            cd: CdRom::new(),
            spu: Spu::new(),
            pad: Sio0::new(),
            mdec: Mdec::new(),
            mem_ctrl: [0; 9],
            ram_size_reg: 0,
            cache_ctrl: 0,
            frame_done: false,
            cycles: 0,
        }
    }

    /// Reset everything except BIOS, inserted disc, memory cards and controller config.
    pub fn reset(&mut self) {
        self.ram.iter_mut().for_each(|b| *b = 0);
        self.scratch.iter_mut().for_each(|b| *b = 0);
        self.i_stat = 0;
        self.i_mask = 0;
        self.dma = [DmaChannel::default(); 7];
        self.dpcr = 0x0765_4321;
        self.dicr = 0;
        self.timers = Timers::new();
        self.gpu.reset();
        self.cd.reset();
        self.spu.reset();
        self.pad.reset();
        self.mdec = Mdec::new();
        self.frame_done = false;
        self.cycles = 0;
    }

    #[inline(always)]
    pub fn irq_pending(&self) -> bool {
        self.i_stat & self.i_mask != 0
    }

    #[inline(always)]
    pub fn irq(&mut self, n: u32) {
        self.i_stat |= 1 << n;
    }

    /// Advance all devices by `cyc` CPU cycles.
    pub fn tick(&mut self, cyc: u32) {
        self.cycles += cyc as u64;
        let ev = self.gpu.tick(cyc);
        if ev != 0 {
            if ev & EV_VBLANK_START != 0 {
                self.irq(IRQ_VBLANK);
                self.frame_done = true;
            }
            self.timers.blank_events(
                ev & EV_HBLANK_START != 0,
                ev & EV_HBLANK_END != 0,
                ev & EV_VBLANK_START != 0,
                ev & EV_VBLANK_END != 0,
            );
        }
        if self.gpu.take_irq() {
            self.irq(IRQ_GPU);
        }
        let t = self.timers.tick(cyc, self.gpu.dot_divider());
        self.i_stat |= t << 4;
        if self.cd.tick(cyc) {
            self.irq(IRQ_CDROM);
        }
        if self.spu.tick(cyc, &mut self.cd) {
            self.irq(IRQ_SPU);
        }
        if self.pad.tick(cyc) {
            self.irq(IRQ_PAD);
        }
    }

    // ---------------- CPU access ----------------

    #[inline(always)]
    fn phys(addr: u32) -> u32 {
        if addr >= 0xC000_0000 {
            addr
        } else {
            addr & 0x1FFF_FFFF
        }
    }

    #[inline(always)]
    pub fn fetch(&mut self, addr: u32) -> u32 {
        let a = Self::phys(addr);
        if a < 0x0080_0000 {
            let o = (a as usize) & (RAM_SIZE - 4);
            // SAFETY: o is 4-aligned and o + 4 <= RAM_SIZE == ram.len()
            unsafe { u32::from_le((self.ram.as_ptr().add(o) as *const u32).read_unaligned()) }
        } else if (0x1FC0_0000..0x1FC8_0000).contains(&a) {
            let o = (a - 0x1FC0_0000) as usize;
            let o = o & (BIOS_SIZE - 4);
            if self.bios.len() != BIOS_SIZE {
                return 0;
            }
            // SAFETY: o is 4-aligned and o + 4 <= BIOS_SIZE == bios.len()
            unsafe { u32::from_le((self.bios.as_ptr().add(o) as *const u32).read_unaligned()) }
        } else {
            self.read32(addr)
        }
    }

    pub fn read8(&mut self, addr: u32) -> u32 {
        self.read(addr, 1)
    }
    pub fn read16(&mut self, addr: u32) -> u32 {
        self.read(addr, 2)
    }
    pub fn read32(&mut self, addr: u32) -> u32 {
        self.read(addr, 4)
    }
    pub fn write8(&mut self, addr: u32, v: u32) {
        self.write(addr, 1, v)
    }
    pub fn write16(&mut self, addr: u32, v: u32) {
        self.write(addr, 2, v)
    }
    pub fn write32(&mut self, addr: u32, v: u32) {
        self.write(addr, 4, v)
    }

    #[inline(always)]
    fn rd_slice(s: &[u8], o: usize, size: u32) -> u32 {
        match size {
            1 => s[o] as u32,
            2 => u16::from_le_bytes(s[o..o + 2].try_into().unwrap()) as u32,
            _ => u32::from_le_bytes(s[o..o + 4].try_into().unwrap()),
        }
    }

    #[inline(always)]
    fn wr_slice(s: &mut [u8], o: usize, size: u32, v: u32) {
        match size {
            1 => s[o] = v as u8,
            2 => s[o..o + 2].copy_from_slice(&(v as u16).to_le_bytes()),
            _ => s[o..o + 4].copy_from_slice(&v.to_le_bytes()),
        }
    }

    pub fn read(&mut self, addr: u32, size: u32) -> u32 {
        let a = Self::phys(addr);
        if a < 0x0080_0000 {
            return Self::rd_slice(&self.ram, (a as usize) & (RAM_SIZE - 1), size);
        }
        if (0x1FC0_0000..0x1FC8_0000).contains(&a) {
            return Self::rd_slice(&self.bios, (a - 0x1FC0_0000) as usize, size);
        }
        if (0x1F80_0000..0x1F80_0400).contains(&a) {
            return Self::rd_slice(&self.scratch, (a - 0x1F80_0000) as usize, size);
        }
        if (0x1F80_1000..0x1F80_3000).contains(&a) {
            return self.io_read(a - 0x1F80_1000, size);
        }
        if (0x1F00_0000..0x1F80_0000).contains(&a) {
            return 0xFFFF_FFFF >> (32 - size * 8); // expansion region 1: open bus
        }
        if a == 0xFFFE_0130 {
            return self.cache_ctrl;
        }
        0
    }

    pub fn write(&mut self, addr: u32, size: u32, v: u32) {
        let a = Self::phys(addr);
        if a < 0x0080_0000 {
            Self::wr_slice(&mut self.ram, (a as usize) & (RAM_SIZE - 1), size, v);
        } else if (0x1F80_0000..0x1F80_0400).contains(&a) {
            Self::wr_slice(&mut self.scratch, (a - 0x1F80_0000) as usize, size, v);
        } else if (0x1F80_1000..0x1F80_3000).contains(&a) {
            self.io_write(a - 0x1F80_1000, size, v);
        } else if a == 0xFFFE_0130 {
            self.cache_ctrl = v;
        }
    }

    fn io_read(&mut self, off: u32, size: u32) -> u32 {
        let sub = (off & 3) * 8;
        let mask = if size == 4 { 0xFFFF_FFFF } else { (1u32 << (size * 8)) - 1 };
        match off {
            0x000..=0x023 => self.mem_ctrl[(off / 4) as usize] >> sub & mask,
            0x040..=0x05F => self.pad.read(off - 0x40, size),
            0x060..=0x063 => self.ram_size_reg >> sub & mask,
            0x070..=0x073 => self.i_stat >> sub & mask,
            0x074..=0x077 => self.i_mask >> sub & mask,
            0x080..=0x0FF => self.dma_read(off & !3) >> sub & mask,
            0x100..=0x12F => self.timers.read(off & !3) >> sub & mask,
            0x800..=0x803 => {
                let mut r = 0;
                for k in 0..size {
                    r |= (self.cd.read((off + k) & 3) as u32) << (k * 8);
                }
                r
            }
            0x810..=0x817 => {
                let v = if off & !3 == 0x810 { self.gpu.read_data() } else { self.gpu.status() };
                v >> sub & mask
            }
            0x820..=0x827 => {
                let v = if off & !3 == 0x820 { self.mdec.read_data() } else { self.mdec.status() };
                v >> sub & mask
            }
            0xC00..=0xFFF => {
                let lo = self.spu.read16(off & !1);
                if size == 4 {
                    lo as u32 | ((self.spu.read16((off & !1) + 2) as u32) << 16)
                } else if size == 1 {
                    (lo as u32 >> ((off & 1) * 8)) & 0xFF
                } else {
                    lo as u32
                }
            }
            _ => 0,
        }
    }

    fn io_write(&mut self, off: u32, size: u32, v: u32) {
        match off {
            0x000..=0x023 => self.mem_ctrl[(off / 4) as usize] = v,
            0x040..=0x05F => self.pad.write(off - 0x40, size, v),
            0x060..=0x063 => self.ram_size_reg = v,
            0x070..=0x073 => self.i_stat &= v & 0x7FF,
            0x074..=0x077 => self.i_mask = v & 0x7FF,
            0x080..=0x0FF => {
                let v = if off & 3 != 0 { v << ((off & 3) * 8) } else { v };
                self.dma_write(off & !3, v)
            }
            0x100..=0x12F => self.timers.write(off & !3, v),
            0x800..=0x803 => {
                for k in 0..size {
                    self.cd.write((off + k) & 3, (v >> (k * 8)) as u8);
                }
            }
            0x810 => self.gpu.gp0(v),
            0x814 => self.gpu.gp1(v),
            0x820 => self.mdec.write_cmd(v),
            0x824 => self.mdec.write_ctrl(v),
            0xC00..=0xFFF => {
                if size == 4 {
                    self.spu.write16(off, v as u16);
                    self.spu.write16(off + 2, (v >> 16) as u16);
                } else {
                    self.spu.write16(off & !1, v as u16);
                }
            }
            _ => {}
        }
    }

    // ---------------- DMA ----------------

    fn dma_read(&self, off: u32) -> u32 {
        match off {
            0x0F0 => self.dpcr,
            0x0F4 => {
                let force = self.dicr & (1 << 15) != 0;
                let master = self.dicr & (1 << 23) != 0;
                let flags = (self.dicr >> 24) & 0x7F;
                let en = (self.dicr >> 16) & 0x7F;
                let irq = force || (master && (flags & en) != 0);
                (self.dicr & 0x7FFF_FFFF) | ((irq as u32) << 31)
            }
            _ => {
                let ch = ((off - 0x80) >> 4) as usize;
                if ch > 6 {
                    return 0;
                }
                match off & 0xF {
                    0 => self.dma[ch].madr,
                    4 => self.dma[ch].bcr,
                    _ => self.dma[ch].chcr,
                }
            }
        }
    }

    fn dma_write(&mut self, off: u32, v: u32) {
        match off {
            0x0F0 => self.dpcr = v,
            0x0F4 => {
                let ack = v & 0x7F00_0000;
                let keep = self.dicr & 0x7F00_0000 & !ack;
                self.dicr = (v & 0x00FF_803F) | keep;
            }
            _ => {
                let ch = ((off - 0x80) >> 4) as usize;
                if ch > 6 {
                    return;
                }
                match off & 0xF {
                    0 => self.dma[ch].madr = v & 0x00FF_FFFF,
                    4 => self.dma[ch].bcr = v,
                    _ => {
                        let v = if ch == 6 { (v & 0x5100_0000) | 2 } else { v };
                        self.dma[ch].chcr = v;
                        self.dma_check(ch);
                    }
                }
            }
        }
    }

    fn dma_check(&mut self, ch: usize) {
        let c = self.dma[ch].chcr;
        let enabled = c & (1 << 24) != 0;
        let sync = (c >> 9) & 3;
        let trigger = c & (1 << 28) != 0;
        let master = self.dpcr & (8 << (ch * 4)) != 0;
        if enabled && master && (sync != 0 || trigger) {
            // MDEC output waits until the decoder has produced data.
            if ch == 1 && self.mdec.out_len() == 0 {
                return;
            }
            self.dma_run(ch);
            if ch == 0 {
                self.dma_check(1);
            }
        }
    }

    fn dma_done(&mut self, ch: usize) {
        self.dma[ch].chcr &= !((1 << 24) | (1 << 28));
        let was = self.dicr_irq_line();
        if self.dicr & (1 << (16 + ch)) != 0 {
            self.dicr |= 1 << (24 + ch);
        }
        if !was && self.dicr_irq_line() {
            self.irq(IRQ_DMA);
        }
    }

    fn dicr_irq_line(&self) -> bool {
        let force = self.dicr & (1 << 15) != 0;
        let master = self.dicr & (1 << 23) != 0;
        force || (master && ((self.dicr >> 24) & (self.dicr >> 16) & 0x7F) != 0)
    }

    fn ram_r32(&self, a: u32) -> u32 {
        let o = (a as usize) & (RAM_SIZE - 4);
        u32::from_le_bytes([self.ram[o], self.ram[o + 1], self.ram[o + 2], self.ram[o + 3]])
    }

    fn ram_w32(&mut self, a: u32, v: u32) {
        let o = (a as usize) & (RAM_SIZE - 4);
        self.ram[o..o + 4].copy_from_slice(&v.to_le_bytes());
    }

    fn dma_run(&mut self, ch: usize) {
        let c = self.dma[ch].chcr;
        let from_ram = c & 1 != 0;
        let step: u32 = if c & 2 != 0 { (-4i32) as u32 } else { 4 };
        let sync = (c >> 9) & 3;
        let mut addr = self.dma[ch].madr & 0x1F_FFFC;

        if sync == 2 {
            // Linked list (GPU ordering tables).
            let mut guard = 0;
            loop {
                let header = self.ram_r32(addr);
                let n = header >> 24;
                for k in 0..n {
                    let w = self.ram_r32(addr.wrapping_add(4 * (k + 1)) & 0x1F_FFFC);
                    self.gpu.gp0(w);
                }
                guard += 1;
                if header & 0x80_0000 != 0 || guard > 0x10_0000 {
                    break;
                }
                addr = header & 0x1F_FFFC;
            }
            self.dma[ch].madr = 0x00FF_FFFF;
            self.dma_done(ch);
            return;
        }

        let words = if sync == 0 {
            let n = self.dma[ch].bcr & 0xFFFF;
            if n == 0 { 0x10000 } else { n }
        } else {
            let bs = self.dma[ch].bcr & 0xFFFF;
            let bc = self.dma[ch].bcr >> 16;
            bs * bc
        };

        for k in 0..words {
            let a = addr & 0x1F_FFFC;
            if from_ram {
                let w = self.ram_r32(a);
                match ch {
                    0 => self.mdec.write_cmd(w),
                    2 => self.gpu.gp0(w),
                    4 => self.spu.dma_write(w),
                    _ => {}
                }
            } else {
                let w = match ch {
                    1 => self.mdec.read_data(),
                    2 => self.gpu.read_data(),
                    3 => self.cd.dma_read_word(),
                    4 => self.spu.dma_read(),
                    6 => {
                        if k == words - 1 {
                            0x00FF_FFFF
                        } else {
                            addr.wrapping_sub(4) & 0x1F_FFFF
                        }
                    }
                    _ => 0,
                };
                self.ram_w32(a, w);
            }
            addr = addr.wrapping_add(step);
        }
        if sync == 1 {
            self.dma[ch].madr = addr & 0x00FF_FFFF;
            self.dma[ch].bcr &= 0xFFFF;
        }
        self.dma_done(ch);
    }

    // ---------------- helpers for sideloading ----------------

    pub fn ram_write_bytes(&mut self, addr: u32, data: &[u8]) {
        let base = (addr & 0x1F_FFFF) as usize;
        for (i, b) in data.iter().enumerate() {
            self.ram[(base + i) & (RAM_SIZE - 1)] = *b;
        }
    }
}
