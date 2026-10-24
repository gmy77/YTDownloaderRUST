#!/usr/bin/env python3
"""Genera assets/icon.ico (16–256 px): riquadro rosso arrotondato con il simbolo
"play" bianco e una freccia di download ambra. Solo libreria standard."""
import struct, zlib
from pathlib import Path

RED = (204, 32, 32)
WHITE = (255, 255, 255)
AMBER = (249, 168, 37)


def inside_rounded(u, v, m, r):
    cx = min(max(u, m + r), 1 - m - r)
    cy = min(max(v, m + r), 1 - m - r)
    return (u - cx) ** 2 + (v - cy) ** 2 <= r * r


def pixel(x, y, n):
    u, v = (x + 0.5) / n, (y + 0.5) / n
    # Freccia di download in basso a destra, sopra al resto.
    if 0.62 <= u <= 0.94 and 0.58 <= v <= 0.96:
        cu = 0.78
        if abs(u - cu) <= 0.045 and v <= 0.80:
            return AMBER + (255,)
        if v >= 0.74 and v <= 0.90 and abs(u - cu) <= (0.90 - v) * 0.95:
            return AMBER + (255,)
        if v >= 0.92 and 0.66 <= u <= 0.90:
            return AMBER + (255,)
    if not inside_rounded(u, v, 0.05, 0.2):
        return (0, 0, 0, 0)
    # Triangolo "play": vertici (0.36,0.28) (0.36,0.72) (0.70,0.50)
    if u >= 0.36 and abs(v - 0.5) <= (0.70 - u) * (0.22 / 0.34):
        return WHITE + (255,)
    return RED + (255,)


def png(n):
    rows = b''.join(b'\x00' + b''.join(bytes(pixel(x, y, n)) for x in range(n)) for y in range(n))
    def chunk(tag, data):
        return struct.pack('>I', len(data)) + tag + data + struct.pack('>I', zlib.crc32(tag + data) & 0xFFFFFFFF)
    return b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', n, n, 8, 6, 0, 0, 0)) + chunk(b'IDAT', zlib.compress(rows, 9)) + chunk(b'IEND', b'')


sizes = [16, 24, 32, 48, 64, 256]
images = [png(n) for n in sizes]
offset = 6 + 16 * len(sizes)
entries = b''
for n, img in zip(sizes, images):
    entries += struct.pack('<BBBBHHII', n % 256, n % 256, 0, 0, 1, 32, len(img), offset)
    offset += len(img)
out = Path(__file__).with_name('icon.ico')
out.write_bytes(struct.pack('<HHH', 0, 1, len(sizes)) + entries + b''.join(images))
Path(__file__).with_name('icon256.png').write_bytes(images[-1])
print(f'scritto {out} ({out.stat().st_size} byte)')
