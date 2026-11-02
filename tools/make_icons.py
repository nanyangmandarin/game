#!/usr/bin/env python3
"""Render the app icons (PNG, no dependencies): dark rounded tile with △ ○ ✕ □."""
import math
import struct
import sys
import zlib

def png(path, w, h, px):
    raw = b"".join(b"\0" + bytes(px[y * w * 4:(y + 1) * w * 4]) for y in range(h))
    def chunk(t, d):
        c = t + d
        return struct.pack(">I", len(d)) + c + struct.pack(">I", zlib.crc32(c) & 0xFFFFFFFF)
    open(path, "wb").write(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0))
                           + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))

def render(size, maskable):
    S = 3  # supersampling
    N = size * S
    bg = (23, 26, 33)
    cols = {"tri": (95, 224, 180), "cir": (255, 107, 107), "crs": (143, 183, 255), "sq": (243, 155, 208)}
    pad = 0 if maskable else 0.06
    radius = 0 if maskable else 0.22
    scale = 0.62 if maskable else 0.8  # keep symbols in the maskable safe zone
    def inside_tile(x, y):
        lo, hi = pad, 1 - pad
        if not (lo <= x <= hi and lo <= y <= hi):
            return False
        r = radius * (hi - lo)
        cx = min(max(x, lo + r), hi - r)
        cy = min(max(y, lo + r), hi - r)
        return (x - cx) ** 2 + (y - cy) ** 2 <= r * r + 1e-12
    def shape(x, y):
        # symbol layout in [-1, 1]
        u = (x - 0.5) / (0.5 * scale)
        v = (y - 0.5) / (0.5 * scale)
        t = 0.11  # stroke
        def ring(cx, cy, r):
            d = math.hypot(u - cx, v - cy)
            return abs(d - r) < t / 1.6
        def tri(cx, cy, r):
            # outline of an upward triangle
            pts = [(cx, cy - r), (cx + r * 0.95, cy + r * 0.62), (cx - r * 0.95, cy + r * 0.62)]
            for i in range(3):
                (x1, y1), (x2, y2) = pts[i], pts[(i + 1) % 3]
                dx, dy = x2 - x1, y2 - y1
                k = max(0, min(1, ((u - x1) * dx + (v - y1) * dy) / (dx * dx + dy * dy)))
                if math.hypot(u - (x1 + k * dx), v - (y1 + k * dy)) < t / 1.6:
                    return True
            return False
        def cross(cx, cy, r):
            a, b = u - cx, v - cy
            return (abs(a - b) < t / 1.2 or abs(a + b) < t / 1.2) and abs(a) < r * 0.75 and abs(b) < r * 0.75
        def square(cx, cy, r):
            a, b = abs(u - cx), abs(v - cy)
            m = max(a, b)
            return abs(m - r * 0.68) < t / 1.6
        r = 0.30
        if tri(0, -0.56, r): return cols["tri"]
        if ring(0.56, 0, r * 0.82): return cols["cir"]
        if cross(0, 0.56, r): return cols["crs"]
        if square(-0.56, 0, r): return cols["sq"]
        return None
    out = bytearray(size * size * 4)
    for py in range(size):
        for px_ in range(size):
            acc = [0, 0, 0, 0]
            for sy in range(S):
                for sx in range(S):
                    x = (px_ * S + sx + 0.5) / N
                    y = (py * S + sy + 0.5) / N
                    if maskable or inside_tile(x, y):
                        c = shape(x, y) or bg
                        acc[0] += c[0]; acc[1] += c[1]; acc[2] += c[2]; acc[3] += 255
            n = S * S
            o = (py * size + px_) * 4
            out[o:o + 4] = bytes([acc[0] // n, acc[1] // n, acc[2] // n, acc[3] // n])
    return out

out = sys.argv[1] if len(sys.argv) > 1 else "web/icons"
for size, mask, name in [(192, False, "icon-192.png"), (512, False, "icon-512.png"), (512, True, "icon-maskable-512.png"), (180, True, "apple-touch-icon.png"), (32, False, "favicon-32.png")]:
    png(f"{out}/{name}", size, size, render(size, mask))
