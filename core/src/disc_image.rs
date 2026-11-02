//! Compressed disc image containers: PBP (PSP "EBOOT" PS1 format) and CHD (MAME).
//!
//! Both are exposed to the CD-ROM controller as a *virtual raw image*: file 0 holds
//! consecutive 2352-byte frames, decompressed on demand with a small block cache.

use crate::cdrom::{DiscReader, Track, SECTOR_RAW};
use std::io::{Read, Seek, SeekFrom};

fn bcd2bin(v: u8) -> u32 {
    ((v >> 4) as u32) * 10 + (v & 0xF) as u32
}

fn rd32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

/// Byte-addressed reader over one file of the host (JS) reader.
pub struct FileAdapter {
    inner: Box<dyn DiscReader>,
    file: u32,
    pos: u64,
    size: u64,
}

impl FileAdapter {
    pub fn new(inner: Box<dyn DiscReader>, file: u32, size: u64) -> Self {
        FileAdapter { inner, file, pos: 0, size }
    }

    fn read_at(&mut self, off: u64, buf: &mut [u8]) -> bool {
        if off + buf.len() as u64 > self.size {
            return false;
        }
        self.inner.read(self.file, off, buf)
    }
}

impl Read for FileAdapter {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = (self.size.saturating_sub(self.pos)).min(buf.len() as u64) as usize;
        if n == 0 {
            return Ok(0);
        }
        if !self.inner.read(self.file, self.pos, &mut buf[..n]) {
            return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "read failed"));
        }
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for FileAdapter {
    fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
        let p = match to {
            SeekFrom::Start(p) => p as i64,
            SeekFrom::End(d) => self.size as i64 + d,
            SeekFrom::Current(d) => self.pos as i64 + d,
        };
        if p < 0 {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "negative seek"));
        }
        self.pos = p as u64;
        Ok(self.pos)
    }
}

/// Track list + virtual raw reader produced by a container parser.
pub struct OpenedImage {
    pub tracks: Vec<Track>,
    pub reader: Box<dyn DiscReader>,
    pub disc_count: u32,
}

/// Serve byte reads of the virtual raw image from whole-frame reads.
fn read_virtual(buf: &mut [u8], offset: u64, mut frame_fn: impl FnMut(u64, &mut [u8; SECTOR_RAW]) -> bool) -> bool {
    let mut tmp = [0u8; SECTOR_RAW];
    let mut done = 0usize;
    while done < buf.len() {
        let abs = offset + done as u64;
        let frame = abs / SECTOR_RAW as u64;
        let within = (abs % SECTOR_RAW as u64) as usize;
        if !frame_fn(frame, &mut tmp) {
            buf[done..].fill(0);
            return false;
        }
        let n = (SECTOR_RAW - within).min(buf.len() - done);
        buf[done..done + n].copy_from_slice(&tmp[within..within + n]);
        done += n;
    }
    true
}

// ======================================================================
// PBP
// ======================================================================

const PBP_BLOCK_FRAMES: u64 = 16;
const PBP_BLOCK_BYTES: usize = PBP_BLOCK_FRAMES as usize * SECTOR_RAW;

struct PbpReader {
    file: FileAdapter,
    /// Absolute file offsets of each compressed block; one extra entry marks the end.
    blocks: Vec<u64>,
    cache_idx: u64,
    cache: Vec<u8>,
    raw: Vec<u8>,
}

impl PbpReader {
    fn frame(&mut self, frame: u64, out: &mut [u8; SECTOR_RAW]) -> bool {
        let b = frame / PBP_BLOCK_FRAMES;
        if b + 1 >= self.blocks.len() as u64 {
            out.fill(0);
            return false;
        }
        if self.cache_idx != b {
            let start = self.blocks[b as usize];
            let size = (self.blocks[b as usize + 1].saturating_sub(start) as usize).min(PBP_BLOCK_BYTES * 2);
            self.raw.resize(size, 0);
            let mut raw = std::mem::take(&mut self.raw);
            let ok = self.file.read_at(start, &mut raw);
            if !ok {
                self.raw = raw;
                return false;
            }
            self.cache.clear();
            if size == PBP_BLOCK_BYTES {
                self.cache.extend_from_slice(&raw);
            } else {
                match miniz_oxide::inflate::decompress_to_vec(&raw) {
                    Ok(v) => self.cache = v,
                    Err(_) => {
                        self.raw = raw;
                        return false;
                    }
                }
            }
            self.raw = raw;
            self.cache.resize(PBP_BLOCK_BYTES, 0);
            self.cache_idx = b;
        }
        let o = (frame % PBP_BLOCK_FRAMES) as usize * SECTOR_RAW;
        out.copy_from_slice(&self.cache[o..o + SECTOR_RAW]);
        true
    }
}

impl DiscReader for PbpReader {
    fn read(&mut self, _file: u32, offset: u64, buf: &mut [u8]) -> bool {
        read_virtual(buf, offset, |f, out| self.frame(f, out))
    }
}

