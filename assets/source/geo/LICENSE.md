# Источники реального рельефа (02.7)

Исходные файлы лежат в `assets/source/geo/cache/` и `assets/source/geo/gsi/` вне git: их адреса и SHA-256 — в [sources.json](sources.json), скачивание и проверка — `tools/geo/yakushima.py fetch`. В репозитории хранятся только производные слои `assets/world/yakushima/`.

| Источник | Что взято | Лицензия и авторство |
| --- | --- | --- |
| Copernicus DEM GLO-30, тайл N30 E130 | Высоты поверхности (вместе с кронами леса), шаг 1″ ≈ 30 м | © DLR e.V. 2010–2014 and © Airbus Defence and Space GmbH 2014–2018 provided under COPERNICUS by the European Union and ESA; all rights reserved. Бесплатное использование, в том числе коммерческое, с указанием авторства |
| OpenStreetMap, снимок Overpass 2026-11-14 | Реки и ручьи, тропы и дороги, водопады, вершины, приюты, именные криптомерии, болото Ханоэго | © участники OpenStreetMap, ODbL 1.0. `assets/world/yakushima/far/features.json` — производная база данных и распространяется под ODbL; рисунок мира и слои PNG — «произведённая работа» с указанием авторства |
| ESA WorldCover 2021 v200, тайл N30E129 | Покров суши 10 м → пояса растительности (`cover.png`) | © ESA WorldCover project 2021 / Contains modified Copernicus Sentinel data (2021) processed by ESA WorldCover consortium. CC BY 4.0 |
| GSI 基盤地図情報 数値標高モデル (DEM1A, DEM5A, DEM10B), квадраты 4530xx | Рельеф грунта: DEM5A (≈ 5 м) — дальний рельеф всего острова; DEM1A (≈ 1 м) — для игровой зоны; DEM10B — запас. Архив скачан пользователем 2026-11-14, SHA-256 в `sources.json` | 出典：国土地理院 基盤地図情報（数値標高モデル）を加工して作成. Правила правительства Японии 2.0, совместимы с CC BY 4.0 |

Авторство всех используемых источников выводится в титрах игры.
