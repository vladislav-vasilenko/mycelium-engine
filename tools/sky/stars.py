"""Каталог ярких звёзд Йеля (BSC5, 5-е изд.) → assets/sky/stars.bin для неба (04.1).

Источник: http://tdc-www.harvard.edu/catalogs/bsc5.dat.gz (Hoffleit, Warren 1991;
общественное достояние), архив в assets/source/sky/bsc5.dat.gz.

Формат выхода: little-endian f32 × 5 на звезду — направление в экваториальных
осях J2000 (x — точка весны, z — северный полюс мира), видимая величина V,
показатель цвета B−V (0,6 — нет данных). Звёзды отсортированы по яркости.
Запуск: python3 tools/sky/stars.py
"""

import gzip
import hashlib
import math
import struct
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SRC = ROOT / "assets/source/sky/bsc5.dat.gz"
OUT = ROOT / "assets/sky/stars.bin"
SHA256 = "0471ced07fc241e625613baa689b2fa5f4b59fd733f731003e57d2d436036cdb"


def field(line, a, b):
    """Колонки a…b (с 1, включительно) из описания каталога."""
    return line[a - 1 : b].strip()


def main():
    data = SRC.read_bytes()
    assert hashlib.sha256(data).hexdigest() == SHA256, "bsc5.dat.gz: другой SHA-256"
    stars = []
    for line in gzip.decompress(data).decode("latin-1").splitlines():
        ra_h, vmag = field(line, 76, 77), field(line, 103, 107)
        if not ra_h or not vmag:
            continue  # новые и снятые объекты без координат
        ra = (int(ra_h) + int(field(line, 78, 79)) / 60 + float(field(line, 80, 83)) / 3600) * 15
        dec = int(field(line, 85, 86)) + int(field(line, 87, 88)) / 60 + int(field(line, 89, 90)) / 3600
        if field(line, 84, 84) == "-":
            dec = -dec
        bv_s = field(line, 110, 114)
        bv = float(bv_s) if bv_s else 0.6
        r, d = math.radians(ra), math.radians(dec)
        stars.append((math.cos(d) * math.cos(r), math.cos(d) * math.sin(r), math.sin(d), float(vmag), bv))
    stars.sort(key=lambda s: s[3])
    OUT.write_bytes(b"".join(struct.pack("<5f", *s) for s in stars))
    print(f"звёзд {len(stars)}, ярче 6,5m: {sum(s[3] < 6.5 for s in stars)}; {OUT}")


if __name__ == "__main__":
    main()
