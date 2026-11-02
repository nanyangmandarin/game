//! ECM (Error Code Modeler, Neill Corlett) decoder with random access.
//!
//! ECM strips the regenerable parts of CD sectors (sync, address, EDC, ECC). The stream is a
//! sequence of records `[type, count]` followed by their payloads:
//!   type 0: raw bytes, type 1: Mode 1 sector (3-byte address + 2048 data),
//!   type 2: Mode 2 Form 1 (subheader + 2048), type 3: Mode 2 Form 2 (subheader + 2324).
//! The file is scanned once to collect checkpoints, so any byte of the decoded image can be
//! reached by decoding from the nearest checkpoint.

use crate::cdrom::DiscReader;
use std::cell::RefCell;
use std::rc::Rc;

const CHECKPOINT_EVERY: u64 = 64 * 1024;

struct Luts {
    ecc_f: [u8; 256],
    ecc_b: [u8; 256],
    edc: [u32; 256],
}

fn luts() -> &'static Luts {
    use std::sync::OnceLock;
    static L: OnceLock<Luts> = OnceLock::new();
    L.get_or_init(|| {
        let mut l = Luts { ecc_f: [0; 256], ecc_b: [0; 256], edc: [0; 256] };
        for i in 0..256u32 {
            let j = (i << 1) ^ if i & 0x80 != 0 { 0x11D } else { 0 };
            l.ecc_f[i as usize] = j as u8;
            l.ecc_b[(i ^ j) as usize & 0xFF] = i as u8;
            let mut e = i;
            for _ in 0..8 {
                e = (e >> 1) ^ if e & 1 != 0 { 0xD801_8001 } else { 0 };
            }
            l.edc[i as usize] = e;
        }
        l
    })
}

fn edc(data: &[u8]) -> u32 {
    let l = luts();
    data.iter().fold(0u32, |e, &b| (e >> 8) ^ l.edc[((e ^ b as u32) & 0xFF) as usize])
}

fn ecc_block(sector: &mut [u8], major_count: usize, minor_count: usize, major_mult: usize, minor_inc: usize, dest: usize) {
    let l = luts();
    let size = major_count * minor_count;
    for major in 0..major_count {
        let mut index = (major >> 1) * major_mult + (major & 1);
        let (mut a, mut b) = (0u8, 0u8);
        for _ in 0..minor_count {
            let t = sector[0xC + index];
            index += minor_inc;
            if index >= size {
                index -= size;
            }
            a ^= t;
            b ^= t;
            a = l.ecc_f[a as usize];
        }
        a = l.ecc_b[(l.ecc_f[a as usize] ^ b) as usize];
        sector[dest + major] = a;
        sector[dest + major + major_count] = a ^ b;
    }
}

fn ecc_generate(sector: &mut [u8], zero_address: bool) {
    let saved = [sector[12], sector[13], sector[14], sector[15]];
    if zero_address {
        sector[12..16].fill(0);
    }
    ecc_block(sector, 86, 24, 2, 86, 0x81C); // P parity
    ecc_block(sector, 52, 43, 86, 88, 0x8C8); // Q parity
    if zero_address {
        sector[12..16].copy_from_slice(&saved);
    }
}

/// Rebuild a full sector from an ECM item (layout as in unecm). Returns the output slice range.
fn rebuild(typ: u8, payload: &[u8], sector: &mut [u8; 2352]) -> (usize, usize) {
    match typ {
        1 => {
            sector.fill(0);
            sector[1..11].fill(0xFF);
            sector[0xC..0xF].copy_from_slice(&payload[0..3]);
            sector[0xF] = 1;
            sector[0x10..0x810].copy_from_slice(&payload[3..3 + 0x800]);
            let e = edc(&sector[0..0x810]);
            sector[0x810..0x814].copy_from_slice(&e.to_le_bytes());
            ecc_generate(sector, false);
            (0, 2352)
        }
        2 => {
            sector[0x14..0x14 + 0x804].copy_from_slice(&payload[..0x804]);
            sector.copy_within(0x14..0x18, 0x10);
            let e = edc(&sector[0x10..0x10 + 0x808]);
            sector[0x818..0x81C].copy_from_slice(&e.to_le_bytes());
            ecc_generate(sector, true);
            (0x10, 0x10 + 2336)
        }
        _ => {
            sector[0x14..0x14 + 0x918].copy_from_slice(&payload[..0x918]);
            sector.copy_within(0x14..0x18, 0x10);
            let e = edc(&sector[0x10..0x10 + 0x91C]);
            sector[0x92C..0x930].copy_from_slice(&e.to_le_bytes());
            (0x10, 0x10 + 2336)
        }
    }
}

