#!/usr/bin/env python3
"""Сырые RGB (квадрат) → PNG без сторонних библиотек.
Использование: scripts/rgb-to-png.py [вход.rgb] [выход.png]"""
import math, struct, sys, zlib

src = sys.argv[1] if len(sys.argv) > 1 else "target/world-map.rgb"
dst = sys.argv[2] if len(sys.argv) > 2 else "target/world-map.png"
data = open(src, "rb").read()
n = int(math.isqrt(len(data) // 3))
rows = b"".join(b"\x00" + data[y * n * 3:(y + 1) * n * 3] for y in range(n))

def chunk(tag, body):
    return struct.pack(">I", len(body)) + tag + body + struct.pack(">I", zlib.crc32(tag + body))

png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", n, n, 8, 2, 0, 0, 0)) \
    + chunk(b"IDAT", zlib.compress(rows, 9)) + chunk(b"IEND", b"")
open(dst, "wb").write(png)
print(f"{dst}: {n}×{n}")
