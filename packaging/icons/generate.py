#!/usr/bin/env python3
"""Render the Ferrocord icon set from ``ferrocord.svg``.

The SVG is the source of truth. It mirrors the window icon drawn by
``app_icon()`` in ``crates/app/src/main.rs``: an orange (#e06b2b) superellipse
``|x|^4 + |y|^4 <= r^4`` with a white "F", on a 64-unit grid.

Only the Python standard library is needed, so the outputs can be regenerated
anywhere. The rasterizer understands exactly what ``ferrocord.svg`` uses:
``<path>`` elements with a ``fill`` colour whose ``d`` attribute is made of
absolute ``M``/``L``/``H``/``V``/``Z`` commands (straight-edged polygons). Keep
the SVG within that subset when editing it.

Usage::

    python3 packaging/icons/generate.py             # render all outputs from the SVG
    python3 packaging/icons/generate.py --write-svg # rewrite the SVG from the parameters below, then render

Outputs (committed, so CI needs no image tools):

* ``hicolor/<N>x<N>/apps/dev.ferrocord.Ferrocord.png`` for the freedesktop sizes
* ``hicolor/scalable/apps/dev.ferrocord.Ferrocord.svg``
* ``ferrocord.ico`` (16-256 px, embedded in ferrocord.exe by ``crates/app/build.rs``)
* ``ferrocord.icns`` (16-1024 px, used for the macOS app bundle)
"""

from __future__ import annotations

import argparse
import math
import re
import shutil
import struct
import sys
import xml.etree.ElementTree as ET
import zlib
from pathlib import Path

HERE = Path(__file__).resolve().parent
SVG_PATH = HERE / "ferrocord.svg"
APP_ID = "dev.ferrocord.Ferrocord"

HICOLOR_SIZES = [16, 22, 24, 32, 48, 64, 128, 256, 512]
ICO_SIZES = [16, 20, 24, 32, 40, 48, 64, 256]
# (OSType, pixel size) pairs for the .icns container; all entries are PNG encoded.
ICNS_ENTRIES = [
    (b"icp4", 16),
    (b"icp5", 32),
    (b"icp6", 64),
    (b"ic07", 128),
    (b"ic08", 256),
    (b"ic09", 512),
    (b"ic10", 1024),
    (b"ic11", 32),
    (b"ic12", 64),
    (b"ic13", 256),
    (b"ic14", 512),
]
# Below this size, axis-aligned shapes (the "F") are snapped to the pixel grid.
HINT_BELOW = 64

# ---------------------------------------------------------------------------
# SVG authoring (geometry identical to app_icon() in crates/app/src/main.rs)
# ---------------------------------------------------------------------------
GRID = 64
ACCENT = "#e06b2b"
GLYPH = "#ffffff"
CENTER = 32.0
RADIUS = 31.0
EXPONENT = 4.0
# app_icon() fills pixel columns 18..=26 etc.; as continuous coordinates the
# "F" outline is the union of [18,27]x[14,51], [18,47]x[14,22] and [18,41]x[29,36].
F_OUTLINE = [(18, 14), (47, 14), (47, 22), (27, 22), (27, 29), (41, 29), (41, 36), (27, 36), (27, 51), (18, 51)]


def superellipse_points(segments: int = 256) -> list[tuple[float, float]]:
    pts = []
    for i in range(segments):
        t = 2 * math.pi * i / segments
        c, s = math.cos(t), math.sin(t)
        x = CENTER + RADIUS * math.copysign(abs(c) ** (2 / EXPONENT), c)
        y = CENTER + RADIUS * math.copysign(abs(s) ** (2 / EXPONENT), s)
        pts.append((round(x, 3), round(y, 3)))
    return pts


def fmt(v: float) -> str:
    return f"{v:.3f}".rstrip("0").rstrip(".")


def polygon_d(pts: list[tuple[float, float]]) -> str:
    head, *rest = pts
    return f"M{fmt(head[0])} {fmt(head[1])}" + "".join(f"L{fmt(x)} {fmt(y)}" for x, y in rest) + "Z"


def write_svg() -> None:
    svg = (
        '<?xml version="1.0" encoding="UTF-8"?>\n'
        f'<!-- Ferrocord app icon. Rendered to PNG/ICO/ICNS by generate.py; keep to M/L/H/V/Z paths. -->\n'
        f'<svg xmlns="http://www.w3.org/2000/svg" width="512" height="512" viewBox="0 0 {GRID} {GRID}">\n'
        "  <title>Ferrocord</title>\n"
        f'  <path id="squircle" fill="{ACCENT}" d="{polygon_d(superellipse_points())}"/>\n'
        f'  <path id="glyph" fill="{GLYPH}" d="{polygon_d(F_OUTLINE)}"/>\n'
        "</svg>\n"
    )
    SVG_PATH.write_text(svg, encoding="utf-8", newline="\n")