fn item_sizes(typ: u8) -> (u64, u64) {
    match typ {
        1 => (3 + 0x800, 2352),
        2 => (0x804, 2336),
        _ => (0x918, 2336),
    }
}

/// Parser position at an item boundary (or inside a raw run).
#[derive(Clone, Copy, Debug)]
struct State {
    in_pos: u64,
    out_pos: u64,
    typ: u8,
    rem: u64, // items left in the record (bytes left for raw runs); 0 = header next
}

/// Shared handle to the host reader so several decoders can use it.
#[derive(Clone)]
pub struct SharedReader(pub Rc<RefCell<Box<dyn DiscReader>>>);

impl DiscReader for SharedReader {
    fn read(&mut self, file: u32, offset: u64, buf: &mut [u8]) -> bool {
        self.0.borrow_mut().read(file, offset, buf)
    }
}

struct Input {
    r: SharedReader,
    file: u32,
    size: u64,
    buf: Vec<u8>,
    buf_pos: u64,
}

impl Input {
    fn fill(&mut self, pos: u64, want: usize) -> bool {
        let n = (self.size.saturating_sub(pos)).min(want.max(1 << 20) as u64) as usize;
        if n == 0 {
            return false;
        }
        self.buf.resize(n, 0);
        self.buf_pos = pos;
        let ok = self.r.read(self.file, pos, &mut self.buf);
        if !ok {
            self.buf.clear();
        }
        ok
    }

    fn byte(&mut self, pos: u64) -> Option<u8> {
        if pos < self.buf_pos || pos >= self.buf_pos + self.buf.len() as u64 {
            self.fill(pos, 0);
        }
        let o = pos.checked_sub(self.buf_pos)? as usize;
        self.buf.get(o).copied()
    }

    fn read(&mut self, pos: u64, out: &mut [u8]) -> bool {
        let end = pos + out.len() as u64;
        if pos < self.buf_pos || end > self.buf_pos + self.buf.len() as u64 {
            if !self.fill(pos, out.len()) {
                return false;
            }
        }
        let o = (pos - self.buf_pos) as usize;
        if o + out.len() > self.buf.len() {
            return false;
        }
        out.copy_from_slice(&self.buf[o..o + out.len()]);
        true
    }
}

pub struct EcmFile {
    input: Input,
    checkpoints: Vec<State>,
    cur: State,
    pub decoded_size: u64,
    sector: [u8; 2352],
    sector_out: u64, // out_pos of the decoded item in `sector` (u64::MAX = none)
    payload: Vec<u8>,
}

impl EcmFile {
    /// Scan an ECM file and build its checkpoint index.
    pub fn open(r: SharedReader, file: u32, size: u64) -> Result<EcmFile, String> {
        let mut input = Input { r, file, size, buf: Vec::new(), buf_pos: 0 };
        let mut magic = [0u8; 4];
        if !input.read(0, &mut magic) || &magic != b"ECM\0" {
            return Err("Bukan file ECM yang valid".into());
        }
        let mut st = State { in_pos: 4, out_pos: 0, typ: 0, rem: 0 };
        let mut cps = vec![st];
        let mut next_cp = CHECKPOINT_EVERY;
        loop {
            if st.rem == 0 {
                match Self::header(&mut input, &mut st) {
                    Some(true) => {}
                    Some(false) => break, // end marker
                    None => return Err("File ECM terpotong atau rusak".into()),
                }
                if st.in_pos > size {
                    return Err("File ECM terpotong".into());
                }
            }
            if st.typ == 0 {
                // raw run: allow checkpoints inside it
                while st.rem > 0 {
                    let step = if st.out_pos + st.rem > next_cp && next_cp > st.out_pos { next_cp - st.out_pos } else { st.rem };
                    st.in_pos += step;
                    st.out_pos += step;
                    st.rem -= step;
                    if st.out_pos >= next_cp {
                        cps.push(st);
                        next_cp = st.out_pos - st.out_pos % CHECKPOINT_EVERY + CHECKPOINT_EVERY;
                    }
                }
            } else {
                let (isz, osz) = item_sizes(st.typ);
                st.in_pos += isz;
                st.out_pos += osz;
                st.rem -= 1;
                if st.out_pos >= next_cp {
                    cps.push(st);
                    next_cp = st.out_pos - st.out_pos % CHECKPOINT_EVERY + CHECKPOINT_EVERY;
                }
            }
        }
        let cur = cps[0];
        Ok(EcmFile { input, checkpoints: cps, cur, decoded_size: st.out_pos, sector: [0; 2352], sector_out: u64::MAX, payload: Vec::new() })
    }

