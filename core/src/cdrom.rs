//! CD-ROM controller, disc image access, CD-DA and XA-ADPCM audio.

use std::collections::VecDeque;

pub const SECTOR_RAW: usize = 2352;
const CPU_HZ: i32 = 33_868_800;
const SECTOR_1X: i32 = CPU_HZ / 75;
const ACK_DELAY: i32 = 25_000;
const SEEK_DELAY: i32 = 120_000;

/// Source of disc bytes (implemented by the JS bridge, or by files in native tests).
pub trait DiscReader {
    fn read(&mut self, file: u32, offset: u64, buf: &mut [u8]) -> bool;
}

#[derive(Clone, Debug)]
pub struct Track {
    pub number: u8,
    pub file: u32,
    /// Absolute frame (MSF 00:02:00 == 150) of INDEX 01.
    pub start: u32,
    pub length: u32,
    pub sector_size: u32,
    pub audio: bool,
    pub file_offset: u64,
}

pub struct Disc {
    pub tracks: Vec<Track>,
    reader: Box<dyn DiscReader>,
    pub region: u8,
}

fn bcd(v: u32) -> u8 {
    (((v / 10) << 4) | (v % 10)) as u8
}
fn unbcd(v: u8) -> u32 {
    ((v >> 4) as u32) * 10 + (v & 0xF) as u32
}
fn msf(frame: u32) -> (u8, u8, u8) {
    (bcd(frame / 75 / 60), bcd((frame / 75) % 60), bcd(frame % 75))
}

impl Disc {
    pub fn new(tracks: Vec<Track>, reader: Box<dyn DiscReader>) -> Self {
        let mut d = Disc { tracks, reader, region: b'A' };
        d.detect_region();
        d
    }

    pub fn end_frame(&self) -> u32 {
        self.tracks.last().map(|t| t.start + t.length).unwrap_or(150)
    }

    pub fn track_at(&self, frame: u32) -> Option<&Track> {
        self.tracks.iter().find(|t| frame < t.start + t.length)
    }

    /// Read a full raw sector; sectors stored as 2048 bytes get a synthesized header.
    pub fn read_sector(&mut self, frame: u32, buf: &mut [u8; SECTOR_RAW]) -> bool {
        let t = match self.track_at(frame) {
            Some(t) => t.clone(),
            None => {
                buf.fill(0);
                return false;
            }
        };
        if frame < t.start {
            // pregap: silence / zeroed data
            buf.fill(0);
            return true;
        }
        let rel = (frame - t.start) as u64;
        let off = t.file_offset + rel * t.sector_size as u64;
        if t.sector_size as usize == SECTOR_RAW {
            return self.reader.read(t.file, off, &mut buf[..]);
        }
        // 2048-byte ISO: build sync + header + Mode2/Form1 subheader.
        buf.fill(0);
        buf[1..11].fill(0xFF);
        let (m, s, f) = msf(frame);
        buf[12] = m;
        buf[13] = s;
        buf[14] = f;
        buf[15] = 2;
        buf[18] = 0x08;
        buf[22] = 0x08;
        let ss = t.sector_size as usize;
        let n = ss.min(2048);
        self.reader.read(t.file, off, &mut buf[24..24 + n])
    }

    fn detect_region(&mut self) {
        let mut buf = [0u8; SECTOR_RAW];
        if !self.read_sector(150 + 4, &mut buf) {
            return;
        }
        let text = &buf[24..24 + 128];
        let find = |pat: &[u8]| text.windows(pat.len()).any(|w| w == pat);
        self.region = if find(b"Europe") || find(b"Euro") {
            b'E'
        } else if find(b"Inc.") {
            b'I'
        } else {
            b'A'
        };
    }
}

struct Pending {
    delay: i32,
    int: u8,
    resp: Vec<u8>,
}

#[derive(Default, Clone, Copy)]
struct XaState {
    old: [i32; 2],
    older: [i32; 2],
}

