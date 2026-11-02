# PS1 Web Emulator

Emulator PlayStation 1 yang berjalan sepenuhnya di browser. Core ditulis dalam Rust dan dikompilasi ke **WebAssembly**. Semua proses terjadi di perangkat pengguna; tidak ada file game atau BIOS yang diunggah ke server.

## Fitur

| Area | Status |
|---|---|
| CPU MIPS R3000A (interpreter, load/branch delay slot, exception, COP0) | ✅ |
| GTE (koprosesor 3D, semua perintah + flag) | ✅ |
| GPU: poligon flat/gouraud/bertekstur (4/8/15-bit, CLUT, texture window), sprite, garis, semi-transparansi, mask bit, dithering, transfer VRAM, mode 24-bit | ✅ |
| CD-ROM: ISO, BIN/CUE (multi-track, multi-file), **CHD**, **PBP** (termasuk multi-disc), deteksi region, CD-DA, XA-ADPCM | ✅ |
| Ganti disc untuk game multi-disc (PBP multi-disc atau file disc terpisah) | ✅ |
| SPU: 24 voice ADPCM, ADSR, noise, pitch modulation, reverb, mixing CD audio | ✅ |
| MDEC (video FMV) | ✅ |
| Kontroler digital & DualShock (analog + getar), 2 port | ✅ |
| Jalankan BIOS tanpa disc (menu memory card / CD player) | ✅ |
| Memory card 2 slot (bersama / per game), impor/ekspor `.mcr`/`.gme`, kelola save | ✅ |
| Timer, DMA, interrupt | ✅ |
| Save state: 5 slot + quick save (F5/F8) + auto-save, dengan thumbnail | ✅ |
| Game & progres tetap ada setelah refresh (OPFS + IndexedDB) | ✅ |
| Kontrol layar sentuh (HP/tablet): multi-touch, D-pad, stik analog, haptic | ✅ |

### Renderer: dengan atau tanpa GPU

Pilihannya ada di **Pengaturan → Grafik → Renderer**:

- **Software (CPU)**: rasterizer CPU + Canvas 2D. Jalan di perangkat apa pun, termasuk yang tanpa GPU atau driver grafisnya bermasalah.
- **Hardware (GPU / WebGL)**: VRAM mentah dikirim ke GPU dan konversi warna, skala, serta filter (nearest, bilinear, sharp bilinear, CRT) dikerjakan shader. Kalau WebGL tidak tersedia, otomatis kembali ke Software.

Emulasi berjalan di **Web Worker**, jadi UI tetap responsif. Di perangkat multi-core, **emulasi GPU berjalan di thread terpisah**: CPU mengerjakan frame berikutnya sementara GPU menggambar frame sekarang. Hasilnya tetap identik dengan satu thread (diuji byte per byte). Mode ini butuh `SharedArrayBuffer`; karena GitHub Pages tidak bisa mengirim header COOP/COEP, `web/sw.js` menambahkannya, dan halaman dimuat ulang sekali pada kunjungan pertama. Bisa dimatikan di Pengaturan → Sistem. Audio memakai **AudioWorklet** dengan sinkronisasi kecepatan berbasis buffer.

### Pengaturan lain
- **Sistem:** BIOS, batas kecepatan (50%–200%/tanpa batas), fast-forward, pause saat tab tidak aktif, overlay FPS.
- **Grafik:** rasio aspek (4:3, 16:9, 1:1, penuh), skala integer, filter, intensitas CRT, dithering, tampilan VRAM.
- **Audio:** volume (sampai 200%), latensi, reverb, interpolasi, bisu saat fast-forward.
- **Kontroler:** jenis per port, pilih gamepad, deadzone, getar, remap keyboard & gamepad.
- **Memory card:** mode per slot, daftar save (judul Shift-JIS), hapus save, impor/ekspor, format.
- **Hotkey:** bisa diubah semua.

## Tetap tersimpan setelah refresh

- **Game** yang dibuka otomatis disalin ke penyimpanan browser (OPFS) dan muncul di **Game Saya**. Setelah refresh tidak perlu memilih file lagi.
- **Progres** disimpan otomatis tiap 30 detik dan saat tab disembunyikan/ditutup. Tombol **Lanjutkan** membuka game persis di titik terakhir.
- **Save state** manual: tombol 💾 State (5 slot + slot cepat F5/F8).
- Bisa dimatikan di Pengaturan → Sistem. Menghapus data situs di browser juga menghapus game dan save yang tersimpan.

## Di HP / tablet

Kontrol layar sentuh muncul otomatis di perangkat sentuh (atur di Pengaturan → Kontroler).

