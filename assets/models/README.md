# Модели

Файлы здесь собираются скриптами из `assets/source/` и не правятся вручную.

## `fps-arms.glb`

Руки от первого лица для [этапа 10](../../specs/10-first-person-hands.md). Источник и лицензия — [assets/source/fps-arms/LICENSE.md](../source/fps-arms/LICENSE.md).

- Единицы — метры, начало координат — глаз, взгляд −Z, вверх +Y.
- Сетка со скином: 4600 вершин после разделения по UV, 8152 треугольника, 1 материал `skin` с встроенной текстурой кожи 1024².
- Скелет: 50 костей рига и `neutral_bone`, который экспортёр добавляет для вершин без весов.
- Позы — анимации из одного ключа: `relaxed`, `grip`, `point`, `reach`. Параметры поз — [tools/blender/arms_poses.json](../../tools/blender/arms_poses.json).
- Покой (bind pose) — исходная поза файла с опущенными руками; в кадре руки всегда в одной из поз.
- `fps-arms.poses.json` — эталон суставов кисти по позам для проверки.

Пересборка и проверка (Blender 5.2):

```sh
/Applications/Blender.app/Contents/MacOS/Blender -b --disable-autoexec \
    assets/source/fps-arms/fps-arms-rig.blend --python tools/blender/export_arms.py -- \
    tools/blender/arms_poses.json assets/models/fps-arms.glb
/Applications/Blender.app/Contents/MacOS/Blender -b --factory-startup \
    --python tools/blender/check_arms.py -- assets/models/fps-arms.glb logs/screens/hands
```
