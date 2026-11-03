//! PS1 emulator core compiled to WebAssembly.

pub mod bus;
pub mod cdrom;
pub mod cheats;
pub mod cpu;
pub mod disc_image;
pub mod ecm;
pub mod gpu;
pub mod gte;
pub mod mdec;
pub mod pad;
pub mod psx;
pub mod rewind;
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
    rewind: rewind::Rewind,
    /// ECM files decoded for the next `insert_disc` (file index -> decoder)
    ecm_pending: Vec<(u32, ecm::EcmFile)>,
}

#[wasm_bindgen]
impl Emulator {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Emulator {
        Emulator { psx: Psx::new(), frame: Vec::new(), fw: 0, fh: 0, audio: Vec::new(), rewind: Default::default(), ecm_pending: Vec::new() }
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
        let raw = ecm::SharedReader(std::rc::Rc::new(std::cell::RefCell::new(Box::new(JsReader { f: reader }) as Box<dyn DiscReader>)));
        let pending = std::mem::take(&mut self.ecm_pending);
        let r: Box<dyn DiscReader> = if pending.is_empty() { Box::new(raw) } else { Box::new(ecm::MuxReader { raw, ecm: pending }) };
        let disc = Disc::new(list, r);
        self.psx.bus.cd.insert(Some(disc));
        Ok(())
    }

    /// Scan an ECM file (index `file` of the reader) before `insert_disc`; returns its decoded size.
    pub fn prepare_ecm(&mut self, reader: js_sys::Function, file: u32, size: f64) -> Result<f64, JsValue> {
        let r = ecm::SharedReader(std::rc::Rc::new(std::cell::RefCell::new(Box::new(JsReader { f: reader }) as Box<dyn DiscReader>)));
        let e = ecm::EcmFile::open(r, file, size as u64).map_err(|e| JsValue::from_str(&e))?;
        let n = e.decoded_size as f64;
        self.ecm_pending.retain(|(f, _)| *f != file);
        self.ecm_pending.push((file, e));
        Ok(n)
    }

    /// First bytes of a prepared ECM file's decoded data (sector-size detection).
    pub fn ecm_peek(&mut self, file: u32, len: u32) -> Vec<u8> {
        let mut out = vec![0u8; len as usize];
        if let Some((_, e)) = self.ecm_pending.iter_mut().find(|(f, _)| *f == file) {
            e.read_at(0, &mut out);
        }
        out
    }

    /// Insert a compressed image. `kind`: "pbp" | "chd". `size` is the file size in bytes.
    /// For multi-disc PBP files `disc` selects the disc (0-based). Returns the disc count.
    pub fn insert_disc_image(&mut self, reader: js_sys::Function, kind: &str, size: f64, disc: u32) -> Result<u32, JsValue> {
        let r: Box<dyn DiscReader> = Box::new(JsReader { f: reader });
        let img = match kind {
            "pbp" => disc_image::open_pbp(r, size as u64, disc),
            "chd" => disc_image::open_chd(r, size as u64),
            "ecm" => {
                // single .iso.ecm / .bin.ecm: one data track
                let shared = ecm::SharedReader(std::rc::Rc::new(std::cell::RefCell::new(r)));
                let mut e = ecm::EcmFile::open(shared.clone(), 0, size as u64).map_err(|e| JsValue::from_str(&e))?;
                let mut head = [0u8; 16];
                e.read_at(0, &mut head);
                let raw = head[0] == 0 && head[1..11].iter().all(|&b| b == 0xFF) && head[11] == 0 && e.decoded_size % 2352 == 0;
                let ss: u32 = if raw { 2352 } else { 2048 };
                let track = Track { number: 1, file: 0, start: 150, length: (e.decoded_size / ss as u64) as u32, sector_size: ss, audio: false, file_offset: 0 };
                let mux = ecm::MuxReader { raw: shared, ecm: vec![(0, e)] };
                Ok(disc_image::OpenedImage { tracks: vec![track], reader: Box::new(mux), disc_count: 1 })
            }
            _ => Err(format!("Format {kind} tidak dikenal")),
        }
        .map_err(|e| JsValue::from_str(&e))?;
        let count = img.disc_count;
        let d = Disc::new(img.tracks, img.reader);
        self.psx.bus.cd.insert(Some(d));
        Ok(count)
    }