pub struct CdRom {
    pub disc: Option<Disc>,
    index: u8,
    params: VecDeque<u8>,
    resp: VecDeque<u8>,
    data: Vec<u8>,
    data_pos: usize,
    irq_flag: u8,
    irq_enable: u8,
    queue: VecDeque<Pending>,
    // drive state
    motor: bool,
    reading: bool,
    playing: bool,
    seeking: bool,
    mode: u8,
    setloc: u32,
    setloc_pending: bool,
    loc: u32,
    read_timer: i32,
    sector: Box<[u8; SECTOR_RAW]>,
    last_header: [u8; 8],
    filter_file: u8,
    filter_chan: u8,
    muted: bool,
    xa_muted: bool,
    shell_open: bool,
    // audio
    pub audio: VecDeque<(i16, i16)>,
    vol: [u8; 4], // LL, LR, RL, RR
    vol_pending: [u8; 4],
    xa: XaState,
    xa_resample: f32,
    xa_last: (i16, i16),
    track_end: u32,
    pub stat_sectors: u32,
}

const XA_POS: [i32; 4] = [0, 60, 115, 98];
const XA_NEG: [i32; 4] = [0, 0, -52, -55];

impl CdRom {
    pub fn new() -> Self {
        CdRom {
            disc: None,
            index: 0,
            params: VecDeque::new(),
            resp: VecDeque::new(),
            data: Vec::new(),
            data_pos: 0,
            irq_flag: 0,
            irq_enable: 0,
            queue: VecDeque::new(),
            motor: false,
            reading: false,
            playing: false,
            seeking: false,
            mode: 0,
            setloc: 150,
            setloc_pending: false,
            loc: 150,
            read_timer: 0,
            sector: Box::new([0; SECTOR_RAW]),
            last_header: [0; 8],
            filter_file: 0,
            filter_chan: 0,
            muted: false,
            xa_muted: false,
            shell_open: false,
            audio: VecDeque::new(),
            vol: [0x80, 0, 0, 0x80],
            vol_pending: [0x80, 0, 0, 0x80],
            xa: XaState::default(),
            xa_resample: 0.0,
            xa_last: (0, 0),
            track_end: 0,
            stat_sectors: 0,
        }
    }

    pub fn reset(&mut self) {
        let disc = self.disc.take();
        *self = CdRom::new();
        self.disc = disc;
    }

    pub fn insert(&mut self, disc: Option<Disc>) {
        self.disc = disc;
        self.reading = false;
        self.playing = false;
        self.motor = self.disc.is_some();
    }

    fn stat(&self) -> u8 {
        let mut s = 0;
        if self.motor {
            s |= 0x02;
        }
        if self.shell_open || self.disc.is_none() {
            s |= 0x10;
        }
        if self.reading {
            s |= 0x20;
        }
        if self.seeking {
            s |= 0x40;
        }
        if self.playing {
            s |= 0x80;
        }
        s
    }

    fn push(&mut self, delay: i32, int: u8, resp: Vec<u8>) {
        self.queue.push_back(Pending { delay, int, resp });
    }

    // ---------------- register interface ----------------

    pub fn read(&mut self, reg: u32) -> u8 {
        match reg {
            0 => {
                let mut s = self.index;
                s |= (self.params.is_empty() as u8) << 3;
                s |= ((self.params.len() < 16) as u8) << 4;
                s |= (!self.resp.is_empty() as u8) << 5;
                s |= ((self.data_pos < self.data.len()) as u8) << 6;
                let busy = self.queue.front().map(|p| p.int == 3).unwrap_or(false);
                s |= (busy as u8) << 7;
                s
            }
            1 => self.resp.pop_front().unwrap_or(0),
            2 => self.data_byte(),
            _ => match self.index & 1 {
                0 => self.irq_enable | 0xE0,
                _ => self.irq_flag | 0xE0,
            },
        }
    }

    fn data_byte(&mut self) -> u8 {
        if self.data_pos < self.data.len() {
            let b = self.data[self.data_pos];
            self.data_pos += 1;
            b
        } else {
            0
        }
    }

