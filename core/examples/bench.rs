//! Micro benchmarks: rasterizer throughput and CPU interpreter speed.
use ps1core::psx::Psx;
use std::time::Instant;

fn main() {
    let mut psx = Psx::new();
    let g = &mut psx.bus.gpu;
    g.gp1(0x0800_0027); // 640x480 interlaced
    g.gp0(0xE100_0200);
    g.gp0(0xE300_0000);
    g.gp0(0xE407_FE7F); // 639x511
    g.gp0(0xE500_0000);
    let t = Instant::now();
    let n = 60;
    for _ in 0..n {
        // two full-screen gouraud triangles
        for w in [0x3800_00FF, 0x0000_0000, 0x0000_FF00, 0x0000_027F, 0x00FF_0000, 0x01DF_0000, 0x00FF_FFFF, 0x01DF_027F] {
            g.gp0(w);
        }
        // 20 textured 64x64 sprites
        for k in 0..20u32 {
            g.gp0(0x6480_8080);
            g.gp0((k * 30) | ((k * 20) << 16));
            g.gp0(0x0000_0000);
            g.gp0(0x0040_0040);
        }
    }
    let dt = t.elapsed().as_secs_f64();
    println!("GPU: {:.2} ms/frame ({} frames, 640x480 gouraud quad + 20 sprites)", dt * 1000.0 / n as f64, n);

    // CPU: tight loop with loads/stores/branches, no BIOS
    let base = 0x8001_0000u32;
    let code: Vec<u32> = vec![
        0x3C08_8000, // lui t0,0x8000
        0x2509_0001, // addiu t1,t0,1
        0xAD09_0100, // sw t1,0x100(t0)
        0x8D0A_0100, // lw t2,0x100(t0)
        0x0000_0000,
        0x0149_5821, // addu t3,t2,t1
        0x0800_4001, // j base+4
        0x0000_0000,
    ];
    let mut exe = vec![0u8; 0x800];
    exe[0..8].copy_from_slice(b"PS-X EXE");
    exe[0x10..0x14].copy_from_slice(&base.to_le_bytes());
    exe[0x18..0x1C].copy_from_slice(&base.to_le_bytes());
    exe[0x1C..0x20].copy_from_slice(&((code.len() * 4) as u32).to_le_bytes());
    for w in &code { exe.extend_from_slice(&w.to_le_bytes()); }
    let mut psx = Psx::new();
    psx.load_exe(&exe).unwrap();
    let t = Instant::now();
    for _ in 0..60 { psx.run_frame(); }
    let dt = t.elapsed().as_secs_f64();
    println!("CPU: {:.2} ms/frame emulated (realtime budget 16.7 ms)", dt * 1000.0 / 60.0);
}
