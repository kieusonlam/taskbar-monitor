"""Draw the app logo (a dark rounded tile with three meter bars in the bar's colours) at every size the
MSIX package needs. Pure Python: no image library required.

Usage: python store/make_assets.py OUT_DIR
"""

import struct
import sys
import zlib
from pathlib import Path

TILE = (0x1C, 0x1C, 0x1C)
BORDER = (0x3A, 0x3A, 0x3A)
# Same colours as the bar: update-dot blue, value white, warning yellow
BARS = [((0x4F, 0xA3, 0xFF), 0.50), ((0xF2, 0xF2, 0xF2), 0.78), ((0xFF, 0xD1, 0x66), 0.62)]
SS = 4  # supersampling factor for anti-aliasing


def rounded(x, y, x0, y0, x1, y1, r):
    """Whether (x, y) is inside the rounded rectangle."""
    if not (x0 <= x < x1 and y0 <= y < y1):
        return False
    cx = min(max(x, x0 + r), x1 - r)
    cy = min(max(y, y0 + r), y1 - r)
    return (x - cx) ** 2 + (y - cy) ** 2 <= r * r


def mark_pixel(u, v):
    """Colour (r, g, b, a) of the logo at (u, v) in the unit square, or None if transparent."""
    if not rounded(u, v, 0, 0, 1, 1, 0.22):
        return None
    if not rounded(u, v, 0.035, 0.035, 0.965, 0.965, 0.19):
        return BORDER
    bar_w, gap, bottom, top = 0.16, 0.085, 0.78, 0.22
    left = (1 - (3 * bar_w + 2 * gap)) / 2
    for i, (colour, level) in enumerate(BARS):
        x0 = left + i * (bar_w + gap)
        y0 = bottom - (bottom - top) * level
        if rounded(u, v, x0, y0, x0 + bar_w, bottom, 0.035):
            return colour
    return TILE


def render(w, h, mark):
    """RGBA image w x h with the logo centred, `mark` pixels wide."""
    ox, oy = (w - mark) / 2, (h - mark) / 2
    out = bytearray()
    for y in range(h):
        out.append(0)  # PNG filter: none
        for x in range(w):
            r = g = b = a = 0
            for sy in range(SS):
                for sx in range(SS):
                    u = (x + (sx + 0.5) / SS - ox) / mark
                    v = (y + (sy + 0.5) / SS - oy) / mark
                    c = mark_pixel(u, v)
                    if c:
                        r, g, b, a = r + c[0], g + c[1], b + c[2], a + 1
            n = SS * SS
            out += bytes((r // a, g // a, b // a, 255 * a // n)) if a else bytes(4)
    return bytes(out)


def png(path, w, h, raw):
    def chunk(t, d):
        return struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)

    Path(path).write_bytes(
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )


def main():
    out = Path(sys.argv[1])
    out.mkdir(parents=True, exist_ok=True)
    jobs = []
    # App list / taskbar icon: plated scales, plus unplated target sizes (full-bleed)
    for scale in (100, 200):
        s = 44 * scale // 100
        jobs.append((f"Square44x44Logo.scale-{scale}.png", s, s, round(s * 0.84)))
    for t in (16, 24, 32, 48, 256):
        jobs.append((f"Square44x44Logo.targetsize-{t}.png", t, t, t))
        jobs.append((f"Square44x44Logo.targetsize-{t}_altform-unplated.png", t, t, t))
    for scale in (100, 200):
        k = scale / 100
        jobs.append((f"Square150x150Logo.scale-{scale}.png", round(150 * k), round(150 * k), round(88 * k)))
        jobs.append((f"Wide310x150Logo.scale-{scale}.png", round(310 * k), round(150 * k), round(88 * k)))
        jobs.append((f"StoreLogo.scale-{scale}.png", round(50 * k), round(50 * k), round(50 * k)))
    for name, w, h, mark in jobs:
        png(out / name, w, h, render(w, h, mark))
        print(name)


if __name__ == "__main__":
    main()
