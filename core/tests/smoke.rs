//! End-to-end smoke test without a BIOS: a hand-assembled PS-X EXE exercises
//! CPU arithmetic and draws primitives through the GPU.

use ps1core::psx::Psx;

fn lui(rt: u32, imm: u32) -> u32 { (0x0F << 26) | (rt << 16) | (imm & 0xFFFF) }
fn ori(rt: u32, rs: u32, imm: u32) -> u32 { (0x0D << 26) | (rs << 21) | (rt << 16) | (imm & 0xFFFF) }
fn sw(rt: u32, off: u32, base: u32) -> u32 { (0x2B << 26) | (base << 21) | (rt << 16) | (off & 0xFFFF) }
fn special(rs: u32, rt: u32, rd: u32, f: u32) -> u32 { (rs << 21) | (rt << 16) | (rd << 11) | f }
fn j(addr: u32) -> u32 { (2 << 26) | ((addr >> 2) & 0x03FF_FFFF) }

const T0: u32 = 8;
const T1: u32 = 9;
const T2: u32 = 10;
const T3: u32 = 11;

struct Asm(Vec<u32>);
impl Asm {
    fn li(&mut self, r: u32, v: u32) { self.0.push(lui(r, v >> 16)); self.0.push(ori(r, r, v)); }
    fn gp0(&mut self, v: u32) { self.li(T1, v); self.0.push(sw(T1, 0x1810, T0)); }
    fn gp1(&mut self, v: u32) { self.li(T1, v); self.0.push(sw(T1, 0x1814, T0)); }
}

fn build_exe(code: &[u32], load: u32) -> Vec<u8> {
    let mut d = vec![0u8; 0x800];
    d[0..8].copy_from_slice(b"PS-X EXE");
    d[0x10..0x14].copy_from_slice(&load.to_le_bytes());
    d[0x18..0x1C].copy_from_slice(&load.to_le_bytes());
    d[0x1C..0x20].copy_from_slice(&((code.len() * 4) as u32).to_le_bytes());
    for w in code { d.extend_from_slice(&w.to_le_bytes()); }
    d
}

fn write_png(path: &str, w: u32, h: u32, rgba: &[u8]) {
    fn crc(data: &[u8]) -> u32 {
        let mut c = 0xFFFF_FFFFu32;
        for &b in data { c ^= b as u32; for _ in 0..8 { c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 }; } }
        !c
    }
    fn chunk(out: &mut Vec<u8>, ty: &[u8], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut c = ty.to_vec(); c.extend_from_slice(data);
        out.extend_from_slice(&c); out.extend_from_slice(&crc(&c).to_be_bytes());
    }
    let mut raw = Vec::new();
    for y in 0..h as usize { raw.push(0); raw.extend_from_slice(&rgba[y * w as usize * 4..(y + 1) * w as usize * 4]); }
    let mut z = vec![0x78, 0x01];
    for (i, blk) in raw.chunks(65535).enumerate() {
        let last = (i + 1) * 65535 >= raw.len();
        z.push(last as u8);
        z.extend_from_slice(&(blk.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(blk.len() as u16)).to_le_bytes());
        z.extend_from_slice(blk);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &x in &raw { a = (a + x as u32) % 65521; b = (b + a) % 65521; }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes()); ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr); chunk(&mut out, b"IDAT", &z); chunk(&mut out, b"IEND", &[]);
    std::fs::write(path, out).unwrap();
}

