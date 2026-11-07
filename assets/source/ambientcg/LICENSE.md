# Источник: материалы ландшафта ambientCG

- Сайт — https://ambientcg.com
- Лицензия — [CC0 1.0](https://docs.ambientcg.com/license/) (общественное достояние), указание автора не требуется
- Скачано — 2026-11-08, наборы `*_1K-JPG.zip` (1024 × 1024)
- Из каждого набора взяты карты `Color`, `NormalGL` (нормали в соглашении OpenGL, +Y вверх), `Roughness`, `Displacement`; файлы переименованы без изменения содержимого

| Слой движка | Материал | Страница | SHA-256 архива |
| --- | --- | --- | --- |
| 0 камень | `Rock062` | https://ambientcg.com/view?id=Rock062 | `7a98f755a040bfbdae634b2b610f0d91cbb243e8e961e6eb3340b6b96ee1887c` |
| 1 песок | `Ground093A` | https://ambientcg.com/view?id=Ground093A | `c179447baf185c4ae749f1879c05f7991c8892aeb3d4d67c8e41e447d7267bd2` |
| 2 почва | `Ground082S` | https://ambientcg.com/view?id=Ground082S | `d5d1b714dd6e6a118a514c5f8f62da67d525298dea166f908e8086c7274d6f8e` |
| 3 мох | `Moss002` | https://ambientcg.com/view?id=Moss002 | `727d4e3134f604837c2e11aa3bf12417a022d3c668a6d7339efa572fdbb056bb` |
| 4 лесная подстилка | `Ground037` | https://ambientcg.com/view?id=Ground037 | `4e69db2b7251b1736fa70fe6fe4ab80d64bd220117dd5dd4c7caaead51c2574e` |

Движок читает JPEG при запуске (`src/terrain_materials.rs`) и собирает два массива текстур: альбедо с высотой и нормали с шероховатостью.