    pub fn dma_read_word(&mut self) -> u32 {
        let b0 = self.data_byte() as u32;
        let b1 = self.data_byte() as u32;
        let b2 = self.data_byte() as u32;
        let b3 = self.data_byte() as u32;
        b0 | (b1 << 8) | (b2 << 16) | (b3 << 24)
    }

    pub fn write(&mut self, reg: u32, v: u8) {
        match (reg, self.index) {
            (0, _) => self.index = v & 3,
            (1, 0) => self.command(v),
            (1, 3) => self.vol_pending[3] = v,
            (2, 0) => {
                if self.params.len() < 16 {
                    self.params.push_back(v)
                }
            }
            (2, 1) => self.irq_enable = v & 0x1F,
            (2, 2) => self.vol_pending[0] = v,
            (2, 3) => self.vol_pending[2] = v,
            (3, 0) => {
                if v & 0x80 != 0 {
                    if self.data_pos >= self.data.len() {
                        self.load_data_fifo();
                    }
                } else {
                    self.data.clear();
                    self.data_pos = 0;
                }
            }
            (3, 1) => {
                self.irq_flag &= !(v & 0x1F);
                if v & 0x40 != 0 {
                    self.params.clear();
                }
            }
            (3, 2) => self.vol_pending[1] = v,
            (3, 3) => {
                self.xa_muted = v & 1 != 0;
                if v & 0x20 != 0 {
                    self.vol = self.vol_pending;
                }
            }
            _ => {}
        }
    }

    fn load_data_fifo(&mut self) {
        let (start, len) = if self.mode & 0x20 != 0 { (12, 2340) } else { (24, 2048) };
        self.data.clear();
        self.data.extend_from_slice(&self.sector[start..start + len]);
        self.data_pos = 0;
    }

    fn param(&mut self) -> u8 {
        self.params.pop_front().unwrap_or(0)
    }

