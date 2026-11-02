//! MIPS R3000A interpreter (PS1 CPU) with COP0 and COP2 (GTE) dispatch.

use serde::{Deserialize, Serialize};

use crate::bus::Bus;
use crate::gte::Gte;

const EXC_INT: u32 = 0x0;
const EXC_ADEL: u32 = 0x4;
const EXC_ADES: u32 = 0x5;
const EXC_SYSCALL: u32 = 0x8;
const EXC_BREAK: u32 = 0x9;
const EXC_RI: u32 = 0xA;
const EXC_CPU: u32 = 0xB;
const EXC_OVF: u32 = 0xC;

#[derive(Serialize, Deserialize)]
pub struct Cpu {
    pub regs: [u32; 32],
    /// Register written by the current instruction (cancels a pending delayed load to it).
    written: u32,
    pub pc: u32,
    next_pc: u32,
    current_pc: u32,
    pub hi: u32,
    pub lo: u32,
    load: (usize, u32),
    pending: (usize, u32),
    branch: bool,
    delay_slot: bool,
    pub sr: u32,
    cause: u32,
    epc: u32,
    badvaddr: u32,
    cop0_misc: [u32; 16],
    pub gte: Gte,
    pub stat_exceptions: u32,
    // ---- idle-loop detection (not part of save states) ----
    /// Set when the CPU is spinning in a short loop that cannot change anything by itself.
    #[serde(skip)]
    pub idle: bool,
    #[serde(skip)]
    loop_target: u32,
    #[serde(skip)]
    loop_regs: [u32; 32],
    #[serde(skip)]
    loop_hilo: (u32, u32),
    #[serde(skip)]
    loop_side_effect: bool,
    #[serde(skip)]
    loop_hits: u32,
}

impl Cpu {
    pub fn new() -> Self {
        Cpu {
            regs: [0; 32],
            written: 0,
            pc: 0xBFC0_0000,
            next_pc: 0xBFC0_0004,
            current_pc: 0xBFC0_0000,
            hi: 0,
            lo: 0,
            load: (0, 0),
            pending: (0, 0),
            branch: false,
            delay_slot: false,
            sr: 0,
            cause: 0,
            epc: 0,
            badvaddr: 0,
            cop0_misc: [0; 16],
            gte: Gte::new(),
            stat_exceptions: 0,
            idle: false,
            loop_target: 0,
            loop_regs: [0; 32],
            loop_hilo: (0, 0),
            loop_side_effect: false,
            loop_hits: 0,
        }
    }

    pub fn reset(&mut self) {
        *self = Cpu::new();
    }

    /// Jump directly to an address (used by the PS-X EXE sideloader).
    pub fn set_pc(&mut self, pc: u32) {
        self.pc = pc;
        self.next_pc = pc.wrapping_add(4);
        self.branch = false;
        self.load = (0, 0);
    }

    pub fn set_reg_direct(&mut self, r: usize, v: u32) {
        if r != 0 {
            self.regs[r] = v;
        }
    }

    #[inline(always)]
    fn reg(&self, r: u32) -> u32 {
        self.regs[r as usize]
    }

    #[inline(always)]
    fn set_reg(&mut self, r: u32, v: u32) {
        self.regs[r as usize] = v;
        self.regs[0] = 0;
        self.written = r;
    }

    /// Current value including an in-flight delayed load (used by LWL/LWR).
    #[inline(always)]
    fn reg_inflight(&self, r: u32, pending: (usize, u32)) -> u32 {
        if pending.0 == r as usize && r != 0 { pending.1 } else { self.regs[r as usize] }
    }

    fn exception(&mut self, code: u32) {
        self.loop_target = 0;
        self.stat_exceptions += 1;
        let handler = if self.sr & (1 << 22) != 0 { 0xBFC0_0180 } else { 0x8000_0080 };
        let mode = self.sr & 0x3F;
        self.sr = (self.sr & !0x3F) | ((mode << 2) & 0x3F);
        self.cause = (self.cause & !0x8000_007C) | (code << 2);
        if self.delay_slot {
            self.epc = self.current_pc.wrapping_sub(4);
            self.cause |= 1 << 31;
        } else {
            self.epc = self.current_pc;
        }
        self.pc = handler;
        self.next_pc = handler.wrapping_add(4);
    }

