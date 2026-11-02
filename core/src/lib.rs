//! PS1 emulator core compiled to WebAssembly.

pub mod bus;
pub mod cdrom;
pub mod cpu;
pub mod gpu;
pub mod gte;
pub mod mdec;
pub mod pad;
pub mod psx;
pub mod spu;
pub mod timers;

use cdrom::{Disc, DiscReader, Track};
use pad::PadType;
use psx::Psx;
use wasm_bindgen::prelude::*;

/// Reads disc bytes through a JS callback: `(file, offset, length) => Uint8Array`.
struct JsReader {
    f: js_sys::Function,
}

impl DiscReader for JsReader {
    fn read(&mut self, file: u32, offset: u64, buf: &mut [u8]) -> bool {
        let r = self.f.call3(
            &JsValue::NULL,
            &JsValue::from(file),
            &JsValue::from(offset as f64),
            &JsValue::from(buf.len() as u32),
        );
        match r {
            Ok(v) => {
                let a = js_sys::Uint8Array::new(&v);
                let n = (a.length() as usize).min(buf.len());
                a.subarray(0, n as u32).copy_to(&mut buf[..n]);
                if n < buf.len() {
                    buf[n..].fill(0);
                }
                n > 0
            }
            Err(_) => false,
        }
    }
}

#[wasm_bindgen]
pub struct Emulator {
    psx: Psx,
    frame: Vec<u8>,
    fw: u32,
    fh: u32,
    audio: Vec<i16>,
}

#[wasm_bindgen]
impl Emulator {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Emulator {
        Emulator { psx: Psx::new(), frame: Vec::new(), fw: 0, fh: 0, audio: Vec::new() }
    }

    pub fn load_bios(&mut self, data: &[u8]) -> Result<(), JsValue> {
        self.psx.load_bios(data).map_err(|e| JsValue::from_str(&e))
    }

    pub fn bios_loaded(&self) -> bool {
        self.psx.bios_loaded
    }

    /// `tracks`: 8 u32 per track:
    /// [number, file, start_frame, length, sector_size, is_audio, offset_lo, offset_hi]
    pub fn insert_disc(&mut self, reader: js_sys::Function, tracks: &[u32]) -> Result<(), JsValue> {
        if tracks.is_empty() || tracks.len() % 8 != 0 {
            return Err(JsValue::from_str("Tabel track tidak valid"));
        }
        let list: Vec<Track> = tracks
            .chunks(8)
            .map(|t| Track {
                number: t[0] as u8,
                file: t[1],
                start: t[2],
                length: t[3],
                sector_size: t[4],
                audio: t[5] != 0,
                file_offset: t[6] as u64 | ((t[7] as u64) << 32),
            })
            .collect();
        let disc = Disc::new(list, Box::new(JsReader { f: reader }));
        self.psx.bus.cd.insert(Some(disc));
        Ok(())
    }

    pub fn eject(&mut self) {
        self.psx.bus.cd.insert(None);
    }

    pub fn disc_region(&self) -> String {
        match self.psx.bus.cd.disc.as_ref().map(|d| d.region) {
            Some(b'E') => "PAL (Eropa)".into(),
            Some(b'I') => "NTSC-J (Jepang)".into(),
            Some(_) => "NTSC-U (Amerika)".into(),
            None => "-".into(),
        }
    }

    pub fn load_exe(&mut self, data: &[u8]) -> Result<(), JsValue> {
        self.psx.load_exe(data).map_err(|e| JsValue::from_str(&e))
    }

    pub fn reset(&mut self) {
        self.psx.reset();
    }

    pub fn run_frame(&mut self) {
        self.psx.run_frame();
        let a = self.psx.bus.spu.take_audio();
        self.audio.clear();
        self.audio.extend_from_slice(&a);
    }

    /// Render the current display to an RGBA buffer. Returns false if size is zero.
    pub fn render(&mut self, full_vram: bool) -> bool {
        let (w, h) = self.psx.bus.gpu.render(&mut self.frame, full_vram);
        self.fw = w;
        self.fh = h;
        w > 0 && h > 0
    }