/// Parse a PBP and select disc `disc` (0-based) of a multi-disc image.
pub fn open_pbp(inner: Box<dyn DiscReader>, size: u64, disc: u32) -> Result<OpenedImage, String> {
    let mut f = FileAdapter::new(inner, 0, size);
    let mut hdr = [0u8; 0x28];
    if !f.read_at(0, &mut hdr) || &hdr[0..4] != b"\0PBP" {
        return Err("Bukan file PBP yang valid".into());
    }
    let psar = rd32(&hdr, 0x24) as u64;
    let mut sig = [0u8; 16];
    if !f.read_at(psar, &mut sig) {
        return Err("PBP rusak (PSAR)".into());
    }
    let (iso_off, disc_count) = if &sig[0..10] == b"PSTITLEIMG" {
        let mut tab = [0u8; 32];
        if !f.read_at(psar + 0x200, &mut tab) {
            return Err("PBP rusak (tabel disc)".into());
        }
        let offs: Vec<u32> = (0..8).map(|i| rd32(&tab, i * 4)).take_while(|&o| o != 0).collect();
        if offs.is_empty() {
            return Err("PBP tidak berisi disc".into());
        }
        let d = (disc as usize).min(offs.len() - 1);
        (psar + offs[d] as u64, offs.len() as u32)
    } else if &sig[0..10] == b"PSISOIMG00" {
        (psar, 1)
    } else {
        return Err("PBP ini bukan game PS1 (tidak ada PSISOIMG)".into());
    };
    let mut s2 = [0u8; 10];
    if !f.read_at(iso_off, &mut s2) || &s2 != b"PSISOIMG00" {
        return Err("PBP rusak (PSISOIMG)".into());
    }

    // TOC: 10-byte entries at +0x800: [type, pad, point, idx0 m s f, pad, idx1 m s f]
    let mut toc = [0u8; 10 * 104];
    if !f.read_at(iso_off + 0x800, &mut toc) {
        return Err("PBP rusak (TOC)".into());
    }
    let ent = |i: usize| &toc[i * 10..i * 10 + 10];
    let msf = |e: &[u8]| (bcd2bin(e[7]) * 60 + bcd2bin(e[8])) * 75 + bcd2bin(e[9]);
    let num_tracks = bcd2bin(ent(1)[7]).clamp(1, 99) as usize;
    let lead_out = msf(ent(2));
    let mut tracks = Vec::new();
    for i in 1..=num_tracks {
        let e = ent(2 + i);
        let start = msf(e);
        tracks.push(Track {
            number: i as u8,
            file: 0,
            start,
            length: 0,
            sector_size: SECTOR_RAW as u32,
            audio: e[0] & 0x40 == 0,
            // the image is linear: image frame = absolute frame - 150
            file_offset: (start.saturating_sub(150) as u64) * SECTOR_RAW as u64,
        });
    }
    for i in 0..tracks.len() {
        let end = if i + 1 < tracks.len() { tracks[i + 1].start } else { lead_out };
        tracks[i].length = end.saturating_sub(tracks[i].start);
    }

    // Block index at +0x4000: 32-byte entries {offset, size, ...}; data at +0x100000.
    let mut idx = vec![0u8; 0x100000 - 0x4000];
    if !f.read_at(iso_off + 0x4000, &mut idx) {
        return Err("PBP rusak (indeks blok)".into());
    }
    let base = iso_off + 0x100000;
    let mut blocks = Vec::new();
    let mut last = 0u64;
    for e in idx.chunks(32) {
        let off = rd32(e, 0) as u64;
        let sz = rd32(e, 4) & 0xFFFF;
        if sz == 0 {
            break;
        }
        blocks.push(base + off);
        last = base + off + sz as u64;
    }
    if blocks.is_empty() {
        return Err("PBP tidak berisi data".into());
    }
    blocks.push(last);

    let reader = PbpReader { file: f, blocks, cache_idx: u64::MAX, cache: Vec::new(), raw: Vec::new() };
    Ok(OpenedImage { tracks, reader: Box::new(reader), disc_count })
}

// ======================================================================
// CHD
// ======================================================================

const CHD_FRAME_BYTES: usize = 2448; // 2352 data + 96 subcode

struct ChdReader {
    chd: chd::Chd<FileAdapter>,
    frames_per_hunk: u64,
    hunk_buf: Vec<u8>,
    cmp_buf: Vec<u8>,
    cache_hunk: u64,
    /// Virtual frame ranges holding CD audio (stored big-endian in CHD).
    audio: Vec<(u64, u64)>,
}