# ---------------------------------------------------------------------------
# Minimal SVG reader and anti-aliased polygon rasterizer
# ---------------------------------------------------------------------------
Shape = tuple[tuple[int, int, int, int], list[list[tuple[float, float]]]]


def parse_color(value: str) -> tuple[int, int, int, int]:
    value = value.strip()
    if not re.fullmatch(r"#[0-9a-fA-F]{6}", value):
        raise ValueError(f"unsupported fill {value!r} (use #rrggbb)")
    return int(value[1:3], 16), int(value[3:5], 16), int(value[5:7], 16), 255


def parse_path(d: str) -> list[list[tuple[float, float]]]:
    tokens = re.findall(r"[MLHVZmlhvz]|-?\d*\.?\d+(?:[eE][-+]?\d+)?", d)
    polys: list[list[tuple[float, float]]] = []
    cur: list[tuple[float, float]] = []
    cmd = None
    x = y = 0.0
    i = 0
    while i < len(tokens):
        tok = tokens[i]
        if tok.isalpha():
            if tok.islower():
                raise ValueError("relative path commands are not supported")
            cmd = tok
            i += 1
            if cmd == "Z":
                if cur:
                    polys.append(cur)
                cur = []
            continue
        if cmd in ("M", "L"):
            x, y = float(tokens[i]), float(tokens[i + 1])
            i += 2
            if cmd == "M" and cur:
                polys.append(cur)
                cur = []
            cur.append((x, y))
            if cmd == "M":
                cmd = "L"
        elif cmd == "H":
            x = float(tok)
            i += 1
            cur.append((x, y))
        elif cmd == "V":
            y = float(tok)
            i += 1
            cur.append((x, y))
        else:
            raise ValueError(f"unexpected token {tok!r} in path")
    if cur:
        polys.append(cur)
    return polys


def load_svg(path: Path) -> tuple[float, float, list[Shape]]:
    root = ET.parse(path).getroot()
    ns = "{http://www.w3.org/2000/svg}"
    vb = [float(v) for v in root.attrib["viewBox"].replace(",", " ").split()]
    shapes: list[Shape] = []
    for el in root.iter(f"{ns}path"):
        shapes.append((parse_color(el.attrib["fill"]), parse_path(el.attrib["d"])))
    if not shapes:
        raise ValueError("no <path> elements found")
    return vb[2], vb[3], shapes


def coverage(polys: list[list[tuple[float, float]]], size: int, scale: float, ss: int) -> list[float]:
    """Per-pixel coverage (0..1), non-zero winding, ``ss`` sub-scanlines per pixel row
    with exact horizontal coverage along each sub-scanline."""
    edges = []
    for poly in polys:
        for (x0, y0), (x1, y1) in zip(poly, poly[1:] + poly[:1]):
            if y0 != y1:
                edges.append((x0 * scale, y0 * scale, x1 * scale, y1 * scale))
    cov = [0.0] * (size * size)
    w = 1.0 / ss
    for py in range(size):
        row = py * size
        for sub in range(ss):
            sy = py + (sub + 0.5) * w
            xs = []
            for x0, y0, x1, y1 in edges:
                lo, hi = (y0, y1) if y0 < y1 else (y1, y0)
                if lo <= sy < hi:
                    xs.append((x0 + (sy - y0) * (x1 - x0) / (y1 - y0), 1 if y1 > y0 else -1))
            if not xs:
                continue
            xs.sort()
            wind = 0
            for k in range(len(xs) - 1):
                wind += xs[k][1]
                if wind == 0:
                    continue
                a, b = max(xs[k][0], 0.0), min(xs[k + 1][0], float(size))
                if b <= a:
                    continue
                ia, ib = int(a), int(math.ceil(b)) - 1
                if ia == ib:
                    cov[row + ia] += (b - a) * w
                    continue
                cov[row + ia] += (ia + 1 - a) * w
                for px in range(ia + 1, ib):
                    cov[row + px] += w
                cov[row + ib] += (b - ib) * w
    return cov


def hint(polys: list[list[tuple[float, float]]], scale: float) -> list[list[tuple[float, float]]]:
    """Snap rectilinear polygons to whole pixels so small icons stay crisp."""
    out = []
    for poly in polys:
        rectilinear = all(a[0] == b[0] or a[1] == b[1] for a, b in zip(poly, poly[1:] + poly[:1]))
        if rectilinear:
            poly = [(math.floor(x * scale + 0.5) / scale, math.floor(y * scale + 0.5) / scale) for x, y in poly]
        out.append(poly)
    return out