#[test]
fn cpu_and_gpu_smoke() {
    let base = 0x8001_0000u32;
    let mut a = Asm(Vec::new());
    a.0.push(lui(T0, 0x1F80));
    // CPU: 12345 * 678 and 1000000 / 7, stored to 0x80000100.. (RAM addr 0x100)
    a.li(T2, 12345); a.li(T3, 678);
    a.0.push(special(T2, T3, 0, 0x18)); a.0.push(special(0, 0, T1, 0x12));
    a.li(12, 0x8000_0000); a.0.push(sw(T1, 0x100, 12));
    a.li(T2, 1_000_000); a.li(T3, 7);
    a.0.push(special(T2, T3, 0, 0x1A)); a.0.push(special(0, 0, T1, 0x12)); a.0.push(sw(T1, 0x104, 12));
    a.0.push(special(0, 0, T1, 0x10)); a.0.push(sw(T1, 0x108, 12));
    // GPU setup
    a.gp1(0x0000_0000); a.gp1(0x0300_0000); a.gp1(0x0800_0001); a.gp1(0x0500_0000);
    a.gp1(0x0700_0000 | 0x10 | (0x100 << 10));
    a.gp0(0xE100_0200); a.gp0(0xE300_0000); a.gp0(0xE403_BD3F); a.gp0(0xE500_0000);
    a.gp0(0x0220_2020); a.gp0(0x0000_0000); a.gp0(0x00F0_0140);
    // gouraud triangle
    a.gp0(0x3000_00FF); a.gp0(0x00C8_0028); a.gp0(0x0000_FF00); a.gp0(0x0014_00A0); a.gp0(0x00FF_0000); a.gp0(0x00C8_0118);
    // semi-transparent white rectangle
    a.gp0(0x62FF_FFFF); a.gp0(0x0014_000A); a.gp0(0x0028_003C);
    // yellow line
    a.gp0(0x4000_FFFF); a.gp0(0x000A_000A); a.gp0(0x00E6_0136);
    let here = base + (a.0.len() as u32) * 4;
    a.0.push(j(here)); a.0.push(0);

    let exe = build_exe(&a.0, base);
    std::fs::create_dir_all("../target").ok();
    std::fs::write("../target/smoke.exe", &exe).unwrap();
    let mut psx = Psx::new();
    psx.load_exe(&exe).unwrap();
    for _ in 0..3 { psx.run_frame(); }

    let rd = |o: usize| u32::from_le_bytes(psx.bus.ram[o..o + 4].try_into().unwrap());
    assert_eq!(rd(0x100), 12345 * 678);
    assert_eq!(rd(0x104), 1_000_000 / 7);
    assert_eq!(rd(0x108), 1_000_000 % 7);

    let mut frame = Vec::new();
    let (w, h) = psx.bus.gpu.render(&mut frame, false);
    assert_eq!((w, h), (320, 240));
    let px = |x: u32, y: u32| { let o = ((y * w + x) * 4) as usize; [frame[o], frame[o + 1], frame[o + 2]] };
    // background
    assert_eq!(px(5, 120), [0x21, 0x21, 0x21]);
    // inside the triangle near the red vertex
    let p = px(60, 190); assert!(p[0] > 150 && p[2] < 80, "triangle {:?}", p);
    // semi-transparent rect over background: (bg + white)/2
    let p = px(15, 25); assert!(p[0] > 0x80 && p[0] < 0xA0, "rect {:?}", p);
    std::fs::create_dir_all("../target").ok();
    write_png("../target/smoke.png", w, h, &frame);
}

#[test]
fn load_delay_slot_semantics() {
    let base = 0x8001_0000u32;
    let lw = |rt: u32, off: u32, b: u32| (0x23 << 26) | (b << 21) | (rt << 16) | (off & 0xFFFF);
    let addu = |rd: u32, rs: u32, rt: u32| special(rs, rt, rd, 0x21);
    let mut c = vec![
        lui(T0, 0x8000),
        ori(T1, 0, 5), sw(T1, 0x200, T0),
        ori(T2, 0, 1),
        lw(T2, 0x200, T0),
        addu(T3, T2, 0),          // delay slot: must still see old value (1)
        addu(12, T2, 0),          // now sees loaded value (5)
        sw(T3, 0x204, T0), sw(12, 0x208, T0),
        // delay-slot instruction overwriting the load target wins
        lw(13, 0x200, T0),
        ori(13, 0, 7),
        0,
        sw(13, 0x20C, T0),
    ];
    let here = base + (c.len() as u32) * 4;
    c.push(j(here));
    c.push(0);
    let mut psx = Psx::new();
    psx.load_exe(&build_exe(&c, base)).unwrap();
    psx.run_frame();
    let rd = |o: usize| u32::from_le_bytes(psx.bus.ram[o..o + 4].try_into().unwrap());
    assert_eq!(rd(0x204), 1, "delay slot read old value");
    assert_eq!(rd(0x208), 5, "value visible after delay slot");
    assert_eq!(rd(0x20C), 7, "write in delay slot overrides load");
}
