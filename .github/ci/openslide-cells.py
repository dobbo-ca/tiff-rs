#!/usr/bin/env python3
"""openslide-cells.py [-max N] [-skip LOG] <out-dir> <slide>...

The OpenSlide side of the Aperio colour comparison (docs/parity.md). A manual test oracle: it runs
with Debian's python3-openslide and is never a dependency of the crate.

For each TIFF page (IFD) of each slide that OpenSlide shows as a pyramid level or an associated
image of the same size, it writes the cells that examples/quality.rs writes for that page: a grid
of 256 x 256 cells, row-major, edge cells cropped, cell floor(j * n / m) for j < m = min(n, N),
as 8-bit PPM `<slide file name, . as _>.ifd<k>.<cell:06>.ppm`. One line per page goes to stdout:
`<slide> ifd<k> <what>`. Pages listed in LOG (examples/quality.rs's log of the pages it skips or
fails, `<slide> ifd<k> <class>`) are not read.

The pixels are OpenSlide's own decode. Level 0 cells come from read_region at level 0 (downsample
1, integer offsets: a copy of the decoded tiles). Associated images are whole images. A level k > 0
is exact only near its origin: OpenSlide paints a level at x / downsample, its downsample is not an
integer, and read_region splits a request into 4096 x 4096 pieces, each of which but the first
starts at a fraction of a pixel and is resampled (bilinear). So at k > 0 only the sampled cells that
lie inside the first piece, (0, 0) to (4096, 4096), are written, from one read of that piece.
"""
import os
import struct
import sys

import openslide

CELL = 256
# openslide_read_region's piece size (openslide.c): a level k > 0 is exact only inside the first.
PIECE = 4096


def ifd_sizes(path):
    """(width, height) of every IFD of a classic TIFF, in file order."""
    with open(path, "rb") as f:
        head = f.read(8)
        e = {b"II": "<", b"MM": ">"}[head[:2]]
        if struct.unpack(e + "H", head[2:4])[0] != 42:
            raise SystemExit(f"{path}: not a classic TIFF")
        off = struct.unpack(e + "I", head[4:8])[0]
        out, seen = [], set()
        while off and off not in seen:
            seen.add(off)
            f.seek(off)
            n = struct.unpack(e + "H", f.read(2))[0]
            ent = f.read(12 * n)
            tags = {}
            for i in range(n):
                tag, typ, _ = struct.unpack(e + "HHI", ent[12 * i : 12 * i + 8])
                v = ent[12 * i + 8 : 12 * i + 12]
                tags[tag] = struct.unpack(e + ("H" if typ == 3 else "I"), v[: 2 if typ == 3 else 4])[0]
            out.append((tags.get(256), tags.get(257)))
            off = struct.unpack(e + "I", f.read(4))[0]
        return out


def cells(w, h, max_cells):
    cx, cy = -(-w // CELL), -(-h // CELL)
    n = cx * cy
    m = min(n, max_cells)
    for j in range(m):
        c = j * n // m
        x0, y0 = (c % cx) * CELL, (c // cx) * CELL
        yield c, x0, y0, min(CELL, w - x0), min(CELL, h - y0)


def save(img, path):
    if img.mode == "RGBA" and img.getextrema()[3][0] != 255:
        raise SystemExit(f"{path}: OpenSlide returned transparent pixels")
    img.convert("RGB").save(path, "PPM")


def slide(out, path, max_cells, skip):
    s = openslide.OpenSlide(path)
    stem = os.path.basename(path).replace(".", "_").replace("/", "_")
    levels = {d: k for k, d in enumerate(s.level_dimensions)}
    assoc = {img.size: (name, img) for name, img in s.associated_images.items()}
    for k, (w, h) in enumerate(ifd_sizes(path)):
        base = f"{out}/{stem}.ifd{k}"
        if (path, k) in skip:
            print(f"{path} ifd{k} skipped: {skip[(path, k)]}")
        elif (w, h) in levels:
            lv = levels.pop((w, h))
            print(f"{path} ifd{k} level{lv}")
            piece = None
            if lv > 0:
                piece = s.read_region((0, 0), lv, (min(w, PIECE), min(h, PIECE)))
            for c, x0, y0, cw, ch in cells(w, h, max_cells):
                if piece is None:
                    img = s.read_region((x0, y0), 0, (cw, ch))
                elif x0 + cw <= PIECE and y0 + ch <= PIECE:
                    img = piece.crop((x0, y0, x0 + cw, y0 + ch))
                else:
                    continue
                save(img, f"{base}.{c:06d}.ppm")
            piece = None
        elif (w, h) in assoc:
            name, img = assoc.pop((w, h))
            print(f"{path} ifd{k} {name}")
            for c, x0, y0, cw, ch in cells(w, h, max_cells):
                save(img.crop((x0, y0, x0 + cw, y0 + ch)), f"{base}.{c:06d}.ppm")
        else:
            print(f"{path} ifd{k} none")
    s.close()


def main():
    a = sys.argv[1:]
    max_cells, skip = 150, {}
    while a[:1] in (["-max"], ["-skip"]):
        if a[0] == "-max":
            max_cells = int(a[1])
        else:
            for line in open(a[1]):
                p, k, why = (line.split(" ", 2) + ["", ""])[:3]
                if k.startswith("ifd"):
                    skip[(p, int(k.removeprefix("ifd")))] = why.strip()
        a = a[2:]
    if len(a) < 2:
        raise SystemExit("usage: openslide-cells.py [-max N] [-skip LOG] <out-dir> <slide>...")
    os.makedirs(a[0], exist_ok=True)
    print(f"OpenSlide {openslide.__library_version__}, openslide-python {openslide.__version__}")
    for p in a[1:]:
        slide(a[0], p, max_cells, skip)


main()