    /// Read the next record header. Some(false) = end of stream.
    fn header(input: &mut Input, st: &mut State) -> Option<bool> {
        let mut c = input.byte(st.in_pos)?;
        st.in_pos += 1;
        let typ = c & 3;
        let mut num: u64 = ((c >> 2) & 0x1F) as u64;
        let mut bits = 5;
        while c & 0x80 != 0 {
            c = input.byte(st.in_pos)?;
            st.in_pos += 1;
            num |= ((c & 0x7F) as u64) << bits;
            bits += 7;
            if bits > 35 {
                return None;
            }
        }
        if num == 0xFFFF_FFFF {
            return Some(false);
        }
        st.typ = typ;
        st.rem = num + 1;
        Some(true)
    }

    /// Move `cur` so that it covers decoded offset `target`.
    fn seek(&mut self, target: u64) -> bool {
        if target < self.cur.out_pos || target > self.cur.out_pos + 4 * CHECKPOINT_EVERY {
            let i = self.checkpoints.partition_point(|c| c.out_pos <= target).saturating_sub(1);
            self.cur = self.checkpoints[i];
        }
        loop {
            if self.cur.rem == 0 {
                match Self::header(&mut self.input, &mut self.cur) {
                    Some(true) => {}
                    _ => return false,
                }
            }
            let st = &mut self.cur;
            if st.typ == 0 {
                if target < st.out_pos + st.rem {
                    return true;
                }
                st.in_pos += st.rem;
                st.out_pos += st.rem;
                st.rem = 0;
            } else {
                let (isz, osz) = item_sizes(st.typ);
                if target < st.out_pos + osz {
                    return true;
                }
                st.in_pos += isz;
                st.out_pos += osz;
                st.rem -= 1;
            }
        }
    }

    pub fn read_at(&mut self, mut offset: u64, buf: &mut [u8]) -> bool {
        let mut done = 0;
        while done < buf.len() {
            if offset >= self.decoded_size || !self.seek(offset) {
                buf[done..].fill(0);
                return false;
            }
            let st = self.cur;
            let within = offset - st.out_pos;
            if st.typ == 0 {
                let n = ((st.rem - within) as usize).min(buf.len() - done);
                if !self.input.read(st.in_pos + within, &mut buf[done..done + n]) {
                    return false;
                }
                done += n;
                offset += n as u64;
            } else {
                if self.sector_out != st.out_pos {
                    let (isz, _) = item_sizes(st.typ);
                    self.payload.resize(isz as usize, 0);
                    let mut p = std::mem::take(&mut self.payload);
                    let ok = self.input.read(st.in_pos, &mut p);
                    if ok {
                        rebuild(st.typ, &p, &mut self.sector);
                    }
                    self.payload = p;
                    if !ok {
                        return false;
                    }
                    self.sector_out = st.out_pos;
                }
                let (a, b) = match st.typ {
                    1 => (0, 2352),
                    _ => (0x10, 0x10 + 2336),
                };
                let item = &self.sector[a..b];
                let n = (item.len() - within as usize).min(buf.len() - done);
                buf[done..done + n].copy_from_slice(&item[within as usize..within as usize + n]);
                done += n;
                offset += n as u64;
            }
        }
        true
    }
}

/// Serves raw files from the host reader, except files that are ECM-decoded.
pub struct MuxReader {
    pub raw: SharedReader,
    pub ecm: Vec<(u32, EcmFile)>,
}

