"""Writes a 2-frame looping GIF (red <-> blue, 200 ms) with no imaging library.

LZW trick: min code size 7 (128-colour table) keeps every code 8 bits wide as long
as a CLEAR is emitted every <=120 literals, so the code stream is plain bytes.
    python -I tools/make_anim_gif.py out.gif [w] [h]
"""
import struct, sys

def frame_data(index, n):
    codes = []
    for i in range(0, n, 120):
        codes.append(128)                     # CLEAR
        codes.extend([index] * min(120, n - i))
    codes.append(129)                         # EOI
    raw = bytes(codes)
    blocks = b"".join(bytes([len(raw[i:i + 255])]) + raw[i:i + 255] for i in range(0, len(raw), 255))
    return b"\x07" + blocks + b"\x00"

def main():
    out = sys.argv[1]
    w = int(sys.argv[2]) if len(sys.argv) > 2 else 120
    h = int(sys.argv[3]) if len(sys.argv) > 3 else 80
    pal = bytearray(128 * 3)
    pal[3:6] = bytes((220, 60, 60))           # 1 = red
    pal[6:9] = bytes((60, 90, 220))           # 2 = blue
    g = bytearray(b"GIF89a" + struct.pack("<HHBBB", w, h, 0xF6, 0, 0) + pal)
    g += b"\x21\xFF\x0BNETSCAPE2.0\x03\x01\x00\x00\x00"
    for idx in (1, 2):
        g += b"\x21\xF9\x04\x04" + struct.pack("<H", 20) + b"\x00\x00"   # 20 cs, disposal 1
        g += b"\x2C" + struct.pack("<HHHHB", 0, 0, w, h, 0) + frame_data(idx, w * h)
    g += b"\x3B"
    open(out, "wb").write(g)
    print(out, len(g), "bytes")

main()
