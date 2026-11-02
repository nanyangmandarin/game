//! Boot a BIOS (no disc) and report per-frame timing + counters.
//! Usage: cargo run --release --example biosrun -- <bios.bin> [frames] [out.png]
use ps1core::psx::Psx;
use std::time::Instant;

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


fn main() {
    let args: Vec<String> = std::env::args().collect();
    let bios = std::fs::read(&args[1]).expect("bios");
    let frames: usize = args.get(2).map(|s| s.parse().unwrap()).unwrap_or(600);
    let mut psx = Psx::new();
    psx.load_bios(&bios).unwrap();
    let mut worst = (0.0f64, 0usize);
    let t0 = Instant::now();
    let mut chunk = Instant::now();
    for f in 0..frames {
        let t = Instant::now();
        psx.run_frame();
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        if ms > worst.0 { worst = (ms, f); }
        if (f + 1) % 60 == 0 {
            let g = &mut psx.bus.gpu;
            let (w, h) = g.display_size();
            println!(
                "frames {:4}-{:4}: {:6.2} ms/frame idle={:3}% pc={:08X} disp={}x{} px={}k cmds={} exc={}",
                f - 59, f, chunk.elapsed().as_secs_f64() * 1000.0 / 60.0, std::mem::take(&mut psx.idle_cycles) * 100 / (60 * 566_502), psx.cpu.pc, w, h,
                std::mem::take(&mut g.stat_pixels) / 1000, std::mem::take(&mut g.stat_cmds),
                std::mem::take(&mut psx.cpu.stat_exceptions)
            );
            chunk = Instant::now();
        }
    }
    if let Some(out) = args.get(3) {
        let mut frame = Vec::new();
        let (w, h) = psx.bus.gpu.render(&mut frame, false);
        write_png(out, w, h, &frame);
    }
    println!("total {:.1} ms, worst frame {:.2} ms at {}", t0.elapsed().as_secs_f64() * 1000.0, worst.0, worst.1);
}