- **Kontrol arah** (satu slot di kiri, pilih salah satu): D-pad, Joystick arah, Joystick arah mengambang, Stik analog, atau Stik analog mengambang. Tombol **A** di layar menyalakan mode analog DualShock (lampu merah). Saat mode digital, stik analog otomatis menggerakkan D-pad sehingga tetap berfungsi di game yang tidak mendukung analog.
- **Gestur langsung di layar game**: tap = ✕, tap 2 jari = ○ (keduanya bisa diganti), tekan & tahan = menahan tombol, geser & tahan = arah. Opsi *hanya gestur* menyembunyikan tombol (Start/Select tetap ada) sehingga gambar game memenuhi layar. Mode potret: layar di atas, tombol di bawah. Mode lanskap: tombol di kiri-kanan gambar dan toolbar disembunyikan. Tombol ☰ ⏩ 💾 ↺ di layar untuk menu, fast-forward, quick save, dan quick load.

## Cara pakai

1. Buka aplikasinya, lalu pilih **BIOS** PS1 (512 KB, misalnya SCPH-1001 / SCPH-5501 / SCPH-7502). BIOS disimpan di IndexedDB browser.
   > BIOS adalah milik Sony dan **tidak disertakan**. Ambil dari konsol milik Anda sendiri.
2. Klik **Buka Game** atau seret file ke layar:
   - `.iso` / `.bin` saja (2048 atau 2352 byte/sektor, terdeteksi otomatis), atau
   - `.cue` **bersama** semua file `.bin`-nya (pilih sekaligus), atau
   - `.chd` (MAME/chdman: codec LZMA, Deflate, FLAC, Zstd), atau
   - `.pbp` (EBOOT PS1 dari PSP/PS3, termasuk multi-disc), atau
   - `.exe` (PS-X EXE, untuk homebrew; bisa tanpa BIOS).

   Game multi-disc: saat game meminta disc berikutnya, tekan **💿 Disc** lalu pilih disc (PBP) atau file disc berikutnya.

### Kontrol default (keyboard, Port 1)

| PS1 | Tombol | | PS1 | Tombol |
|---|---|---|---|---|
| D-Pad | W A S D | | △ ○ ✕ □ | I L K J |
| L1 / R1 | Q / E | | L2 / R2 | 1 / 3 |
| Start / Select | Enter / Backspace | | L3 / R3 | Z / X |
| Stik kiri | Panah | | Stik kanan | Numpad 8/4/2/6 |

Hotkey: **P** pause, **F5** quick save, **F8** quick load, **Tab** (tahan) fast-forward, **F2** layar penuh, **F3** reset, **F4** toggle analog, **F9** screenshot, **Esc** menu. Gamepad standar (Xbox/PS/dll.) langsung terpetakan; tombol Home/Guide mengaktifkan mode analog.

## Build

Butuh Rust (target `wasm32-unknown-unknown`) dan `wasm-bindgen-cli` dengan versi yang sama seperti di `Cargo.lock`:

```bash
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.130 --locked
./build.sh
```

Jalankan lewat server HTTP (worker module tidak jalan dari `file://`):

```bash
cd web && python3 -m http.server 8080
```

Test core (native). Test image CHD/PBP dibuat dengan `tools/make_test_images.py` + `chdman` (lihat workflow CI):

```bash
cargo test --manifest-path core/Cargo.toml --release
```

### Deploy
Workflow `.github/workflows/pages.yml` menjalankan test, build, lalu deploy folder `web/` ke GitHub Pages pada setiap push ke `main`. Aktifkan sekali di **Settings → Pages → Source: GitHub Actions**.

## Struktur

```
core/src/
  cpu.rs     MIPS R3000A + COP0
  gte.rs     Geometry Transformation Engine (COP2)
  bus.rs     peta memori, IRQ, DMA
  gpu.rs     GP0/GP1, rasterizer software, timing video
  cdrom.rs   controller CD, image disc, CD-DA, XA-ADPCM
  spu.rs     audio 24 voice, ADSR, reverb
  mdec.rs    dekoder video FMV
  pad.rs     kontroler + memory card (SIO0)
  timers.rs  root counter
  psx.rs     mesin utama, frame stepping, sideload EXE
  lib.rs     API WebAssembly
web/
  index.html, style.css
  js/main.js        UI, pengaturan, alur game
  js/worker.js      menjalankan core di Web Worker
  js/presenter.js   renderer Software (Canvas 2D) / GPU (WebGL)
  js/audio-worklet.js, input.js, disc.js, memcard.js, storage.js
```

## Status pengujian

Yang sudah diverifikasi otomatis: aritmetika CPU, semantik load delay slot, rasterizer GPU (lewat program uji PS-X EXE), dan UI di Chromium dengan kedua renderer. Kompatibilitas game komersial **belum diuji dengan BIOS dan game asli**. Silakan laporkan game yang bermasalah lewat Issues.
