//! GameShark / Action Replay (PS1) cheat codes, applied to RAM once per frame.
//!
//! Supported code types (`TTAAAAAA VVVV`):
//! 80 write16, 30 write8, 10/11 inc/dec16, 20/21 inc/dec8,
//! D0-D3 if16 (==, !=, <, >) next code, E0-E3 if8 next code,
//! C0 if16 == then enable the rest of the cheat, 50 serial repeater, C2 memory copy.

#[derive(Clone, Copy, Debug, PartialEq)]
enum Op {
    W16(u32, u16),
    W8(u32, u8),
    Inc16(u32, u16),
    Dec16(u32, u16),
    Inc8(u32, u8),
    Dec8(u32, u8),
    If16(u32, u16, u8), // cmp: 0 ==, 1 !=, 2 <, 3 >
    If8(u32, u8, u8),
    EnableIf16(u32, u16),
    Repeat { count: u32, astep: u32, vstep: u16 },
    Copy { src: u32, len: u32 },
}

#[derive(Default)]
pub struct Cheats {
    groups: Vec<Vec<(Op, Option<u32>)>>, // op + optional destination (for Copy: next line's address)
}

const RAM_MASK: u32 = 0x1F_FFFF;

/// Parse one cheat (several lines). Returns the ops or a human-readable error.
fn parse_cheat(text: &str) -> Result<Vec<(Op, Option<u32>)>, String> {
    let mut out: Vec<(Op, Option<u32>)> = Vec::new();
    let mut pending_copy = false;
    for (ln, raw) in text.lines().enumerate() {
        let line: String = raw.chars().filter(|c| !c.is_whitespace() && *c != '-' && *c != ':').collect();
        if line.is_empty() {
            continue;
        }
        if line.len() != 12 || !line.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(format!("Baris {}: \"{}\" bukan kode (format: XXXXXXXX YYYY)", ln + 1, raw.trim()));
        }
        let a = u32::from_str_radix(&line[0..8], 16).unwrap();
        let v = u16::from_str_radix(&line[8..12], 16).unwrap();
        let t = a >> 24;
        let addr = a & 0x00FF_FFFF;
        if pending_copy {
            // destination line of a C2 copy (80ZZZZZZ 0000)
            if let Some(last) = out.last_mut() {
                last.1 = Some(addr & RAM_MASK);
            }
            pending_copy = false;
            continue;
        }
        let op = match t {
            0x80 => Op::W16(addr, v),
            0x30 => Op::W8(addr, v as u8),
            0x10 => Op::Inc16(addr, v),
            0x11 => Op::Dec16(addr, v),
            0x20 => Op::Inc8(addr, v as u8),
            0x21 => Op::Dec8(addr, v as u8),
            0xD0..=0xD3 => Op::If16(addr, v, (t - 0xD0) as u8),
            0xE0..=0xE3 => Op::If8(addr, v as u8, (t - 0xE0) as u8),
            0xC0 => Op::EnableIf16(addr, v),
            0x50 => Op::Repeat { count: (addr >> 8) & 0xFF, astep: addr & 0xFF, vstep: v },
            0xC2 => {
                pending_copy = true;
                Op::Copy { src: addr, len: v as u32 }
            }
            0x1F => {
                return Err(format!("Baris {}: kode tipe 1F (scratchpad) belum didukung", ln + 1));
            }
            _ => return Err(format!("Baris {}: tipe kode {:02X} belum didukung", ln + 1, t)),
        };
        out.push((op, None));
    }
    if out.is_empty() {
        return Err("Kode kosong".into());
    }
    Ok(out)
}

/// Validate a cheat for the UI: empty string when OK.
pub fn validate(text: &str) -> String {
    match parse_cheat(text) {
        Ok(_) => String::new(),
        Err(e) => e,
    }
}

fn r16(ram: &[u8], a: u32) -> u16 {
    let o = (a & RAM_MASK & !1) as usize;
    u16::from_le_bytes([ram[o], ram[o + 1]])
}
fn w16(ram: &mut [u8], a: u32, v: u16) {
    let o = (a & RAM_MASK & !1) as usize;
    ram[o..o + 2].copy_from_slice(&v.to_le_bytes());
}
fn r8(ram: &[u8], a: u32) -> u8 {
    ram[(a & RAM_MASK) as usize]
}
fn w8(ram: &mut [u8], a: u32, v: u8) {
    ram[(a & RAM_MASK) as usize] = v;
}