def render(size: int, svg: tuple[float, float, list[Shape]]) -> bytes:
    """Return straight-alpha RGBA bytes for a ``size`` x ``size`` rendering."""
    vw, vh, shapes = svg
    scale = size / max(vw, vh)
    ss = 16 if size <= 64 else 8 if size <= 256 else 4
    acc = [(0.0, 0.0, 0.0, 0.0)] * (size * size)  # premultiplied
    for (r, g, b, _a), polys in shapes:
        if size < HINT_BELOW:
            polys = hint(polys, scale)
        cov = coverage(polys, size, scale, ss)
        for i, c in enumerate(cov):
            if c <= 0:
                continue
            c = min(c, 1.0)
            pr, pg, pb, pa = acc[i]
            inv = 1 - c
            acc[i] = (r / 255 * c + pr * inv, g / 255 * c + pg * inv, b / 255 * c + pb * inv, c + pa * inv)
    out = bytearray(size * size * 4)
    for i, (pr, pg, pb, pa) in enumerate(acc):
        if pa <= 0:
            continue
        out[i * 4 : i * 4 + 4] = bytes(
            (round(pr / pa * 255), round(pg / pa * 255), round(pb / pa * 255), round(pa * 255))
        )
    return bytes(out)


# ---------------------------------------------------------------------------
# Encoders
# ---------------------------------------------------------------------------
def png(size: int, rgba: bytes) -> bytes:
    def chunk(tag: bytes, data: bytes) -> bytes:
        return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

    stride = size * 4
    raw = b"".join(b"\x00" + rgba[y * stride : (y + 1) * stride] for y in range(size))
    ihdr = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b"")


def bmp_dib(size: int, rgba: bytes) -> bytes:
    """32-bit BGRA DIB with an (all-opaque) AND mask, as stored in .ico files."""
    header = struct.pack("<IiiHHIIiiII", 40, size, size * 2, 1, 32, 0, 0, 0, 0, 0, 0)
    rows = []
    for y in range(size - 1, -1, -1):
        line = rgba[y * size * 4 : (y + 1) * size * 4]
        rows.append(b"".join(bytes((line[i + 2], line[i + 1], line[i], line[i + 3])) for i in range(0, len(line), 4)))
    mask_stride = ((size + 31) // 32) * 4
    return header + b"".join(rows) + b"\x00" * (mask_stride * size)


def ico(images: dict[int, bytes]) -> bytes:
    entries, blobs = [], []
    offset = 6 + 16 * len(images)
    for size in sorted(images):
        data = png(size, images[size]) if size >= 256 else bmp_dib(size, images[size])
        dim = 0 if size >= 256 else size
        entries.append(struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(data), offset))
        blobs.append(data)
        offset += len(data)
    return struct.pack("<HHH", 0, 1, len(images)) + b"".join(entries) + b"".join(blobs)


def icns(pngs: dict[int, bytes]) -> bytes:
    body = b"".join(tag + struct.pack(">I", 8 + len(pngs[size])) + pngs[size] for tag, size in ICNS_ENTRIES)
    return b"icns" + struct.pack(">I", 8 + len(body)) + body


# ---------------------------------------------------------------------------
def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--write-svg", action="store_true", help="regenerate ferrocord.svg from the parameters first")
    args = parser.parse_args()
    if args.write_svg:
        write_svg()
    svg = load_svg(SVG_PATH)

    sizes = sorted(set(HICOLOR_SIZES) | set(ICO_SIZES) | {s for _, s in ICNS_ENTRIES})
    rgba, pngs = {}, {}
    for size in sizes:
        rgba[size] = render(size, svg)
        pngs[size] = png(size, rgba[size])
        print(f"rendered {size}x{size}", file=sys.stderr)

    hicolor = HERE / "hicolor"
    for size in HICOLOR_SIZES:
        dest = hicolor / f"{size}x{size}" / "apps" / f"{APP_ID}.png"
        dest.parent.mkdir(parents=True, exist_ok=True)
        dest.write_bytes(pngs[size])
    scalable = hicolor / "scalable" / "apps" / f"{APP_ID}.svg"
    scalable.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(SVG_PATH, scalable)

    (HERE / "ferrocord.ico").write_bytes(ico({s: rgba[s] for s in ICO_SIZES}))
    (HERE / "ferrocord.icns").write_bytes(icns(pngs))
    print(f"wrote icons to {HERE}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