impl DiscReader for MuxReader {
    fn read(&mut self, file: u32, offset: u64, buf: &mut [u8]) -> bool {
        if let Some((_, e)) = self.ecm.iter_mut().find(|(f, _)| *f == file) {
            return e.read_at(offset, buf);
        }
        self.raw.read(file, offset, buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edc_matches_crc32_cdrom_edc_check_value() {
        // CRC-32/CD-ROM-EDC catalogue check value
        assert_eq!(edc(b"123456789"), 0x6EC2_EDC4);
    }

    /// Write sectors rebuilt from ECM items (types 1-3) to `PS1_ECM_OUT` for external checks.
    #[test]
    fn dump_rebuilt_sectors() {
        let Ok(out) = std::env::var("PS1_ECM_OUT") else { return };
        let mut bin = Vec::new();
        let mut s = [0u8; 2352];
        for i in 0..200u32 {
            let mut p = vec![0u8; 0x918];
            for (k, b) in p.iter_mut().enumerate() {
                *b = if k % 97 == 0 { (i as usize * 7 + k) as u8 } else { 0 };
            }
            if i % 2 == 0 {
                // Mode 1 with MSF address
                let f = 150 + i;
                p[0] = ((f / 4500 / 10) << 4 | (f / 4500 % 10)) as u8;
                p[1] = (((f / 75 % 60) / 10) << 4 | (f / 75 % 60 % 10)) as u8;
                p[2] = (((f % 75) / 10) << 4 | (f % 75 % 10)) as u8;
                let (a, b) = rebuild(1, &p, &mut s);
                bin.extend_from_slice(&s[a..b]);
            } else {
                // Mode 2 Form 1: sync + header (raw part) + rebuilt 2336 bytes
                p[0..4].copy_from_slice(&[0, 0, 8, 0]);
                let (a, b) = rebuild(2, &p, &mut s);
                let mut full = [0u8; 2352];
                full[1..11].fill(0xFF);
                let f = 150 + i;
                full[12] = ((f / 4500 / 10) << 4 | (f / 4500 % 10)) as u8;
                full[13] = (((f / 75 % 60) / 10) << 4 | (f / 75 % 60 % 10)) as u8;
                full[14] = (((f % 75) / 10) << 4 | (f % 75 % 10)) as u8;
                full[15] = 2;
                full[16..].copy_from_slice(&s[a..b]);
                bin.extend_from_slice(&full);
            }
        }
        std::fs::write(&out, &bin).unwrap();
    }

    #[test]
    fn edc_ecc_known_values() {
        // EDC of an empty Mode 1 data area is a known constant for all-zero input
        assert_eq!(edc(&[0u8; 16]), 0);
        let mut s = [0u8; 2352];
        let payload = vec![0u8; 3 + 0x800];
        rebuild(1, &payload, &mut s);
        assert_eq!(&s[0..12], &[0, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0]);
        assert_eq!(s[0xF], 1);
    }
}

#[cfg(test)]
mod stream_tests {
    use super::*;

    struct Mem(Vec<u8>);
    impl DiscReader for Mem {
        fn read(&mut self, _f: u32, off: u64, buf: &mut [u8]) -> bool {
            let o = off as usize;
            if o + buf.len() > self.0.len() {
                return false;
            }
            buf.copy_from_slice(&self.0[o..o + buf.len()]);
            true
        }
    }

    fn header(out: &mut Vec<u8>, typ: u8, count: u64) {
        let mut n = count - 1;
        let mut c = (typ as u64) | ((n & 0x1F) << 2);
        n >>= 5;
        if n > 0 {
            c |= 0x80;
        }
        out.push(c as u8);
        while n > 0 {
            let mut b = n & 0x7F;
            n >>= 7;
            if n > 0 {
                b |= 0x80;
            }
            out.push(b as u8);
        }
    }

    #[test]
    fn random_access_matches_expected_image() {
        let mut ecm = b"ECM\0".to_vec();
        let mut expected = Vec::new();
        let mut seed = 12345u32;
        let mut rnd = || {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
            (seed >> 16) as u8
        };
        let mut s = [0u8; 2352];
        for round in 0..40 {
            // raw run of varying length
            let n = 16 + round * 37;
            header(&mut ecm, 0, n as u64);
            for _ in 0..n {
                let b = rnd();
                ecm.push(b);
                expected.push(b);
            }
            // runs of sectors of each type; counts large enough to need multi-byte headers
            for typ in 1u8..=3 {
                let count = 1 + (round * 7 + typ as usize * 13) % 45;
                header(&mut ecm, typ, count as u64);
                for _ in 0..count {
                    let (isz, _) = item_sizes(typ);
                    let p: Vec<u8> = (0..isz).map(|_| rnd()).collect();
                    ecm.extend_from_slice(&p);
                    let (a, b) = rebuild(typ, &p, &mut s);
                    expected.extend_from_slice(&s[a..b]);
                }
            }
        }
        // end marker + (ignored) checksum
        header(&mut ecm, 0, 0x1_0000_0000);
        ecm.extend_from_slice(&[0, 0, 0, 0]);

        let size = ecm.len() as u64;
        let r = SharedReader(Rc::new(RefCell::new(Box::new(Mem(ecm)) as Box<dyn DiscReader>)));
        let mut f = EcmFile::open(r, 0, size).unwrap();
        assert_eq!(f.decoded_size, expected.len() as u64);
        // sequential full read
        let mut all = vec![0u8; expected.len()];
        assert!(f.read_at(0, &mut all));
        assert!(all == expected, "sequential decode differs");
        // random reads, including backwards seeks and reads spanning items
        let mut x = 7u64;
        for _ in 0..500 {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let off = (x >> 20) % expected.len() as u64;
            let len = (((x >> 8) % 5000) as usize).min(expected.len() - off as usize);
            let mut buf = vec![0u8; len];
            assert!(f.read_at(off, &mut buf));
            assert!(buf[..] == expected[off as usize..off as usize + len], "random read at {off}+{len}");
        }
    }
}
