//! Top-level machine: CPU + bus, frame stepping and EXE sideloading.

use crate::bus::{Bus, BIOS_SIZE};
use crate::cpu::Cpu;

/// Average cycles charged per instruction (approximates cache/memory stalls).
const CYCLES_PER_INSTR: u32 = 2;
const BATCH: u32 = 8;

pub struct Psx {
    pub cpu: Cpu,
    pub bus: Bus,
    pub bios_loaded: bool,
    pending_exe: Option<Vec<u8>>,
    pub fast_boot: bool,
}

impl Psx {
    pub fn new() -> Self {
        Psx { cpu: Cpu::new(), bus: Bus::new(), bios_loaded: false, pending_exe: None, fast_boot: false }
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
        // Safety budget: ~2 frames worth of cycles.
        let mut budget: i64 = 1_200_000;
        while !self.bus.frame_done && budget > 0 {
            for _ in 0..BATCH {
                self.cpu.step(&mut self.bus);
            }
            self.bus.tick(BATCH * CYCLES_PER_INSTR);
            budget -= (BATCH * CYCLES_PER_INSTR) as i64;
            if self.cpu.pc == 0x8003_0000 && self.pending_exe.is_some() {
                self.inject_exe();
            }
        }
    }
}