impl ChdReader {
    fn frame(&mut self, frame: u64, out: &mut [u8; SECTOR_RAW]) -> bool {
        let h = frame / self.frames_per_hunk;
        if h != self.cache_hunk {
            let ok = match self.chd.hunk(h as u32) {
                Ok(mut hunk) => hunk.read_hunk_in(&mut self.cmp_buf, &mut self.hunk_buf).is_ok(),
                Err(_) => false,
            };
            if !ok {
                self.cache_hunk = u64::MAX;
                out.fill(0);
                return false;
            }
            self.cache_hunk = h;
        }
        let o = (frame % self.frames_per_hunk) as usize * CHD_FRAME_BYTES;
        out.copy_from_slice(&self.hunk_buf[o..o + SECTOR_RAW]);
        if self.audio.iter().any(|&(a, b)| frame >= a && frame < b) {
            for p in out.chunks_exact_mut(2) {
                p.swap(0, 1);
            }
        }
        true
    }
}

impl DiscReader for ChdReader {
    fn read(&mut self, _file: u32, offset: u64, buf: &mut [u8]) -> bool {
        read_virtual(buf, offset, |f, out| self.frame(f, out))
    }
}

fn meta_field<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    text.split_whitespace().find_map(|kv| kv.strip_prefix(key).and_then(|v| v.strip_prefix(':')))
}

pub fn open_chd(inner: Box<dyn DiscReader>, size: u64) -> Result<OpenedImage, String> {
    let f = FileAdapter::new(inner, 0, size);
    let mut chd = chd::Chd::open(f, None).map_err(|e| match e {
        chd::Error::RequiresParent => "CHD ini butuh file induk (parent) dan tidak didukung".to_string(),
        e => format!("CHD tidak bisa dibuka: {e:?}"),
    })?;
    let hunk_bytes = chd.header().hunk_size() as usize;
    let unit = chd.header().unit_bytes() as usize;
    if unit != CHD_FRAME_BYTES || hunk_bytes % CHD_FRAME_BYTES != 0 {
        return Err("CHD ini bukan image CD (mungkin hard disk / DVD)".into());
    }
    let metas: Vec<chd::metadata::Metadata> = chd
        .metadata_refs()
        .try_into()
        .map_err(|e| format!("Metadata CHD rusak: {e:?}"))?;
    let cht2 = u32::from_be_bytes(*b"CHT2");
    let chtr = u32::from_be_bytes(*b"CHTR");
    let mut entries: Vec<String> = metas
        .iter()
        .filter(|m| m.metatag == cht2 || m.metatag == chtr)
        .map(|m| String::from_utf8_lossy(&m.value).trim_end_matches('\0').to_string())
        .collect();
    if entries.is_empty() {
        return Err("CHD tidak berisi daftar track CD".into());
    }
    entries.sort_by_key(|t| meta_field(t, "TRACK").and_then(|v| v.parse::<u32>().ok()).unwrap_or(0));

    let mut tracks = Vec::new();
    let mut audio = Vec::new();
    let mut chd_frame: u64 = 0; // position in the CHD frame stream
    let mut lba: u32 = 0; // disc LBA (absolute frame - 150)
    for (i, t) in entries.iter().enumerate() {
        let num = meta_field(t, "TRACK").and_then(|v| v.parse().ok()).unwrap_or(i as u32 + 1);
        let ty = meta_field(t, "TYPE").unwrap_or("MODE2_RAW");
        let frames: u32 = meta_field(t, "FRAMES").and_then(|v| v.parse().ok()).unwrap_or(0);
        let pregap: u32 = meta_field(t, "PREGAP").and_then(|v| v.parse().ok()).unwrap_or(0);
        let pg_in_file = meta_field(t, "PGTYPE").map(|v| v.starts_with('V')).unwrap_or(false);
        let is_audio = ty == "AUDIO";
        if !is_audio && !ty.ends_with("_RAW") {
            return Err(format!("CHD dengan track {ty} belum didukung (butuh MODE1_RAW/MODE2_RAW/AUDIO)"));
        }
        let (data_start, body) = if pg_in_file { (chd_frame + pregap as u64, frames - pregap.min(frames)) } else { (chd_frame, frames) };
        let gap = if i == 0 { 0 } else { pregap };
        let start = 150 + lba + gap;
        tracks.push(Track {
            number: num as u8,
            file: 0,
            start,
            length: body,
            sector_size: SECTOR_RAW as u32,
            audio: is_audio,
            file_offset: data_start * SECTOR_RAW as u64,
        });
        if is_audio {
            audio.push((chd_frame, chd_frame + frames as u64));
        }
        lba += gap + body;
        chd_frame += frames as u64;
        chd_frame = (chd_frame + 3) & !3; // tracks are padded to a multiple of 4 frames
    }

    let reader = ChdReader {
        hunk_buf: chd.get_hunksized_buffer(),
        chd,
        frames_per_hunk: (hunk_bytes / CHD_FRAME_BYTES) as u64,
        cmp_buf: Vec::new(),
        cache_hunk: u64::MAX,
        audio,
    };
    Ok(OpenedImage { tracks, reader: Box::new(reader), disc_count: 1 })
}
