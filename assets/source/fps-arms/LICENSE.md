# Источник: руки от первого лица

- Название — «fps arms (rigged only)»
- Автор — para, опубликовано 2015-03-18
- Страница — https://opengameart.org/content/fps-arms-rigged-only
- Лицензия — [CC0 1.0](https://creativecommons.org/publicdomain/zero/1.0/) (общественное достояние)
- Основа сетки и текстуры — MakeHuman; модели, экспортированные из MakeHuman, распространяются по CC0
- Архив — `fps arms.7z`, SHA-256 `31f6c7bd5caea8856c4aafca8461f38a3c8bfdd3d8f05c898e403b9475e54562`, скачан 2026-11-08

Файлы здесь:

| Файл | Исходное имя | SHA-256 |
| --- | --- | --- |
| `fps-arms-rig.blend` | `FPS ARMS RIG 1.blend` | `5c41f224f40525fa2f09c1168b3df638fc0b401ba5e59ae7fad6f88369fd6f82` |
| `new_diff.png` | `new_diff.png` | `f7a67f1c8f72294dc54ddea96253522dabdee99838746d6fb695eeef46120b63` |

Исходник не меняется. Всё, что нужно движку, делает `tools/blender/export_arms.py`: результат — `assets/models/fps-arms.glb`.

Описание автора: сетка около 4000 вершин и 8000 треугольников, текстура 1024 × 1024. Риг с IK и ручкой кисти (`hand.*.control`), фаланги пальцев сгибаются ограничением Copy Rotation от первой фаланги.