    /// Check for a pending hardware/software interrupt and take it.
    /// Called once per instruction batch (a few dozen cycles of latency).
    #[inline]
    pub fn poll_irq(&mut self, bus: &mut Bus) {
        // Hardware interrupt line (IP2) mirrors I_STAT & I_MASK.
        if bus.irq_pending() {
            self.cause |= 1 << 10;
        } else {
            self.cause &= !(1 << 10);
        }
        if self.sr & 1 != 0 && (self.sr & self.cause & 0xFF00) != 0 {
            let pc = self.pc;
            self.current_pc = pc;
            self.delay_slot = self.branch;
            self.branch = false;
            // Flush pending load before taking the interrupt.
            let (r, v) = self.load;
            self.regs[r] = v;
            self.regs[0] = 0;
            self.load = (0, 0);
            // GTE instructions at the interrupt address would be skipped on hardware;
            // the BIOS handler compensates, so we emulate that by executing it first.
            let instr = bus.fetch(pc);
            if (instr >> 25) == 0x25 {
                self.gte.command(instr);
            }
            self.exception(EXC_INT);
        }
    }

    /// Execute one instruction.
    #[inline]
    pub fn step(&mut self, bus: &mut Bus) {
        let pc = self.pc;
        self.current_pc = pc;
        self.delay_slot = self.branch;
        self.branch = false;

        if pc & 3 != 0 {
            self.badvaddr = pc;
            self.exception(EXC_ADEL);
            return;
        }
        let instr = bus.fetch(pc);
        self.pc = self.next_pc;
        self.next_pc = self.next_pc.wrapping_add(4);

        let pending = self.load;
        self.load = (0, 0);
        self.written = 0;
        self.pending = pending;

        self.execute(instr, bus);

        // Apply the previous instruction's delayed load unless this one overwrote it.
        let (r, v) = pending;
        if r != 0 && self.written as usize != r {
            self.regs[r] = v;
        }

        // A taken branch jumping back at most 16 instructions closes a short loop.
        if self.branch && self.next_pc <= pc && pc - self.next_pc <= 64 {
            self.check_idle_loop(self.next_pc);
        }
    }

    /// Idle-loop detection: the same short loop entered again with identical registers
    /// and no stores/coprocessor writes in between can only end through an interrupt,
    /// DMA or device state change, so emulated time can be fast-forwarded.
    #[cold]
    fn check_idle_loop(&mut self, target: u32) {
        let hilo = (self.hi, self.lo);
        if target == self.loop_target && !self.loop_side_effect && self.regs == self.loop_regs && hilo == self.loop_hilo {
            self.loop_hits += 1;
            if self.loop_hits >= 2 {
                self.idle = true;
            }
        } else {
            self.loop_target = target;
            self.loop_regs = self.regs;
            self.loop_hilo = hilo;
            self.loop_hits = 0;
        }
        self.loop_side_effect = false;
    }

    #[inline(always)]
    fn branch_to(&mut self, offset: u32) {
        self.next_pc = self.pc.wrapping_add(offset << 2);
        self.branch = true;
    }

    fn load_addr_ok(&mut self, addr: u32, align: u32) -> bool {
        if addr & (align - 1) != 0 {
            self.badvaddr = addr;
            self.exception(EXC_ADEL);
            false
        } else {
            true
        }
    }

    fn store_addr_ok(&mut self, addr: u32, align: u32) -> bool {
        if addr & (align - 1) != 0 {
            self.badvaddr = addr;
            self.exception(EXC_ADES);
            false
        } else {
            true
        }
    }

    #[inline(always)]
    fn cache_isolated(&self) -> bool {
        self.sr & 0x10000 != 0
    }