impl Cheats {
    /// Replace the active cheats. Cheats are separated by a line containing only `---`.
    /// Invalid cheats are skipped; their errors are returned (one per line).
    pub fn set(&mut self, text: &str) -> String {
        self.groups.clear();
        let mut errors = Vec::new();
        for (i, chunk) in text.split("\n---\n").enumerate() {
            if chunk.trim().is_empty() {
                continue;
            }
            match parse_cheat(chunk) {
                Ok(ops) => self.groups.push(ops),
                Err(e) => errors.push(format!("Cheat {}: {}", i + 1, e)),
            }
        }
        errors.join("\n")
    }

    pub fn is_empty(&self) -> bool {
        self.groups.is_empty()
    }

    pub fn apply(&self, ram: &mut [u8]) {
        for g in &self.groups {
            let mut i = 0;
            let mut skip_next = false;
            while i < g.len() {
                let (op, dst) = g[i];
                i += 1;
                if skip_next {
                    skip_next = false;
                    // a skipped repeater also skips the code it repeats
                    if matches!(op, Op::Repeat { .. }) {
                        i += 1;
                    }
                    continue;
                }
                match op {
                    Op::W16(a, v) => w16(ram, a, v),
                    Op::W8(a, v) => w8(ram, a, v),
                    Op::Inc16(a, v) => w16(ram, a, r16(ram, a).wrapping_add(v)),
                    Op::Dec16(a, v) => w16(ram, a, r16(ram, a).wrapping_sub(v)),
                    Op::Inc8(a, v) => w8(ram, a, r8(ram, a).wrapping_add(v)),
                    Op::Dec8(a, v) => w8(ram, a, r8(ram, a).wrapping_sub(v)),
                    Op::If16(a, v, c) => {
                        let x = r16(ram, a);
                        skip_next = !cmp(x as u32, v as u32, c);
                    }
                    Op::If8(a, v, c) => {
                        let x = r8(ram, a);
                        skip_next = !cmp(x as u32, v as u32, c);
                    }
                    Op::EnableIf16(a, v) => {
                        if r16(ram, a) != v {
                            break; // rest of this cheat disabled
                        }
                    }
                    Op::Repeat { count, astep, vstep } => {
                        if i < g.len() {
                            let (next, _) = g[i];
                            i += 1;
                            for k in 0..count {
                                match next {
                                    Op::W16(a, v) => w16(ram, a + k * astep, v.wrapping_add(vstep.wrapping_mul(k as u16))),
                                    Op::W8(a, v) => w8(ram, a + k * astep, v.wrapping_add((vstep as u8).wrapping_mul(k as u8))),
                                    _ => {}
                                }
                            }
                        }
                    }
                    Op::Copy { src, len } => {
                        if let Some(d) = dst {
                            for k in 0..len {
                                let b = r8(ram, src + k);
                                w8(ram, d + k, b);
                            }
                        }
                    }
                }
            }
        }
    }
}

fn cmp(x: u32, v: u32, c: u8) -> bool {
    match c {
        0 => x == v,
        1 => x != v,
        2 => x < v,
        _ => x > v,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_apply() {
        let mut ram = vec![0u8; 2 * 1024 * 1024];
        let mut c = Cheats::default();
        let err = c.set(
            "800A0000 03E7\n300A0010 0063\n---\nD00A0020 0001\n800A0030 1234\n---\n50000502 0001\n800A0100 0010\n---\nC20A0000 0004\n800A0200 0000",
        );
        assert_eq!(err, "");
        c.apply(&mut ram);
        assert_eq!(r16(&ram, 0x0A0000), 999);
        assert_eq!(r8(&ram, 0x0A0010), 0x63);
        assert_eq!(r16(&ram, 0x0A0030), 0, "condition false: write skipped");
        for k in 0..5 {
            assert_eq!(r16(&ram, 0x0A0100 + k * 2), 0x10 + k as u16, "repeater {k}");
        }
        assert_eq!(&ram[0x0A0200..0x0A0204], &ram[0x0A0000..0x0A0004], "copy");
        w16(&mut ram, 0x0A0020, 1);
        c.apply(&mut ram);
        assert_eq!(r16(&ram, 0x0A0030), 0x1234, "condition true: write applied");
        assert!(validate("8009C6E4 FFFF").is_empty());
        assert!(!validate("hello").is_empty());
        assert!(!validate("FF000000 0000").is_empty());
    }
}