    pub fn frame_ptr(&self) -> *const u8 {
        self.frame.as_ptr()
    }
    pub fn frame_len(&self) -> usize {
        self.frame.len()
    }
    pub fn frame_width(&self) -> u32 {
        self.fw
    }
    pub fn frame_height(&self) -> u32 {
        self.fh
    }
    pub fn is_pal(&self) -> bool {
        self.psx.bus.gpu.pal
    }

    pub fn audio_ptr(&self) -> *const i16 {
        self.audio.as_ptr()
    }
    /// Number of i16 values (interleaved stereo).
    pub fn audio_len(&self) -> usize {
        self.audio.len()
    }

    // ---------------- input ----------------

    /// `kind`: 0 = none, 1 = digital, 2 = DualShock (analog)
    pub fn set_pad_type(&mut self, port: usize, kind: u32) {
        let p = &mut self.psx.bus.pad.pads[port & 1];
        p.kind = match kind {
            0 => PadType::None,
            1 => PadType::Digital,
            _ => PadType::DualShock,
        };
        if p.kind != PadType::DualShock {
            p.analog_mode = false;
        }
    }

    /// Bit set = pressed. Bit order: Select, L3, R3, Start, Up, Right, Down, Left,
    /// L2, R2, L1, R1, Triangle, Circle, Cross, Square.
    pub fn set_buttons(&mut self, port: usize, buttons: u16) {
        self.psx.bus.pad.pads[port & 1].buttons = buttons;
    }

    pub fn set_axes(&mut self, port: usize, lx: u8, ly: u8, rx: u8, ry: u8) {
        self.psx.bus.pad.pads[port & 1].axes = [rx, ry, lx, ly];
    }

    pub fn set_analog_mode(&mut self, port: usize, on: bool) {
        let p = &mut self.psx.bus.pad.pads[port & 1];
        if p.kind == PadType::DualShock {
            p.analog_mode = on;
        }
    }

    pub fn analog_mode(&self, port: usize) -> bool {
        self.psx.bus.pad.pads[port & 1].analog_mode
    }

    /// Rumble motors: (small << 8) | large
    pub fn rumble(&self, port: usize) -> u32 {
        let r = self.psx.bus.pad.pads[port & 1].rumble;
        ((r.0 as u32) << 8) | r.1 as u32
    }

    // ---------------- memory cards ----------------

    pub fn memcard_get(&self, slot: usize) -> Vec<u8> {
        self.psx.bus.pad.cards[slot & 1].data.clone()
    }

    pub fn memcard_set(&mut self, slot: usize, data: &[u8]) -> Result<(), JsValue> {
        if data.len() != pad::MEMCARD_SIZE {
            return Err(JsValue::from_str("Ukuran memory card harus 131072 byte"));
        }
        let c = &mut self.psx.bus.pad.cards[slot & 1];
        c.data.copy_from_slice(data);
        c.dirty = false;
        Ok(())
    }

    pub fn memcard_format(&mut self, slot: usize) {
        self.psx.bus.pad.cards[slot & 1].format();
    }

    pub fn memcard_set_present(&mut self, slot: usize, present: bool) {
        self.psx.bus.pad.cards[slot & 1].present = present;
    }

    /// Returns true once after the card was written by the game.
    pub fn memcard_take_dirty(&mut self, slot: usize) -> bool {
        std::mem::take(&mut self.psx.bus.pad.cards[slot & 1].dirty)
    }

    // ---------------- options ----------------

    pub fn set_dither(&mut self, on: bool) {
        self.psx.bus.gpu.opt_dither = on;
    }
    pub fn set_volume(&mut self, v: f32) {
        self.psx.bus.spu.master_volume = v.clamp(0.0, 2.0);
    }
    pub fn set_reverb(&mut self, on: bool) {
        self.psx.bus.spu.reverb_enabled = on;
    }
    pub fn set_interpolation(&mut self, on: bool) {
        self.psx.bus.spu.interp = on;
    }

    /// Raw VRAM (1024x512, 16-bit) for GPU-side presentation / debugging.
    pub fn vram_ptr(&self) -> *const u16 {
        self.psx.bus.gpu.vram.as_ptr()
    }
}

impl Default for Emulator {
    fn default() -> Self {
        Self::new()
    }
}
