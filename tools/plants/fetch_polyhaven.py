"""Сканы растений и камней Poly Haven (CC0) для леса Якусимы (02.11, сессия 1).

Запуск: `python3 tools/plants/fetch_polyhaven.py [--heavy]`.
Скачивает glTF с текстурами 1K в `assets/source/polyhaven/<имя>/` и пишет
`assets/source/polyhaven/manifest.json` (имя, роль, треугольники, байты,
источник, лицензия). Повторный запуск пропускает скачанное. `--heavy` —
ещё и тяжёлые сканы деревьев (сотни МБ, в git не идут: см. `.gitignore`).

Лицензия Poly Haven — CC0 1.0: без ограничений, указание автора не обязательно
(указываем всё равно, в `LICENSE.md`).
"""

import json
import sys
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2] / "assets/source/polyhaven"
API = "https://api.polyhaven.com"
UA = {"User-Agent": "mycelium-engine-asset-fetch"}

# Имя → роль в лесу Якусимы. Тяжёлые (деревья-сканы) — только с --heavy.
ASSETS = {
    "fern_02": "папоротник подлеска",
    "moss_01": "подушка мха",
    "rock_moss_set_01": "мшистые камни",
    "rock_moss_set_02": "мшистые камни",
    "boulder_01": "валун",
    "stone_01": "камень у тропы",
    "rock_07": "камень",
    "root_cluster_01": "корни на поверхности",
    "root_cluster_02": "корни на поверхности",
    "single_root": "корень поперёк тропы",
    "pine_roots": "корни хвойного у основания",
    "tree_stump_01": "пень",
    "tree_stump_02": "пень",
    "dead_tree_trunk": "упавший ствол",
    "dead_tree_trunk_02": "сухое дерево",
    "dry_branches_medium_01": "сухие ветки (сор)",
    "bark_debris_01": "кора и сор",
    "shrub_02": "кустарник",
    "shrub_03": "кустарник",
    "shrub_04": "кустарник",
    "fir_sapling": "подрост хвойного",
    "grass_medium_02": "низкая трава",
    "grass_bermuda_01": "низкая трава (луг)",
}
HEAVY = {
    "island_tree_02": "широколиственное дерево",
    "tree_small_02": "молодое широколиственное",
    "fir_tree_01": "хвойное (замена криптомерии до своего скана)",
}


def get_json(url):
    with urllib.request.urlopen(urllib.request.Request(url, headers=UA), timeout=60) as r:
        return json.load(r)


def download(url, path):
    path.parent.mkdir(parents=True, exist_ok=True)
    if path.exists():
        return path.stat().st_size
    tmp = path.with_suffix(path.suffix + ".part")
    with urllib.request.urlopen(urllib.request.Request(url, headers=UA), timeout=300) as r, open(tmp, "wb") as f:
        while chunk := r.read(1 << 20):
            f.write(chunk)
    tmp.rename(path)
    return path.stat().st_size


def main():
    heavy = "--heavy" in sys.argv
    info = get_json(f"{API}/assets?type=models")
    wanted = dict(ASSETS)
    if heavy:
        wanted.update(HEAVY)
    manifest_path = ROOT / "manifest.json"
    manifest = json.loads(manifest_path.read_text()) if manifest_path.exists() else {}
    for name, role in wanted.items():
        files = get_json(f"{API}/files/{name}")
        gl = files["gltf"]["1k"]["gltf"]
        total = download(gl["url"], ROOT / name / Path(gl["url"]).name)
        for rel, inc in gl.get("include", {}).items():
            total += download(inc["url"], ROOT / name / rel)
        # Альфа-маски листвы — отдельными PNG (в glTF 1K цвет — JPEG без альфы).
        for key, val in files.items():
            if "lpha" in key and "1k" in val and "png" in val["1k"]:
                url = val["1k"]["png"]["url"]
                total += download(url, ROOT / name / "textures" / Path(url).name)
        meta = info.get(name, {})
        manifest[name] = {
            "role": role,
            "triangles": meta.get("polycount"),
            "bytes": total,
            "heavy": name in HEAVY,
            "source": f"https://polyhaven.com/a/{name}",
            "authors": list(meta.get("authors", {}).keys()),
            "license": "CC0 1.0",
        }
        print(f"{name}: {total / 1e6:.1f} МБ, {meta.get('polycount')} треугольников — {role}")
    manifest_path.write_text(json.dumps(manifest, ensure_ascii=False, indent=2) + "\n")
    lic = ["# Poly Haven — CC0 1.0", "",
           "Сканы с https://polyhaven.com, лицензия CC0 1.0 (общественное достояние,",
           "https://creativecommons.org/publicdomain/zero/1.0/). Указание авторов не",
           "обязательно; приводим из уважения.", "", "| Ассет | Роль | Авторы | Источник |", "| --- | --- | --- | --- |"]
    for name, m in sorted(manifest.items()):
        lic.append(f"| {name} | {m['role']} | {', '.join(m['authors'])} | {m['source']} |")
    (ROOT / "LICENSE.md").write_text("\n".join(lic) + "\n")


if __name__ == "__main__":
    main()
