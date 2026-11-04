"""Compose 1920x1080 Store screenshots from the README images in docs/ (the taskbar strip, the detail
panel and the menu) over a plain desktop background. Pure Python: no image library required.

Usage: python store/make_screenshots.py OUT_DIR
"""

import struct
import sys
import zlib
from pathlib import Path

DOCS = Path(__file__).resolve().parent.parent / "docs"
W, H, TASKBAR_H = 1920, 1080, 48


def read_png(path):
    """8-bit RGB / RGBA PNG -> (w, h, rows of (r, g, b) tuples)."""
    data = Path(path).read_bytes()
    pos, idat = 8, b""
    while pos < len(data):
        n, kind = struct.unpack(">I4s", data[pos:pos + 8])
        body = data[pos + 8:pos + 8 + n]
        if kind == b"IHDR":
            w, h, depth, colour = struct.unpack(">IIBB", body[:10])
            assert depth == 8 and colour in (2, 6), (path, depth, colour)
            bpp = 3 if colour == 2 else 4
        elif kind == b"IDAT":
            idat += body
        pos += n + 12
    raw, stride = zlib.decompress(idat), w * bpp
    rows, prev = [], bytearray(stride)
    for y in range(h):
        f, line = raw[y * (stride + 1)], bytearray(raw[y * (stride + 1) + 1:(y + 1) * (stride + 1)])
        for i in range(stride):
            a = line[i - bpp] if i >= bpp else 0
            b, c = prev[i], prev[i - bpp] if i >= bpp else 0
            if f == 1:
                line[i] = (line[i] + a) & 255
            elif f == 2:
                line[i] = (line[i] + b) & 255
            elif f == 3:
                line[i] = (line[i] + (a + b) // 2) & 255
            elif f == 4:
                p = a + b - c
                pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                line[i] = (line[i] + (a if pa <= pb and pa <= pc else b if pb <= pc else c)) & 255
        rows.append([tuple(line[x * bpp:x * bpp + 3]) for x in range(w)])
        prev = line
    return w, h, rows


def write_png(path, w, h, pixels):
    raw = b"".join(b"\x00" + bytes(v for px in row for v in px) for row in pixels)

    def chunk(t, d):
        return struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)

    Path(path).write_bytes(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
                           + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))


def desktop():
    """Plain gradient wallpaper with an empty taskbar coloured like the one in taskbar.png."""
    _, _, strip = read_png(DOCS / "taskbar.png")
    bar = strip[TASKBAR_H // 2][2]
    top, bottom = (0x33, 0x40, 0x52), (0x14, 0x18, 0x20)
    px = []
    for y in range(H - TASKBAR_H):
        t = y / (H - TASKBAR_H)
        c = tuple(round(top[i] + (bottom[i] - top[i]) * t) for i in range(3))
        px.append([c] * W)
    px += [[bar] * W for _ in range(TASKBAR_H)]
    return px


def paste(px, img, x0, y0):
    _, _, rows = img
    for y, row in enumerate(rows):
        px[y0 + y][x0:x0 + len(row)] = row


def main():
    out = Path(sys.argv[1])
    out.mkdir(parents=True, exist_ok=True)
    strip, popup, menu = (read_png(DOCS / n) for n in ("taskbar.png", "popup.png", "menu.png"))
    sx = W - strip[0]
    bar_right = sx + 12 + 271  # the bar starts 12 px into taskbar.png and is 271 px wide

    shots = {
        "1-taskbar.png": [],
        "2-details.png": [(popup, bar_right - popup[0], H - TASKBAR_H - 8 - popup[1])],
        "3-menu.png": [(menu, bar_right - menu[0], H - TASKBAR_H - 4 - menu[1])],
    }
    for name, extra in shots.items():
        px = desktop()
        paste(px, strip, sx, H - TASKBAR_H)
        for img, x, y in extra:
            paste(px, img, x, y)
        write_png(out / name, W, H, px)
        print(name)


if __name__ == "__main__":
    main()