    fn command(&mut self, cmd: u8) {
        self.resp.clear();
        let no_disc = self.disc.is_none();
        let st = self.stat();
        match cmd {
            0x01 => {
                self.push(ACK_DELAY, 3, vec![st]);
                self.shell_open = false;
            }
            0x02 => {
                let m = unbcd(self.param());
                let s = unbcd(self.param());
                let f = unbcd(self.param());
                self.setloc = (m * 60 + s) * 75 + f;
                self.setloc_pending = true;
                self.push(ACK_DELAY, 3, vec![st]);
            }
            0x03 => {
                // Play (CD-DA)
                let track = if !self.params.is_empty() { unbcd(self.param()) } else { 0 };
                self.push(ACK_DELAY, 3, vec![st]);
                if let Some(d) = &self.disc {
                    if track > 0 {
                        if let Some(t) = d.tracks.iter().find(|t| t.number as u32 == track) {
                            self.loc = t.start;
                        }
                    } else if self.setloc_pending {
                        self.loc = self.setloc;
                    }
                    self.setloc_pending = false;
                    self.track_end = d.track_at(self.loc).map(|t| t.start + t.length).unwrap_or(0);
                    self.playing = true;
                    self.reading = false;
                    self.motor = true;
                    self.read_timer = SEEK_DELAY;
                }
            }
            0x04 | 0x05 => self.push(ACK_DELAY, 3, vec![st]),
            0x06 | 0x1B => {
                if no_disc {
                    self.error(0x80);
                    return;
                }
                self.push(ACK_DELAY, 3, vec![st]);
                if self.setloc_pending {
                    self.loc = self.setloc;
                    self.setloc_pending = false;
                    self.read_timer = SEEK_DELAY + self.sector_time();
                } else {
                    self.read_timer = self.sector_time();
                }
                self.reading = true;
                self.playing = false;
                self.motor = true;
            }
            0x07 => {
                self.push(ACK_DELAY, 3, vec![st]);
                self.motor = true;
                let s2 = self.stat();
                self.push(ACK_DELAY * 4, 2, vec![s2]);
            }
            0x08 => {
                self.push(ACK_DELAY, 3, vec![st]);
                self.reading = false;
                self.playing = false;
                self.motor = false;
                let s2 = self.stat();
                self.push(ACK_DELAY * 8, 2, vec![s2]);
            }
            0x09 => {
                self.push(ACK_DELAY, 3, vec![st]);
                let was = self.reading || self.playing;
                self.reading = false;
                self.playing = false;
                let s2 = self.stat();
                self.push(if was { ACK_DELAY * 4 } else { 7000 }, 2, vec![s2]);
            }
            0x0A => {
                self.push(ACK_DELAY, 3, vec![st]);
                self.mode = 0;
                self.reading = false;
                self.playing = false;
                self.motor = !no_disc;
                self.setloc_pending = false;
                let s2 = self.stat();
                self.push(ACK_DELAY * 2, 2, vec![s2]);
            }
            0x0B => {
                self.muted = true;
                self.push(ACK_DELAY, 3, vec![st]);
            }
            0x0C => {
                self.muted = false;
                self.push(ACK_DELAY, 3, vec![st]);
            }
            0x0D => {
                self.filter_file = self.param();
                self.filter_chan = self.param();
                self.push(ACK_DELAY, 3, vec![st]);
            }
            0x0E => {
                self.mode = self.param();
                self.push(ACK_DELAY, 3, vec![st]);
            }
            0x0F => {
                let r = vec![st, self.mode, 0, self.filter_file, self.filter_chan];
                self.push(ACK_DELAY, 3, r);
            }
            0x10 => {
                let h = self.last_header;
                self.push(ACK_DELAY, 3, h.to_vec());
            }
            0x11 => {
                let (tn, rel) = match &self.disc {
                    Some(d) => match d.track_at(self.loc) {
                        Some(t) => (t.number as u32, self.loc as i64 - t.start as i64),
                        None => (1, 0),
                    },
                    None => (1, 0),
                };
                let (rm, rs, rf) = msf(rel.unsigned_abs() as u32);
                let (am, as_, af) = msf(self.loc);
                self.push(ACK_DELAY, 3, vec![bcd(tn), 1, rm, rs, rf, am, as_, af]);
            }
            0x12 => {
                self.push(ACK_DELAY, 3, vec![st]);
                self.push(ACK_DELAY * 4, 2, vec![st]);
            }
            0x13 => {
                let last = self.disc.as_ref().map(|d| d.tracks.len() as u32).unwrap_or(1);
                self.push(ACK_DELAY, 3, vec![st, 1, bcd(last)]);
            }
            0x14 => {
                let t = unbcd(self.param());
                let frame = match &self.disc {
                    Some(d) => {
                        if t == 0 {
                            d.end_frame()
                        } else {
                            d.tracks.iter().find(|x| x.number as u32 == t).map(|x| x.start).unwrap_or(150)
                        }
                    }
                    None => 0,
                };
                let (m, s, _) = msf(frame);
                self.push(ACK_DELAY, 3, vec![st, m, s]);
            }
            0x15 | 0x16 => {
                self.push(ACK_DELAY, 3, vec![st]);
                self.loc = self.setloc;
                self.setloc_pending = false;
                self.reading = false;
                self.playing = false;
                self.motor = true;
                let s2 = self.stat();
                self.push(SEEK_DELAY, 2, vec![s2]);
            }
            0x19 => {
                let sub = self.param();
                match sub {
                    0x20 => self.push(ACK_DELAY, 3, vec![0x94, 0x09, 0x19, 0xC0]),
                    0x04 | 0x05 => self.push(ACK_DELAY, 3, vec![st, 0, 0]),
                    _ => self.push(ACK_DELAY, 3, vec![0x11, 0x10]),
                }
            }
            0x1A => {
                if no_disc {
                    self.push(ACK_DELAY, 3, vec![st]);
                    self.push(ACK_DELAY * 2, 5, vec![0x08, 0x40, 0, 0, 0, 0, 0, 0]);
                    return;
                }
                self.push(ACK_DELAY, 3, vec![st]);
                let region = self.disc.as_ref().map(|d| d.region).unwrap_or(b'A');
                let audio_only = self.disc.as_ref().map(|d| d.tracks.iter().all(|t| t.audio)).unwrap_or(false);
                if audio_only {
                    self.push(ACK_DELAY * 2, 5, vec![0x0A, 0x90, 0, 0, 0, 0, 0, 0]);
                } else {
                    let tag = match region {
                        b'E' => *b"SCEE",
                        b'I' => *b"SCEI",
                        _ => *b"SCEA",
                    };
                    self.push(ACK_DELAY * 2, 2, vec![0x02, 0x00, 0x20, 0x00, tag[0], tag[1], tag[2], tag[3]]);
                }
            }
            0x1C => {
                self.reading = false;
                self.playing = false;
                self.mode = 0;
                self.push(ACK_DELAY, 3, vec![st]);
            }
            0x1E => {
                self.push(ACK_DELAY, 3, vec![st]);
                self.push(SEEK_DELAY * 4, 2, vec![st]);
            }
            _ => self.error(0x40),
        }
        self.params.clear();
    }

