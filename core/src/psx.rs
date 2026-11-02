//! Top-level machine: CPU + bus, frame stepping and EXE sideloading.

use serde::{Deserialize, Serialize};

use crate::bus::{Bus, BIOS_SIZE};
use crate::cpu::Cpu;

/// Average cycles charged per instruction (approximates cache/memory stalls).
const CYCLES_PER_INSTR: u32 = 2;
/// Instructions executed between device updates (64 cycles: low overhead, tiny IRQ latency).
const BATCH: u32 = 32;
/// Max cycles fast-forwarded per detected idle loop, and the device step size while doing so.
const IDLE_SKIP: u32 = 4096;
const IDLE_STEP: u32 = 64;

#[derive(Serialize, Deserialize)]
pub struct Psx {
    pub cpu: Cpu,
    pub bus: Bus,
    pub bios_loaded: bool,
    pending_exe: Option<Vec<u8>>,
    pub fast_boot: bool,
    #[serde(skip)]
    shell_hook_done: bool,
    #[serde(skip)]
    pub idle_cycles: u64,
}

const STATE_MAGIC: &[u8; 8] = b"PS1WSTAT";
const STATE_VERSION: u32 = 1;

impl Psx {
    /// Serialize the whole machine (except BIOS, disc image and memory card contents).
    pub fn save_state(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(4 * 1024 * 1024);
        out.extend_from_slice(STATE_MAGIC);
        out.extend_from_slice(&STATE_VERSION.to_le_bytes());
        bincode::serialize_into(&mut out, self).expect("serialize state");
        out
    }

    /// Restore a state produced by `save_state`. Keeps the current BIOS, disc,
    /// memory cards, controller types and user options.
    pub fn load_state(&mut self, data: &[u8]) -> Result<(), String> {
        if data.len() < 12 || &data[0..8] != STATE_MAGIC {
            return Err("Bukan file save state".into());
        }
        let ver = u32::from_le_bytes([data[8], data[9], data[10], data[11]]);
        if ver != STATE_VERSION {
            return Err(format!("Versi save state tidak cocok ({ver})"));
        }
        let mut new: Psx = bincode::deserialize(&data[12..]).map_err(|e| format!("State rusak: {e}"))?;
        let old = &mut self.bus;
        new.bus.bios = std::mem::take(&mut old.bios);
        new.bus.cd.disc = old.cd.disc.take();
        for i in 0..2 {
            new.bus.pad.cards[i].data = std::mem::take(&mut old.pad.cards[i].data);
            new.bus.pad.cards[i].present = old.pad.cards[i].present;
            new.bus.pad.pads[i].kind = old.pad.pads[i].kind;
            new.bus.pad.pads[i].buttons = old.pad.pads[i].buttons;
            new.bus.pad.pads[i].axes = old.pad.pads[i].axes;
        }
        new.bus.gpu.opt_dither = old.gpu.opt_dither;
        new.bus.gpu.threaded = old.gpu.threaded;
        new.bus.gpu.out = std::mem::take(&mut old.gpu.out);
        new.bus.gpu_sync = old.gpu_sync.take();
        new.bus.spu.master_volume = old.spu.master_volume;
        new.bus.spu.reverb_enabled = old.spu.reverb_enabled;
        new.bus.spu.interp = old.spu.interp;
        new.bios_loaded = self.bios_loaded;
        *self = new;
        Ok(())
    }

    pub fn new() -> Self {
        Psx { cpu: Cpu::new(), bus: Bus::new(), bios_loaded: false, pending_exe: None, fast_boot: false, shell_hook_done: false, idle_cycles: 0 }
    }

    pub fn load_bios(&mut self, data: &[u8]) -> Result<(), String> {
        if data.len() != BIOS_SIZE {
            return Err(format!("Ukuran BIOS harus 524288 byte (didapat {})", data.len()));
        }
        self.bus.bios.copy_from_slice(data);
        self.bios_loaded = true;
        Ok(())
    }

    pub fn reset(&mut self) {
        self.shell_hook_done = false;
        self.cpu.reset();
        self.bus.reset();
    }

    /// Queue a PS-X EXE; it is injected when the BIOS reaches the shell entry point.
    pub fn load_exe(&mut self, data: &[u8]) -> Result<(), String> {
        if data.len() < 0x800 || &data[0..8] != b"PS-X EXE" {
            return Err("Bukan file PS-X EXE".into());
        }
        self.pending_exe = Some(data.to_vec());
        if !self.bios_loaded {
            self.inject_exe();
        }
        Ok(())
    }

    fn inject_exe(&mut self) {
        let data = match self.pending_exe.take() {
            Some(d) => d,
            None => return,
        };
        let rd = |o: usize| u32::from_le_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]);
        let pc = rd(0x10);
        let gp = rd(0x14);
        let dst = rd(0x18);
        let size = rd(0x1C) as usize;
        let sp_base = rd(0x30);
        let sp_size = rd(0x34);
        let end = (0x800 + size).min(data.len());
        self.bus.ram_write_bytes(dst, &data[0x800..end]);
        self.cpu.set_reg_direct(28, gp);
        if sp_base != 0 {
            let sp = sp_base.wrapping_add(sp_size);
            self.cpu.set_reg_direct(29, sp);
            self.cpu.set_reg_direct(30, sp);
        } else if !self.bios_loaded {
            self.cpu.set_reg_direct(29, 0x801F_FFF0);
        }
        self.cpu.set_pc(pc);
    }

    /// Run until the next vertical blank (one video frame).
    pub fn run_frame(&mut self) {
        self.bus.frame_done = false;
        // Arm the BIOS shell entry hook (EXE sideloading / fast boot) until it fires once.
        let want = !self.shell_hook_done
            && self.bios_loaded
            && (self.pending_exe.is_some() || (self.fast_boot && self.bus.cd.disc.is_some()));
        self.cpu.hook_pc = if want { 0x8003_0000 } else { 0 };
        // Safety budget: ~2 frames worth of cycles.
        let mut budget: i64 = 1_200_000;
        while !self.bus.frame_done && budget > 0 {
            self.cpu.poll_irq(&mut self.bus);
            for _ in 0..BATCH {
                self.cpu.step(&mut self.bus);
            }
            self.bus.tick(BATCH * CYCLES_PER_INSTR);
            budget -= (BATCH * CYCLES_PER_INSTR) as i64;
            if self.cpu.idle {
                // CPU is waiting: advance devices without interpreting the spin loop.
                self.cpu.idle = false;
                let mut skipped = 0;
                while skipped < IDLE_SKIP && !self.bus.irq_pending() && !self.bus.frame_done {
                    self.bus.tick(IDLE_STEP);
                    skipped += IDLE_STEP;
                }
                budget -= skipped as i64;
                self.idle_cycles += skipped as u64;
            }
            if self.cpu.hook_hit {
                self.cpu.hook_hit = false;
                self.cpu.hook_pc = 0;
                self.shell_hook_done = true;
                if self.pending_exe.is_some() {
                    self.inject_exe();
                } else if self.fast_boot {
                    // Skip the BIOS shell (logo / menu): return to its caller, which goes on to
                    // boot the disc. Same approach as PCSX's "fast boot".
                    let ra = self.cpu.regs[31];
                    self.cpu.set_pc(ra);
                }
            }
        }
    }
}