    fn execute(&mut self, i: u32, bus: &mut Bus) {
        let op = i >> 26;
        let rs = (i >> 21) & 0x1F;
        let rt = (i >> 16) & 0x1F;
        let rd = (i >> 11) & 0x1F;
        let sh = (i >> 6) & 0x1F;
        let imm = i & 0xFFFF;
        let imm_se = (i as i16) as u32;

        match op {
            0x00 => match i & 0x3F {
                0x00 => self.set_reg(rd, self.reg(rt) << sh),
                0x02 => self.set_reg(rd, self.reg(rt) >> sh),
                0x03 => self.set_reg(rd, ((self.reg(rt) as i32) >> sh) as u32),
                0x04 => self.set_reg(rd, self.reg(rt) << (self.reg(rs) & 0x1F)),
                0x06 => self.set_reg(rd, self.reg(rt) >> (self.reg(rs) & 0x1F)),
                0x07 => self.set_reg(rd, ((self.reg(rt) as i32) >> (self.reg(rs) & 0x1F)) as u32),
                0x08 => {
                    self.next_pc = self.reg(rs);
                    self.branch = true;
                }
                0x09 => {
                    let ra = self.next_pc;
                    self.next_pc = self.reg(rs);
                    self.branch = true;
                    self.set_reg(rd, ra);
                }
                0x0C => self.exception(EXC_SYSCALL),
                0x0D => self.exception(EXC_BREAK),
                0x10 => self.set_reg(rd, self.hi),
                0x11 => self.hi = self.reg(rs),
                0x12 => self.set_reg(rd, self.lo),
                0x13 => self.lo = self.reg(rs),
                0x18 => {
                    let r = (self.reg(rs) as i32 as i64) * (self.reg(rt) as i32 as i64);
                    self.hi = (r >> 32) as u32;
                    self.lo = r as u32;
                }
                0x19 => {
                    let r = (self.reg(rs) as u64) * (self.reg(rt) as u64);
                    self.hi = (r >> 32) as u32;
                    self.lo = r as u32;
                }
                0x1A => {
                    let n = self.reg(rs) as i32;
                    let d = self.reg(rt) as i32;
                    if d == 0 {
                        self.hi = n as u32;
                        self.lo = if n >= 0 { 0xFFFF_FFFF } else { 1 };
                    } else if n as u32 == 0x8000_0000 && d == -1 {
                        self.hi = 0;
                        self.lo = 0x8000_0000;
                    } else {
                        self.hi = (n % d) as u32;
                        self.lo = (n / d) as u32;
                    }
                }
                0x1B => {
                    let n = self.reg(rs);
                    let d = self.reg(rt);
                    if d == 0 {
                        self.hi = n;
                        self.lo = 0xFFFF_FFFF;
                    } else {
                        self.hi = n % d;
                        self.lo = n / d;
                    }
                }
                0x20 => match (self.reg(rs) as i32).checked_add(self.reg(rt) as i32) {
                    Some(v) => self.set_reg(rd, v as u32),
                    None => self.exception(EXC_OVF),
                },
                0x21 => self.set_reg(rd, self.reg(rs).wrapping_add(self.reg(rt))),
                0x22 => match (self.reg(rs) as i32).checked_sub(self.reg(rt) as i32) {
                    Some(v) => self.set_reg(rd, v as u32),
                    None => self.exception(EXC_OVF),
                },
                0x23 => self.set_reg(rd, self.reg(rs).wrapping_sub(self.reg(rt))),
                0x24 => self.set_reg(rd, self.reg(rs) & self.reg(rt)),
                0x25 => self.set_reg(rd, self.reg(rs) | self.reg(rt)),
                0x26 => self.set_reg(rd, self.reg(rs) ^ self.reg(rt)),
                0x27 => self.set_reg(rd, !(self.reg(rs) | self.reg(rt))),
                0x2A => self.set_reg(rd, ((self.reg(rs) as i32) < (self.reg(rt) as i32)) as u32),
                0x2B => self.set_reg(rd, (self.reg(rs) < self.reg(rt)) as u32),
                _ => self.exception(EXC_RI),
            },
            0x01 => {
                let v = self.reg(rs) as i32;
                let bgez = rt & 1 != 0;
                let link = rt & 0x1E == 0x10;
                let taken = if bgez { v >= 0 } else { v < 0 };
                if link {
                    let ra = self.next_pc;
                    self.set_reg(31, ra);
                }
                if taken {
                    self.branch_to(imm_se);
                } else {
                    self.branch = true;
                }
            }
            0x02 => {
                self.next_pc = (self.pc & 0xF000_0000) | ((i & 0x03FF_FFFF) << 2);
                self.branch = true;
            }
            0x03 => {
                let ra = self.next_pc;
                self.next_pc = (self.pc & 0xF000_0000) | ((i & 0x03FF_FFFF) << 2);
                self.branch = true;
                self.set_reg(31, ra);
            }
            0x04 => {
                self.branch = true;
                if self.reg(rs) == self.reg(rt) {
                    self.branch_to(imm_se);
                }
            }
            0x05 => {
                self.branch = true;
                if self.reg(rs) != self.reg(rt) {
                    self.branch_to(imm_se);
                }
            }
            0x06 => {
                self.branch = true;
                if (self.reg(rs) as i32) <= 0 {
                    self.branch_to(imm_se);
                }
            }
            0x07 => {
                self.branch = true;
                if (self.reg(rs) as i32) > 0 {
                    self.branch_to(imm_se);
                }
            }
            0x08 => match (self.reg(rs) as i32).checked_add(imm_se as i32) {
                Some(v) => self.set_reg(rt, v as u32),
                None => self.exception(EXC_OVF),
            },
            0x09 => self.set_reg(rt, self.reg(rs).wrapping_add(imm_se)),
            0x0A => self.set_reg(rt, ((self.reg(rs) as i32) < (imm_se as i32)) as u32),
            0x0B => self.set_reg(rt, (self.reg(rs) < imm_se) as u32),
            0x0C => self.set_reg(rt, self.reg(rs) & imm),
            0x0D => self.set_reg(rt, self.reg(rs) | imm),
            0x0E => self.set_reg(rt, self.reg(rs) ^ imm),
            0x0F => self.set_reg(rt, imm << 16),
            0x10 => {
                self.loop_side_effect = true;
                self.cop0(i, rs, rt, rd)
            }
            0x11 | 0x13 => self.exception(EXC_CPU),
            0x12 => {
                self.loop_side_effect = true;
                self.cop2(i, rs, rt, rd)
            }
            0x20 => {
                let a = self.reg(rs).wrapping_add(imm_se);
                if !self.cache_isolated() {
                    let v = bus.read8(a) as i8 as i32 as u32;
                    self.load = (rt as usize, v);
                }
            }
            0x21 => {
                let a = self.reg(rs).wrapping_add(imm_se);
                if self.load_addr_ok(a, 2) && !self.cache_isolated() {
                    let v = bus.read16(a) as i16 as i32 as u32;
                    self.load = (rt as usize, v);
                }
            }
            0x22 => {
                let a = self.reg(rs).wrapping_add(imm_se);
                let cur = self.reg_inflight(rt, self.pending);
                let w = bus.read32(a & !3);
                let v = match a & 3 {
                    0 => (cur & 0x00FF_FFFF) | (w << 24),
                    1 => (cur & 0x0000_FFFF) | (w << 16),
                    2 => (cur & 0x0000_00FF) | (w << 8),
                    _ => w,
                };
                self.load = (rt as usize, v);
            }
            0x23 => {
                let a = self.reg(rs).wrapping_add(imm_se);
                if self.load_addr_ok(a, 4) && !self.cache_isolated() {
                    let v = bus.read32(a);
                    self.load = (rt as usize, v);
                }
            }
            0x24 => {
                let a = self.reg(rs).wrapping_add(imm_se);
                if !self.cache_isolated() {
                    let v = bus.read8(a);
                    self.load = (rt as usize, v);
                }
            }
            0x25 => {
                let a = self.reg(rs).wrapping_add(imm_se);
                if self.load_addr_ok(a, 2) && !self.cache_isolated() {
                    let v = bus.read16(a);
                    self.load = (rt as usize, v);
                }
            }
            0x26 => {
                let a = self.reg(rs).wrapping_add(imm_se);
                let cur = self.reg_inflight(rt, self.pending);
                let w = bus.read32(a & !3);
                let v = match a & 3 {
                    0 => w,
                    1 => (cur & 0xFF00_0000) | (w >> 8),
                    2 => (cur & 0xFFFF_0000) | (w >> 16),
                    _ => (cur & 0xFFFF_FF00) | (w >> 24),
                };
                self.load = (rt as usize, v);
            }
            0x28 => {
                // stores can change state an idle loop might be waiting on
                self.loop_side_effect = true;
                let a = self.reg(rs).wrapping_add(imm_se);
                if !self.cache_isolated() {
                    bus.write8(a, self.reg(rt));
                }
            }
            0x29 => {
                // stores can change state an idle loop might be waiting on
                self.loop_side_effect = true;
                let a = self.reg(rs).wrapping_add(imm_se);
                if self.store_addr_ok(a, 2) && !self.cache_isolated() {
                    bus.write16(a, self.reg(rt));
                }
            }
            0x2A => {
                // stores can change state an idle loop might be waiting on
                self.loop_side_effect = true;
                let a = self.reg(rs).wrapping_add(imm_se);
                if !self.cache_isolated() {
                    let v = self.reg(rt);
                    let al = a & !3;
                    let cur = bus.read32(al);
                    let w = match a & 3 {
                        0 => (cur & 0xFFFF_FF00) | (v >> 24),
                        1 => (cur & 0xFFFF_0000) | (v >> 16),
                        2 => (cur & 0xFF00_0000) | (v >> 8),
                        _ => v,
                    };
                    bus.write32(al, w);
                }
            }
            0x2B => {
                // stores can change state an idle loop might be waiting on
                self.loop_side_effect = true;
                let a = self.reg(rs).wrapping_add(imm_se);
                if self.store_addr_ok(a, 4) && !self.cache_isolated() {
                    bus.write32(a, self.reg(rt));
                }
            }
            0x2E => {
                // stores can change state an idle loop might be waiting on
                self.loop_side_effect = true;
                let a = self.reg(rs).wrapping_add(imm_se);
                if !self.cache_isolated() {
                    let v = self.reg(rt);
                    let al = a & !3;
                    let cur = bus.read32(al);
                    let w = match a & 3 {
                        0 => v,
                        1 => (cur & 0x0000_00FF) | (v << 8),
                        2 => (cur & 0x0000_FFFF) | (v << 16),
                        _ => (cur & 0x00FF_FFFF) | (v << 24),
                    };
                    bus.write32(al, w);
                }
            }
            0x32 => {
                let a = self.reg(rs).wrapping_add(imm_se);
                if self.load_addr_ok(a, 4) {
                    let v = bus.read32(a);
                    self.gte.write_data(rt, v);
                }
            }
            0x3A => {
                // stores can change state an idle loop might be waiting on
                self.loop_side_effect = true;
                let a = self.reg(rs).wrapping_add(imm_se);
                if self.store_addr_ok(a, 4) && !self.cache_isolated() {
                    let v = self.gte.read_data(rt);
                    bus.write32(a, v);
                }
            }
            0x30 | 0x31 | 0x33 | 0x38 | 0x39 | 0x3B => self.exception(EXC_CPU),
            _ => self.exception(EXC_RI),
        }
    }