    fn error(&mut self, code: u8) {
        let st = self.stat() | 1;
        self.push(ACK_DELAY, 5, vec![st, code]);
        self.params.clear();
    }

    fn sector_time(&self) -> i32 {
        if self.mode & 0x80 != 0 {
            SECTOR_1X / 2
        } else {
            SECTOR_1X
        }
    }

    // ---------------- timing ----------------

    /// Returns true if an interrupt edge was raised.
    pub fn tick(&mut self, cyc: u32) -> bool {
        let c = cyc as i32;
        let mut raised = false;
        if let Some(p) = self.queue.front_mut() {
            p.delay -= c;
            if p.delay <= 0 && self.irq_flag == 0 {
                let p = self.queue.pop_front().unwrap();
                self.resp.clear();
                self.resp.extend(p.resp);
                raised |= self.set_irq(p.int);
            }
        }
        if self.reading || self.playing {
            self.read_timer -= c;
            if self.read_timer <= 0 {
                self.read_timer += self.sector_time();
                if self.playing {
                    self.play_sector();
                } else {
                    raised |= self.read_next_sector();
                }
            }
        }
        raised
    }

    fn set_irq(&mut self, int: u8) -> bool {
        self.irq_flag = int;
        self.irq_flag & self.irq_enable & 0x1F != 0
    }

    fn read_next_sector(&mut self) -> bool {
        self.stat_sectors += 1;
        let loc = self.loc;
        let ok = match self.disc.as_mut() {
            Some(d) => d.read_sector(loc, &mut self.sector),
            None => false,
        };
        self.loc += 1;
        if !ok {
            return false;
        }
        let s = &self.sector;
        self.last_header = [s[12], s[13], s[14], s[15], s[16], s[17], s[18], s[19]];
        let submode = s[18];
        let is_form2_audio = s[15] == 2 && submode & 0x04 != 0 && submode & 0x20 != 0;

        if self.mode & 0x40 != 0 && is_form2_audio {
            let filt_ok = self.mode & 0x08 == 0 || (s[16] == self.filter_file && s[17] == self.filter_chan);
            if filt_ok {
                self.decode_xa();
            }
            return false; // audio sectors are not delivered to the CPU
        }
        if self.mode & 0x08 != 0 && s[15] == 2 && (s[16] != self.filter_file || s[17] != self.filter_chan) && submode & 0x04 != 0 {
            return false;
        }
        if self.irq_flag != 0 {
            // CPU hasn't acknowledged the previous sector: drop this one (buffer overrun).
            return false;
        }
        self.resp.clear();
        self.resp.push_back(self.stat());
        self.set_irq(1)
    }

