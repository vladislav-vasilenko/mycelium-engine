"""Текстуры Poly Haven (CC0) для рельефа и коры (02.11, сессия 4а).

Запуск: `.venv-geo/bin/python -I tools/terrain/fetch_polyhaven_textures.py`.
Скачивает карты и сохраняет их в раскладке ambientCG, которую читает
`terrain_materials::load` (2048², JPEG): `assets/source/ambientcg/PH_<имя>/`
`{Color,NormalGL,Roughness,Displacement,AmbientOcclusion}.jpg`; кора — 1024²
(`plants::load`). Нет отдельной карты — берётся из ARM (R — AO, G —
шероховатость) или плоская.
"""

import io
import json
import sys
import urllib.request
from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parents[2] / "assets/source/ambientcg"
UA = {"User-Agent": "mycelium-engine-asset-fetch"}

# Имя Poly Haven → (сторона, роль).
TEXTURES = {
    "rock_face_03": (2048, "скала"),
    "coast_sand_01": (2048, "песок"),
    "forrest_ground_01": (2048, "лесная почва"),
    "sparse_grass": (2048, "трава"),
    "forest_leaves_02": (2048, "подстилка"),
    "rocky_trail": (2048, "тропа из камней"),
    "mossy_rock": (2048, "мшистый камень"),
    "worn_asphalt": (2048, "асфальт дорог"),
    "japanese_cedar_bark": (1024, "кора криптомерии"),
}

# Карта движка → ключи Poly Haven по порядку предпочтения.
MAPS = {
    "Color": ["Diffuse", "diff"],
    "NormalGL": ["nor_gl"],
    "Roughness": ["Rough", "rough"],
    "Displacement": ["Displacement", "disp"],
    "AmbientOcclusion": ["AO", "ao"],
}


def get(url):
    req = urllib.request.Request(url, headers=UA)
    with urllib.request.urlopen(req, timeout=300) as r:
        return r.read()


def pick(files, keys, res):
    for k in keys:
        if k in files and res in files[k]:
            fmts = files[k][res]
            for f in ("jpg", "png"):
                if f in fmts:
                    return fmts[f]["url"]
    return None


def main():
    lic = ["# Poly Haven — CC0 1.0 (текстуры, 02.11)", "", "| Папка | Источник | Роль |", "| --- | --- | --- |"]
    for name, (size, role) in TEXTURES.items():
        out = ROOT / f"PH_{name}"
        out.mkdir(parents=True, exist_ok=True)
        files = json.loads(get(f"https://api.polyhaven.com/files/{name}"))
        res = "2k" if size == 2048 else "1k"
        arm = None
        for dst, keys in MAPS.items():
            path = out / f"{dst}.jpg"
            if path.exists():
                continue
            url = pick(files, keys, res)
            mode = "RGB" if dst in ("Color", "NormalGL") else "L"
            if url:
                img = Image.open(io.BytesIO(get(url)))
                if img.mode in ("I;16", "I"):
                    img = img.point(lambda v: v / 256).convert("L")
                img = img.convert(mode)
            else:
                if arm is None:
                    u = pick(files, ["arm"], res)
                    arm = Image.open(io.BytesIO(get(u))).convert("RGB") if u else None
                if arm is not None and dst in ("Roughness", "AmbientOcclusion"):
                    img = arm.split()[1 if dst == "Roughness" else 0]
                else:
                    img = Image.new(mode, (size, size), 128 if dst == "Displacement" else 255)
            img.resize((size, size), Image.LANCZOS).save(path, quality=92)
        lic.append(f"| PH_{name} | https://polyhaven.com/a/{name} | {role} |")
        print(f"PH_{name}: {role}")
    (ROOT / "LICENSE-polyhaven.md").write_text("\n".join(lic) + "\n")


if __name__ == "__main__":
    sys.exit(main())
