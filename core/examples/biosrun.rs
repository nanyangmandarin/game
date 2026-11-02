//! Boot a BIOS (no disc) and report per-frame timing + counters.
//! Usage: cargo run --release --example biosrun -- <bios.bin> [frames] [out.png]
use ps1core::psx::Psx;
use std::time::Instant;

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
                "frames {:4}-{:4}: {:6.2} ms/frame  pc={:08X} disp={}x{} px={}k cmds={} exc={}",
                f - 59, f, chunk.elapsed().as_secs_f64() * 1000.0 / 60.0, psx.cpu.pc, w, h,
                std::mem::take(&mut g.stat_pixels) / 1000, std::mem::take(&mut g.stat_cmds),
                std::mem::take(&mut psx.cpu.stat_exceptions)
            );
            chunk = Instant::now();
        }
    }
    println!("total {:.1} ms, worst frame {:.2} ms at {}", t0.elapsed().as_secs_f64() * 1000.0, worst.0, worst.1);
}
