#!/usr/bin/env python3
"""Generate synthetic disc images to test the CHD/PBP readers.

Creates in OUT_DIR:
  test.bin / test.cue  - MODE2/2352 data track (3000 frames) + AUDIO track with a
                         2-second pregap stored in the file (500 frames after INDEX 01)
  test.pbp             - single-disc PBP of test.bin (raw-deflate blocks of 16 frames)
  multi.pbp            - two-disc PBP (both discs = test.bin)
CHD files are made with MAME's chdman; serial.iso (ISO9660 with SYSTEM.CNF) with genisoimage
(see .github/workflows/pages.yml).
"""
import os
import random
import struct
import sys
import zlib

RAW = 2352
DATA_FRAMES = 3000
PREGAP = 150
AUDIO_FRAMES = 500


def bcd(v):
    return ((v // 10) << 4) | (v % 10)


def msf(frame):
    return bcd(frame // 75 // 60), bcd((frame // 75) % 60), bcd(frame % 75)


def make_bin():
    rnd = random.Random(1234)
    out = bytearray()
    for i in range(DATA_FRAMES):
        s = bytearray(RAW)
        s[0:12] = b"\x00" + b"\xff" * 10 + b"\x00"
        m, sec, f = msf(150 + i)
        s[12:16] = bytes([m, sec, f, 2])
        s[16:24] = bytes([0, 0, 8, 0, 0, 0, 8, 0])
        s[24:RAW] = rnd.randbytes(RAW - 24) if i % 7 else bytes((j * i) & 0xFF for j in range(RAW - 24))
        out += s
    for i in range(PREGAP + AUDIO_FRAMES):
        if i < PREGAP:
            out += bytes(RAW)
        else:
            # little-endian 16-bit stereo PCM (a sweeping square-ish wave)
            out += b"".join(struct.pack("<hh", ((j + i) % 200 - 100) * 300, ((j * 3 + i) % 150 - 75) * 400) for j in range(588))
    return bytes(out)


def make_pbp(image, discs=1):
    total_frames = len(image) // RAW
    blocks = []
    for b in range(0, total_frames, 16):
        raw = image[b * RAW:(b + 16) * RAW].ljust(16 * RAW, b"\0")
        c = zlib.compressobj(9, zlib.DEFLATED, -15)
        z = c.compress(raw) + c.flush()
        blocks.append(raw if len(z) >= len(raw) else z)

    def psisoimg():
        img = bytearray(0x100000)
        img[0:12] = b"PSISOIMG0000"
        t1 = 150
        t2 = 150 + DATA_FRAMES + PREGAP
        lead = t2 + AUDIO_FRAMES
        toc = [
            bytes([0x41, 0, 0xA0, 0, 0, 0, 0, 1, 0x20, 0]),
            bytes([0x01, 0, 0xA1, 0, 0, 0, 0, bcd(2), 0, 0]),
            bytes([0x01, 0, 0xA2, 0, 0, 0, 0, *msf(lead)]),
            bytes([0x41, 0, 1, 0, 0, 0, 0, *msf(t1)]),
            bytes([0x01, 0, 2, *msf(t2 - 150 - PREGAP), 0, *msf(t2)]),
        ]
        o = 0x800
        for e in toc:
            img[o:o + 10] = e
            o += 10
        data = bytearray()
        o = 0x4000
        for blk in blocks:
            img[o:o + 8] = struct.pack("<II", len(data), len(blk))
            o += 32
            data += blk
        return bytes(img) + bytes(data)

    hdr = bytearray(0x28)
    hdr[0:4] = b"\0PBP"
    hdr[4:8] = struct.pack("<I", 0x10000)
    for k in range(8):
        hdr[8 + 4 * k:12 + 4 * k] = struct.pack("<I", 0x28)
    if discs == 1:
        psar = psisoimg()
    else:
        title = bytearray(0x400)
        title[0:16] = b"PSTITLEIMG000000"
        isos = [psisoimg() for _ in range(discs)]
        off = len(title)
        for d, iso in enumerate(isos):
            title[0x200 + 4 * d:0x204 + 4 * d] = struct.pack("<I", off)
            off += len(iso)
        psar = bytes(title) + b"".join(isos)
    return bytes(hdr) + psar


def main():
    out = sys.argv[1] if len(sys.argv) > 1 else "."
    os.makedirs(out, exist_ok=True)
    image = make_bin()
    open(os.path.join(out, "test.bin"), "wb").write(image)
    t2_idx0 = DATA_FRAMES
    t2_idx1 = DATA_FRAMES + PREGAP
    cue = (
        'FILE "test.bin" BINARY\n'
        "  TRACK 01 MODE2/2352\n    INDEX 01 00:00:00\n"
        "  TRACK 02 AUDIO\n"
        f"    INDEX 00 {t2_idx0 // 4500:02d}:{t2_idx0 // 75 % 60:02d}:{t2_idx0 % 75:02d}\n"
        f"    INDEX 01 {t2_idx1 // 4500:02d}:{t2_idx1 // 75 % 60:02d}:{t2_idx1 % 75:02d}\n"
    )
    open(os.path.join(out, "test.cue"), "w").write(cue)
    open(os.path.join(out, "test.pbp"), "wb").write(make_pbp(image, 1))
    open(os.path.join(out, "multi.pbp"), "wb").write(make_pbp(image, 2))


if __name__ == "__main__":
    main()
