#!/usr/bin/env python3
"""Build web/data/ps1-cheats.json: which libretro-database cheat file belongs to which game.

Inputs (from https://github.com/libretro/libretro-database):
  cht/Sony - PlayStation/*.cht       (directory listing only)
  metadat/redump/Sony - PlayStation.dat  (serial -> title)
Usage: build_cheat_index.py <cht_dir> <redump.dat> <out.json>
Only GameShark-format files are indexed (Xploder/Game Buster use other encodings).
"""
import json
import os
import re
import sys

REGION_ALIASES = {"pal": "europe", "eur": "europe", "us": "usa", "jp": "japan"}


def split_name(name):
    base = re.sub(r"\s*\([^)]*\)", "", name).strip()
    tags = [t.strip().lower() for p in re.findall(r"\(([^)]*)\)", name) for t in p.split(",")]
    tags = [REGION_ALIASES.get(t, t) for t in tags]
    return base, tags


def norm(title):
    t = title.lower().replace("&", "and")
    t = re.sub(r"[^a-z0-9]+", " ", t)
    return re.sub(r"\s+", " ", t).strip()


def usable(fname):
    return not re.search(r"\((Game Buster|Xploder|Action Replay Max)\)", fname, re.I)


def main():
    cht_dir, dat_path, out = sys.argv[1:4]
    files = sorted(f for f in os.listdir(cht_dir) if f.endswith(".cht") and usable(f))
    by_title = {}
    for i, f in enumerate(files):
        base, tags = split_name(f[:-4])
        by_title.setdefault(norm(base), []).append((i, tags))

    serials = {}
    text = open(dat_path, encoding="utf-8", errors="replace").read()
    for block in re.findall(r"game \((.*?)\n\)", text, re.S):
        m_name = re.search(r'^\s*name "([^"]+)"', block, re.M)
        m_serial = re.search(r'^\s*serial "([^"]+)"', block, re.M)
        if not m_name or not m_serial:
            continue
        name = re.sub(r"\s*\((Disc|Disk) \d+\)", "", m_name.group(1))
        base, tags = split_name(name)
        cands = by_title.get(norm(base))
        if not cands:
            continue
        regions = {t for t in tags if t in ("usa", "europe", "japan", "world", "asia", "korea")}
        best = max(cands, key=lambda c: (len(regions & set(c[1])) + (1 if "world" in c[1] else 0), "usa" in c[1]))
        for s in re.split(r"[,\s]+", m_serial.group(1)):
            s = s.strip().upper()
            if re.match(r"^[A-Z]{4}-\d{5}$", s):
                serials[s] = best[0]

    json.dump({"source": "libretro-database cht/Sony - PlayStation", "files": files, "serials": serials},
              open(out, "w"), separators=(",", ":"))
    print(f"{len(files)} cheat files, {len(serials)} serials mapped -> {out}")


if __name__ == "__main__":
    main()
