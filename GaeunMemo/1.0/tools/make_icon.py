"""Writes every icon Tauri needs into src-tauri/icons/.

A square sheet of white paper with a lavender band on top and the bottom-right
corner folded over. Standard library only:  python tools/make_icon.py
"""
import struct
import zlib
from pathlib import Path

PAPER = (0xFF, 0xFF, 0xFF)
EDGE = (0xA8, 0xA4, 0x9A)     # outline, dark enough to read on a white taskbar
BAND = (0xAF, 0xA9, 0xEC)     # lavender
FLAP = (0xDC, 0xD8, 0xCE)     # back of the paper

OUT = Path(__file__).resolve().parent.parent / "src-tauri" / "icons"
SS = 4  # supersampling per axis


def shade(size, x, y):
    """Colour (r, g, b, a) at a point in a size x size canvas."""
    m = size * 0.10                      # transparent margin
    lo, hi = m, size - m                 # sheet spans [lo, hi)
    bw = max(1.0, size / 40)             # outline width
    band = (hi - lo) * 0.22              # band height
    fold = (hi - lo) * 0.30              # fold leg length
    diag = 2 * hi - fold                 # the fold line: x + y = diag

    if not (lo <= x < hi and lo <= y < hi):
        return None
    s = x + y
    if s > diag:                         # the corner that was folded away
        return None
    # the flap lying on the sheet: triangle (hi-fold, hi-fold), (hi, hi-fold), (hi-fold, hi)
    if x >= hi - fold and y >= hi - fold:
        if abs(s - diag) < bw or x < hi - fold + bw or y < hi - fold + bw:
            return EDGE
        return FLAP
    if x < lo + bw or y < lo + bw or x >= hi - bw or y >= hi - bw or abs(s - diag) < bw * 0.75:
        return EDGE
    if y < lo + band:
        return BAND
    return PAPER


def render(size):
    rows = bytearray()
    n = SS * SS
    for py in range(size):
        rows.append(0)
        for px in range(size):
            r = g = b = a = 0
            for sy in range(SS):
                for sx in range(SS):
                    c = shade(size, px + (sx + 0.5) / SS, py + (sy + 0.5) / SS)
                    if c:
                        r += c[0]; g += c[1]; b += c[2]; a += 1
            if a:
                rows.extend((r // a, g // a, b // a, 255 * a // n))
            else:
                rows.extend((0, 0, 0, 0))
    return png_bytes(size, rows)


def png_bytes(size, rows):
    def chunk(tag, data):
        c = struct.pack(">I", len(data)) + tag + data
        return c + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

    ihdr = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr)
            + chunk(b"IDAT", zlib.compress(bytes(rows), 9)) + chunk(b"IEND", b""))


def ico(sizes):
    images = [render(s) for s in sizes]
    head = struct.pack("<HHH", 0, 1, len(images))
    offset = 6 + 16 * len(images)
    entries, blobs = b"", b""
    for s, img in zip(sizes, images):
        dim = 0 if s >= 256 else s
        entries += struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(img), offset)
        offset += len(img)
        blobs += img
    return head + entries + blobs


if __name__ == "__main__":
    OUT.mkdir(parents=True, exist_ok=True)
    (OUT / "32x32.png").write_bytes(render(32))
    (OUT / "128x128.png").write_bytes(render(128))
    (OUT / "icon.png").write_bytes(render(512))
    (OUT / "icon.ico").write_bytes(ico([16, 20, 24, 32, 40, 48, 64, 256]))
    src = OUT / "source.png"
    if src.exists():
        src.unlink()
    print("icons written to", OUT)