    /// Swap discs (multi-disc games): the drive reports an opened lid once.
    pub fn swap_disc_image(&mut self, reader: js_sys::Function, kind: &str, size: f64, disc: u32) -> Result<u32, JsValue> {
        let n = self.insert_disc_image(reader, kind, size, disc)?;
        self.psx.bus.cd.open_lid();
        Ok(n)
    }

    /// Report an opened lid once (use after inserting a different disc).
    pub fn open_lid(&mut self) {
        self.psx.bus.cd.open_lid();
    }

    pub fn eject(&mut self) {
        self.psx.bus.cd.insert(None);
    }

    /// Serial of the inserted disc from SYSTEM.CNF ("" if unknown).
    pub fn disc_serial(&mut self) -> String {
        self.psx.bus.cd.disc.as_mut().and_then(|d| d.serial()).unwrap_or_default()
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

    pub fn save_state(&mut self) -> Vec<u8> {
        self.psx.bus.gpu_flush_sync();
        self.psx.save_state()
    }

    pub fn load_state(&mut self, data: &[u8]) -> Result<(), JsValue> {
        self.psx.load_state(data).map_err(|e| JsValue::from_str(&e))?;
        self.rewind.clear();
        if self.psx.bus.gpu.threaded {
            self.psx.bus.gpu.rec_state();
        }
        Ok(())
    }

    pub fn reset(&mut self) {
        self.psx.reset();
        self.rewind.clear();
        if self.psx.bus.gpu.threaded {
            self.psx.bus.gpu.rec_state();
        }
    }

    /// Move rasterization to a GPU thread. `sync(out: Uint32Array, vram: Uint16Array)` must
    /// deliver `out` (command records ending with a SYNC record) to that thread, wait for it,
    /// and copy its VRAM into `vram`.
    pub fn enable_gpu_thread(&mut self, sync: js_sys::Function) {
        self.psx.bus.gpu_sync = Some(Box::new(move |out: &[u32], vram: &mut [u16]| {
            // SAFETY: the views are only used during this synchronous call; no wasm allocation happens meanwhile.
            let o = unsafe { js_sys::Uint32Array::view(out) };
            let v = unsafe { js_sys::Uint16Array::view_mut_raw(vram.as_mut_ptr(), vram.len()) };
            let _ = sync.call2(&JsValue::NULL, &o, &v);
        }));
        self.psx.bus.gpu.threaded = true;
        self.psx.bus.gpu.rec_state();
    }

    pub fn gpu_threaded(&self) -> bool {
        self.psx.bus.gpu.threaded
    }

    /// Recorded GPU command stream (threaded mode). Read it via `gpu_out_ptr/len`, then clear.
    pub fn gpu_out_ptr(&self) -> *const u32 {
        self.psx.bus.gpu.out.as_ptr()
    }
    pub fn gpu_out_len(&self) -> usize {
        self.psx.bus.gpu.out.len()
    }
    pub fn gpu_out_clear(&mut self) {
        let _ = self.psx.bus.gpu.take_out();
    }

    pub fn run_frame(&mut self) {
        self.psx.run_frame();
        if self.rewind.due() {
            let s = self.save_state();
            self.rewind.push(s);
        }
        let a = self.psx.bus.spu.take_audio();
        self.audio.clear();
        self.audio.extend_from_slice(&a);
    }

    /// Render the current display to an RGBA buffer. Returns false if size is zero.
    pub fn render(&mut self, full_vram: bool) -> bool {
        self.psx.bus.gpu_flush_sync();
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

    /// Active cheats (cheats separated by a `---` line). Returns errors ("" if none).
    pub fn set_cheats(&mut self, text: &str) -> String {
        self.psx.cheats.set(text)
    }

    /// Check one cheat's codes for the editor ("" if valid).
    pub fn validate_cheat(&self, text: &str) -> String {
        cheats::validate(text)
    }

    /// Rewind: snapshot every `interval` frames, keep up to `max` snapshots.
    pub fn set_rewind(&mut self, enabled: bool, interval: u32, max: u32) {
        if !enabled {
            self.rewind.clear();
        }
        self.rewind.enabled = enabled;
        self.rewind.interval = interval.max(1);
        self.rewind.max = max.max(1) as usize;
    }

    /// Go back one snapshot. Returns false when the buffer is empty.
    pub fn rewind_step(&mut self) -> bool {
        let state = match self.rewind.pop() {
            Some(s) => s.to_vec(),
            None => return false,
        };
        if self.psx.load_state(&state).is_err() {
            return false;
        }
        if self.psx.bus.gpu.threaded {
            self.psx.bus.gpu.rec_state();
        }
        let _ = self.psx.bus.spu.take_audio();
        true
    }

    /// [snapshots, bytes]
    pub fn rewind_info(&self) -> Vec<u32> {
        vec![self.rewind.len() as u32, self.rewind.bytes() as u32]
    }

    pub fn set_fast_boot(&mut self, on: bool) {
        self.psx.fast_boot = on;
    }

    pub fn set_dither(&mut self, on: bool) {
        self.psx.bus.gpu.opt_dither = on;
        if self.psx.bus.gpu.threaded {
            self.psx.bus.gpu.rec(gpu::REC_OPT, &[on as u32]);
        }
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

    /// Per-frame counters since last call:
    /// [gpu pixels (approx), gpu commands, cd sectors, cpu exceptions/irqs]
    pub fn take_stats(&mut self) -> Vec<u32> {
        let b = &mut self.psx.bus;
        let r = vec![
            std::mem::take(&mut b.gpu.stat_pixels) as u32,
            std::mem::take(&mut b.gpu.stat_cmds),
            std::mem::take(&mut b.cd.stat_sectors),
            std::mem::take(&mut self.psx.cpu.stat_exceptions),
        ];
        r
    }

    /// Human-readable diagnostics for bug reports.
    pub fn diagnostics(&mut self) -> String {
        let p = &mut self.psx;
        let mut s = String::new();
        s += &format!("pc={:08X} sr={:08X} bios_loaded={} fast_boot={}\n", p.cpu.pc, p.cpu.sr, p.bios_loaded, p.fast_boot);
        s += &format!("i_stat={:03X} i_mask={:03X} cycles={}\n", p.bus.i_stat, p.bus.i_mask, p.bus.cycles);
        let g = &p.bus.gpu;
        let (w, h) = g.display_size();
        s += &format!(
            "gpu: disp_on={} {}x{} at ({},{}) depth24={} interlace={} pal={} threaded={}\n",
            g.disp_on, w, h, g.disp_x, g.disp_y, g.depth24, g.interlace, g.pal, g.threaded
        );
        s += &format!("cdrom: {}\n", p.bus.cd.debug_state());
        match p.bus.cd.disc.as_mut() {
            Some(d) => s += &d.debug_report(),
            None => s += "disc: none\n",
        }
        s
    }

    /// Display description for GPU-side presentation: [x, y, width, height, depth24, enabled].
    pub fn display_info(&self) -> Vec<u32> {
        let g = &self.psx.bus.gpu;
        let (w, h) = g.display_size();
        vec![g.disp_x, g.disp_y, w, h, g.depth24 as u32, g.disp_on as u32]
    }

    /// Raw VRAM (1024x512, 16-bit) for GPU-side presentation / debugging.
    pub fn vram_ptr(&self) -> *const u16 {
        self.psx.bus.gpu.vram.as_ptr()
    }
}

/// GPU thread side: owns the authoritative VRAM and rasterizes recorded commands.
#[wasm_bindgen]
pub struct GpuBackend {
    gpu: gpu::Gpu,
    queue: Vec<u32>,
    pos: usize,
    frame: Vec<u8>,
    fw: u32,
    fh: u32,
    present: bool,
    hw_on: bool,
}

#[wasm_bindgen]
impl GpuBackend {
    #[wasm_bindgen(constructor)]
    pub fn new() -> GpuBackend {
        GpuBackend { gpu: gpu::Gpu::new(), queue: Vec::new(), pos: 0, frame: Vec::new(), fw: 0, fh: 0, present: false, hw_on: false }
    }

    /// Append command records received from the CPU side.
    pub fn feed(&mut self, data: &[u32]) {
        if self.pos > 0 && self.pos == self.queue.len() {
            self.queue.clear();
            self.pos = 0;
        }
        self.queue.extend_from_slice(data);
    }

    /// Process records until an event: 0 = need more data, 1 = frame end, 2 = sync request.
    pub fn step(&mut self) -> u32 {
        loop {
            if self.pos >= self.queue.len() {
                self.queue.clear();
                self.pos = 0;
                return 0;
            }
            let hdr = self.queue[self.pos];
            let len = (hdr & 0xFF_FFFF) as usize;
            if self.pos + 1 + len > self.queue.len() {
                // incomplete record: keep the tail for the next feed
                self.queue.drain(..self.pos);
                self.pos = 0;
                return 0;
            }
            let start = self.pos + 1;
            self.pos = start + len;
            match hdr >> 24 {
                gpu::REC_GP0 => {
                    for k in start..start + len {
                        let w = self.queue[k];
                        self.gpu.gp0(w);
                    }
                }
                gpu::REC_GP1 => {
                    let w = self.queue[start];
                    self.gpu.gp1(w);
                }
                gpu::REC_FRAME => {
                    self.present = len == 0 || self.queue[start] != 0;
                    return 1;
                }
                gpu::REC_SYNC => return 2,
                gpu::REC_OPT => self.gpu.opt_dither = self.queue[start] != 0,
                gpu::REC_STATE => {
                    let n = self.queue[start] as usize;
                    let bytes: Vec<u8> = self.queue[start + 1..start + len].iter().flat_map(|w| w.to_le_bytes()).take(n).collect();
                    if let Ok(g) = bincode::deserialize::<gpu::Gpu>(&bytes) {
                        let dither = self.gpu.opt_dither;
                        self.gpu = g;
                        self.gpu.opt_dither = dither;
                        self.gpu.threaded = false;
                        if self.hw_on {
                            self.gpu.hw = Some(Vec::new());
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// Record a draw list for the WebGL2 high-resolution renderer.
    pub fn hw_enable(&mut self, on: bool) {
        self.hw_on = on;
        self.gpu.hw = if on { Some(Vec::new()) } else { None };
    }
    pub fn hw_ptr(&self) -> *const u32 {
        self.gpu.hw.as_ref().map(|v| v.as_ptr()).unwrap_or(std::ptr::null())
    }
    pub fn hw_len(&self) -> usize {
        self.gpu.hw.as_ref().map(|v| v.len()).unwrap_or(0)
    }
    pub fn hw_clear(&mut self) {
        if let Some(v) = self.gpu.hw.as_mut() {
            v.clear();
        }
    }

    /// Whether the last frame-end record asked for the frame to be shown.
    pub fn present(&self) -> bool {
        self.present
    }

    pub fn vram_ptr(&self) -> *const u16 {
        self.gpu.vram.as_ptr()
    }

    pub fn display_info(&self) -> Vec<u32> {
        let g = &self.gpu;
        let (w, h) = g.display_size();
        vec![g.disp_x, g.disp_y, w, h, g.depth24 as u32, g.disp_on as u32]
    }

    pub fn render(&mut self, full_vram: bool) -> bool {
        let (w, h) = self.gpu.render(&mut self.frame, full_vram);
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
}

impl GpuBackend {
    /// Native access to the authoritative VRAM (tests).
    pub fn vram(&self) -> &[u16] {
        &self.gpu.vram
    }
}

impl Default for GpuBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl Default for Emulator {
    fn default() -> Self {
        Self::new()
    }
}