    fn play_sector(&mut self) {
        self.stat_sectors += 1;
        if self.track_end != 0 && self.loc >= self.track_end {
            self.playing = false;
            return;
        }
        let loc = self.loc;
        let ok = match self.disc.as_mut() {
            Some(d) => {
                let audio = d.track_at(loc).map(|t| t.audio).unwrap_or(false);
                audio && d.read_sector(loc, &mut self.sector)
            }
            None => false,
        };
        self.loc += 1;
        if !ok || self.muted {
            return;
        }
        for i in 0..588 {
            let l = i16::from_le_bytes([self.sector[i * 4], self.sector[i * 4 + 1]]);
            let r = i16::from_le_bytes([self.sector[i * 4 + 2], self.sector[i * 4 + 3]]);
            self.push_audio(l, r);
        }
    }

    fn push_audio(&mut self, l: i16, r: i16) {
        if self.audio.len() > 44100 / 2 {
            self.audio.pop_front();
        }
        let (l, r) = (l as i32, r as i32);
        let v = &self.vol;
        let ol = ((l * v[0] as i32) >> 7) + ((r * v[2] as i32) >> 7);
        let or = ((r * v[3] as i32) >> 7) + ((l * v[1] as i32) >> 7);
        self.audio.push_back((ol.clamp(-32768, 32767) as i16, or.clamp(-32768, 32767) as i16));
    }

    fn decode_xa(&mut self) {
        if self.xa_muted || self.muted {
            return;
        }
        let coding = self.sector[19];
        let stereo = coding & 1 != 0;
        let half_rate = coding & 4 != 0;
        let eight_bit = coding & 0x10 != 0;
        let mut left: Vec<i16> = Vec::with_capacity(4032);
        let mut right: Vec<i16> = Vec::with_capacity(2016);
        for g in 0..18 {
            let base = 24 + g * 128;
            let blocks = if eight_bit { 4 } else { 8 };
            for blk in 0..blocks {
                let hdr = self.sector[base + 4 + blk];
                let mut shift = (hdr & 0xF) as i32;
                if shift > 12 {
                    shift = 9;
                }
                let filter = ((hdr >> 4) & 3) as usize;
                let ch = if stereo { blk & 1 } else { 0 };
                for j in 0..28 {
                    let raw = if eight_bit {
                        ((self.sector[base + 16 + j * 4 + blk] as i8 as i32) << 8) as i16 as i32
                    } else {
                        let b = self.sector[base + 16 + j * 4 + blk / 2];
                        let nib = if blk & 1 != 0 { b >> 4 } else { b & 0xF };
                        (((nib as u16) << 12) as i16) as i32
                    };
                    let mut s = raw >> shift;
                    s += (self.xa.old[ch] * XA_POS[filter] + self.xa.older[ch] * XA_NEG[filter] + 32) >> 6;
                    let s = s.clamp(-32768, 32767);
                    self.xa.older[ch] = self.xa.old[ch];
                    self.xa.old[ch] = s;
                    if ch == 0 {
                        left.push(s as i16);
                    } else {
                        right.push(s as i16);
                    }
                }
            }
        }
        let rate = if half_rate { 18900.0 } else { 37800.0 };
        let step = rate / 44100.0;
        let n = left.len();
        // resample with linear interpolation
        let mut pos = self.xa_resample;
        while (pos as usize) < n {
            let i = pos as usize;
            let f = pos - i as f32;
            let li = |v: &Vec<i16>, k: usize, prev: i16| -> f32 { if k < v.len() { v[k] as f32 } else { prev as f32 } };
            let l0 = if i == 0 { self.xa_last.0 as f32 } else { left[i - 1] as f32 };
            let l1 = li(&left, i, self.xa_last.0);
            let (r0, r1) = if stereo {
                (if i == 0 { self.xa_last.1 as f32 } else { right[i - 1] as f32 }, li(&right, i, self.xa_last.1))
            } else {
                (l0, l1)
            };
            let l = l0 + (l1 - l0) * f;
            let r = r0 + (r1 - r0) * f;
            self.push_audio(l as i16, r as i16);
            pos += step;
        }
        self.xa_resample = pos - n as f32;
        self.xa_last = (*left.last().unwrap_or(&0), if stereo { *right.last().unwrap_or(&0) } else { *left.last().unwrap_or(&0) });
    }
}
