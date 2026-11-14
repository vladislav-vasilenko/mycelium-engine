# Источник: материалы ландшафта ambientCG

- Сайт — https://ambientcg.com
- Лицензия — [CC0 1.0](https://docs.ambientcg.com/license/) (общественное достояние), указание автора не требуется
- Скачано — 2026-11-08, наборы `*_1K-JPG.zip` (1024 × 1024); 2026-11-15 заменены на `*_2K-JPG.zip` (2048 × 2048)
- Из каждого набора взяты карты `Color`, `NormalGL` (нормали в соглашении OpenGL, +Y вверх), `Roughness`, `Displacement`, `AmbientOcclusion` (с 2K); файлы переименованы без изменения содержимого

| Слой движка | Материал | Страница | SHA-256 архива |
| --- | --- | --- | --- |
| 0 камень | `Rock062` | https://ambientcg.com/view?id=Rock062 | `7f6290a24eb86b681c755046b9c5641775525c624b75e4df1eca172aff982c64` (2K) |
| 1 песок | `Ground093A` | https://ambientcg.com/view?id=Ground093A | `3277688bb28ba981c7af76a36c88ae4ef26419e3bd2328ff3aeab5cdac0dda1b` (2K) |
| 2 почва | `Ground082S` | https://ambientcg.com/view?id=Ground082S | `4a85e0814bf68e54f78b14cca27d48f40592a693662bd2e320564b480cb5e7c9` (2K) |
| 3 мох | `Moss002` | https://ambientcg.com/view?id=Moss002 | `03fe8868a5b5618b30037fcece05fff9ea736a61227d9566291a9d4b3a2bb047` (2K) |
| 4 лесная подстилка | `Ground037` | https://ambientcg.com/view?id=Ground037 | `defa9f880ec692eb3ef5435fd7bc7b8721c0d577f7824ea18ac68537f01ab65b` (2K) |
| 5 трава (якудзаса, луга) | `Grass004` | https://ambientcg.com/view?id=Grass004 | `d7d3f08e8bb1e59aed0c3dbb85ea0b46d5308481f89b6a3fd62c0ec9e862245b` (2K) (скачано 2026-11-14) |

Движок читает JPEG при запуске (`src/terrain_materials.rs`) и собирает два массива текстур: альбедо с высотой и нормали с шероховатостью.

Кора растений (02.8), скачано 2026-11-15, наборы `*_1K-JPG.zip`, взяты `Color` и `NormalGL`:

| Назначение | Материал | Страница | SHA-256 архива |
| --- | --- | --- | --- |
| кора криптомерии якусуги (подкрашивается в шейдере) | `Bark012` | https://ambientcg.com/view?id=Bark012 | `6e26e4c24fe53cf564909b371afbfc84041d9077f39bf8eb4ae16951df1c4763` |
| кора широколиственных деревьев | `Bark007` | https://ambientcg.com/view?id=Bark007 | `a376a5208e7e9cb20475805042aee6c7d9fd1d5bcd8f80f38f75bea6e063c6f4` |

Переходные слои рельефа (02.2, итерация 3), скачано 2026-11-15, наборы `*_2K-JPG.zip`, карты как у основных слоёв:

| Слой движка | Материал | Страница | SHA-256 архива |
| --- | --- | --- | --- |
| 6 тёмный опад | `Ground023` | https://ambientcg.com/view?id=Ground023 | `aaea0162927a2442098e823304bc268fa53b3a26ddb3630a142573e78948bd4c` |
| 7 листовой опад (2048 × 1024, движок складывает вдвое) | `Ground078` | https://ambientcg.com/view?id=Ground078 | `f0b7f313a5d504c085e95c5d655914b6ce430de146ab056dd7caeeaaef44abe2` |
| 8 щебень | `Ground081` | https://ambientcg.com/view?id=Ground081 | `c61058354610a23dbb1b0a2a456001fab1ccb0ad592fdcc9a566f88b850ec7b1` |
| 9 мшистый камень | `Rock064` | https://ambientcg.com/view?id=Rock064 | `225f88f2e8bfdac834027c497115f1f7bb82653a53e904987fa6b071fda94d9c` |
