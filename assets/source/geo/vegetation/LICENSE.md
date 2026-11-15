# Источник: карта растительности 1 : 25 000 (Якусима)

- Данные — 1/25,000植生図（環境省生物多様性センター, 自然環境保全基礎調査）, сборка на всю Японию Pacific Spatial Solutions inc. (GeoParquet): https://source.coop/pacificspatial/vegetation-jp
- Лицензия — CC BY 4.0 (https://creativecommons.org/licenses/by/4.0/), условия центра: https://www.biodic.go.jp/copyright/index.html
- Указание источника: 「1/25,000植生図（環境省生物多様性センター）を Pacific Spatial Solutions 株式会社が加工したデータを使用」 / "1/25,000 scale vegetation maps (Biodiversity Center, Ministry of the Environment), processed by Pacific Spatial Solutions inc."
- Выборка `yakushima.parquet` — квадраты второго уровня сетки 453012–453066 (Якусима), скачано 2026-11-15 чтением по HTTP через DuckDB, без изменений строк.
- Использование — `tools/geo/vegetation.py`: пояса `cover.png`, `vegetation.png` и `vegetation.json`.