    fn cop0(&mut self, i: u32, rs: u32, rt: u32, rd: u32) {
        match rs {
            0x00 => {
                let v = match rd {
                    8 => self.badvaddr,
                    12 => self.sr,
                    13 => self.cause,
                    14 => self.epc,
                    15 => 0x0000_0002,
                    r => self.cop0_misc[r as usize & 15],
                };
                self.load = (rt as usize, v);
            }
            0x04 => {
                let v = self.reg(rt);
                match rd {
                    12 => self.sr = v,
                    13 => self.cause = (self.cause & !0x300) | (v & 0x300),
                    r => self.cop0_misc[r as usize & 15] = v,
                }
            }
            0x10 if i & 0x3F == 0x10 => {
                let mode = self.sr & 0x3F;
                self.sr = (self.sr & !0xF) | (mode >> 2);
            }
            _ => self.exception(EXC_RI),
        }
    }

    fn cop2(&mut self, i: u32, rs: u32, rt: u32, rd: u32) {
        if rs & 0x10 != 0 {
            self.gte.command(i);
            return;
        }
        match rs {
            0x00 => {
                let v = self.gte.read_data(rd);
                self.load = (rt as usize, v);
            }
            0x02 => {
                let v = self.gte.read_ctrl(rd);
                self.load = (rt as usize, v);
            }
            0x04 => {
                let v = self.reg(rt);
                self.gte.write_data(rd, v);
            }
            0x06 => {
                let v = self.reg(rt);
                self.gte.write_ctrl(rd, v);
            }
            _ => self.exception(EXC_RI),
        }
    }
}
