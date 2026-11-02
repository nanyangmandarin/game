//! CHD / PBP readers must return exactly the sectors of the original BIN.
//! Images come from tools/make_test_images.py (+ MAME chdman for CHD); set
//! PS1_TESTIMG_DIR to the output directory to run.

use ps1core::cdrom::{Disc, DiscReader};
use ps1core::disc_image::{open_chd, open_pbp};
use std::io::{Read, Seek, SeekFrom};

struct FileReader(std::fs::File);
impl DiscReader for FileReader {
    fn read(&mut self, _file: u32, offset: u64, buf: &mut [u8]) -> bool {
        self.0.seek(SeekFrom::Start(offset)).is_ok() && self.0.read_exact(buf).is_ok()
    }
}

fn open(path: &std::path::Path) -> (Box<dyn DiscReader>, u64) {
    let f = std::fs::File::open(path).unwrap();
    let size = f.metadata().unwrap().len();
    (Box::new(FileReader(f)), size)
}

fn compare(disc: &mut Disc, bin: &[u8], label: &str) {
    // BIN/CUE layout: absolute frame f lives at bin frame f - 150
    assert_eq!(disc.tracks.len(), 2, "{label}: track count");
    assert_eq!(disc.tracks[0].start, 150, "{label}: track 1 start");
    assert_eq!(disc.tracks[1].start, 150 + 3000 + 150, "{label}: track 2 start");
    assert!(disc.tracks[1].audio && !disc.tracks[0].audio, "{label}: track types");
    let mut buf = [0u8; 2352];
    let mut checked = 0;
    for t in disc.tracks.clone() {
        for f in t.start..t.start + t.length {
            assert!(disc.read_sector(f, &mut buf), "{label}: read {f}");
            let o = (f as usize - 150) * 2352;
            assert!(buf[..] == bin[o..o + 2352], "{label}: sector {f} differs");
            checked += 1;
        }
    }
    // PBP TOCs carry no INDEX 00, so track 1 also covers track 2's pregap (150 frames).
    assert!(checked == 3500 || checked == 3650, "{label}: sectors checked = {checked}");
}

#[test]
fn chd_and_pbp_match_bin() {
    let Ok(dir) = std::env::var("PS1_TESTIMG_DIR") else { return };
    let dir = std::path::Path::new(&dir);
    let bin = std::fs::read(dir.join("test.bin")).unwrap();

    for codec in ["cdlz", "cdzl", "cdfl"] {
        let (r, size) = open(&dir.join(format!("test-{codec}.chd")));
        let img = open_chd(r, size).unwrap_or_else(|e| panic!("{codec}: {e}"));
        let mut disc = Disc::new(img.tracks, img.reader);
        compare(&mut disc, &bin, codec);
    }

    let (r, size) = open(&dir.join("test.pbp"));
    let img = open_pbp(r, size, 0).unwrap();
    assert_eq!(img.disc_count, 1);
    let mut disc = Disc::new(img.tracks, img.reader);
    compare(&mut disc, &bin, "pbp");

    for d in 0..2 {
        let (r, size) = open(&dir.join("multi.pbp"));
        let img = open_pbp(r, size, d).unwrap();
        assert_eq!(img.disc_count, 2, "multi-disc count");
        let mut disc = Disc::new(img.tracks, img.reader);
        compare(&mut disc, &bin, &format!("multi.pbp disc {}", d + 1));
    }
}

/// With a BIOS, booting a compressed image must make the CD controller read sectors.
#[test]
fn bios_reads_compressed_images() {
    let (Ok(dir), Ok(bios)) = (std::env::var("PS1_TESTIMG_DIR"), std::env::var("PS1_BIOS")) else { return };
    let bios = std::fs::read(bios).unwrap();
    let dir = std::path::Path::new(&dir);
    for name in ["test-cdlz.chd", "test.pbp"] {
        let (r, size) = open(&dir.join(name));
        let img = if name.ends_with(".chd") { open_chd(r, size) } else { open_pbp(r, size, 0) }.unwrap();
        let mut psx = ps1core::psx::Psx::new();
        psx.load_bios(&bios).unwrap();
        psx.bus.cd.insert(Some(Disc::new(img.tracks, img.reader)));
        let mut sectors = 0;
        for _ in 0..900 {
            psx.run_frame();
            sectors += std::mem::take(&mut psx.bus.cd.stat_sectors);
        }
        println!("{name}: BIOS read {sectors} sectors");
        assert!(sectors > 0, "{name}: no sectors read");
    }
}

/// Fast boot reaches the disc boot (sector reads) much earlier than the normal intro.
#[test]
fn fast_boot_skips_intro() {
    let (Ok(dir), Ok(bios)) = (std::env::var("PS1_TESTIMG_DIR"), std::env::var("PS1_BIOS")) else { return };
    let bios = std::fs::read(bios).unwrap();
    let dir = std::path::Path::new(&dir);
    let first_read = |fast: bool| {
        let (r, size) = open(&dir.join("test.pbp"));
        let img = open_pbp(r, size, 0).unwrap();
        let mut psx = ps1core::psx::Psx::new();
        psx.load_bios(&bios).unwrap();
        psx.fast_boot = fast;
        psx.bus.cd.insert(Some(Disc::new(img.tracks, img.reader)));
        for f in 0..1200 {
            psx.run_frame();
            if std::mem::take(&mut psx.bus.cd.stat_sectors) > 0 {
                return Some(f);
            }
        }
        None
    };
    let normal = first_read(false).expect("normal boot reads disc");
    let fast = first_read(true).expect("fast boot reads disc");
    println!("first disc read: normal frame {normal}, fast boot frame {fast}");
    assert!(fast + 60 < normal, "fast boot should be clearly earlier");
}

/// The serial comes from the BOOT line of SYSTEM.CNF (ISO9660), for any image format.
#[test]
fn serial_from_system_cnf() {
    let Ok(dir) = std::env::var("PS1_TESTIMG_DIR") else { return };
    let path = std::path::Path::new(&dir).join("serial.iso");
    if !path.exists() {
        return;
    }
    let (r, size) = open(&path);
    let frames = (size / 2048) as u32;
    let track = ps1core::cdrom::Track { number: 1, file: 0, start: 150, length: frames, sector_size: 2048, audio: false, file_offset: 0 };
    let mut disc = Disc::new(vec![track], r);
    assert_eq!(disc.serial().as_deref(), Some("SLUS-01115"));
}
