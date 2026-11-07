"""Проверка экспортированных рук: импорт .glb в пустую сцену и сверка поз.

    Blender -b --factory-startup --python tools/blender/check_arms.py -- \
        assets/models/fps-arms.glb [папка_превью]

Проверяет: одна сетка со скином, 50 костей (плюс `neutral_bone`, который
экспортёр добавляет для вершин без весов), 4 позы-анимации, размеры в метрах,
суставы кисти в каждой позе совпадают с эталоном экспорта
(`fps-arms.poses.json`) с точностью 1 мм. Печатает `CHECK PASSED` или
`CHECK FAILED` и выходит с кодом 0 или 1.
"""

import json
import math
import sys
from pathlib import Path

import bpy
from mathutils import Vector

TOLERANCE_M = 1e-3

a = sys.argv[sys.argv.index("--") + 1 :]
glb = Path(a[0])
preview = Path(a[1]) if len(a) > 1 else None
reference = json.loads(glb.with_suffix(".poses.json").read_text())

for o in list(bpy.data.objects):
    bpy.data.objects.remove(o)
bpy.ops.import_scene.gltf(filepath=str(glb))

failures = []


def check(ok, what):
    print(("[pass] " if ok else "[FAIL] ") + what)
    if not ok:
        failures.append(what)


# Импортёр создаёт и служебную сетку формы костей — берём сетку со скином.
meshes = [o for o in bpy.data.objects if o.type == "MESH" and any(m.type == "ARMATURE" for m in o.modifiers)]
arms = [o for o in bpy.data.objects if o.type == "ARMATURE"]
check(len(meshes) == 1 and len(arms) == 1, f"одна сетка и один скелет: {len(meshes)}, {len(arms)}")
arm, body = arms[0], meshes[0]
tris = sum(len(p.vertices) - 2 for p in body.data.polygons)
bones = {b.name for b in arm.data.bones} - {"neutral_bone"}
check(len(bones) == 50, f"костей {len(bones)} (+ neutral_bone)")
check(any(m.type == "ARMATURE" for m in body.modifiers), "сетка привязана к скелету")
names = sorted(a.name for a in bpy.data.actions)
check(set(reference) <= {n.split("_")[0] for n in names} | set(names), f"позы {names}")
print(f"треугольников {tris}, вершин {len(body.data.vertices)}")

worst = 0.0
for pose, joints in reference.items():
    action = next(a for a in bpy.data.actions if a.name == pose or a.name.startswith(pose))
    arm.animation_data.action = action
    if action.slots:
        arm.animation_data.action_slot = action.slots[0]
    bpy.context.scene.frame_set(1)
    bpy.context.view_layer.update()
    err = max((arm.matrix_world @ arm.pose.bones[b].head - Vector(p)).length for b, p in joints.items())
    worst = max(worst, err)
    check(err < TOLERANCE_M, f"поза {pose}: суставы кисти, макс. отклонение {err * 1000:.3f} мм")
    hand = (arm.matrix_world @ arm.pose.bones["hand.R"].head) - (arm.matrix_world @ arm.pose.bones["f_middle.03.R"].tail)
    if pose == "relaxed":
        check(0.12 < hand.length < 0.25, f"длина кисти {hand.length * 100:.1f} см")
    if preview:
        sc = bpy.context.scene
        cd = bpy.data.cameras.new("eye")
        cd.lens_unit, cd.sensor_fit, cd.angle, cd.clip_start = "FOV", "VERTICAL", math.radians(60), 0.01
        cam = bpy.data.objects.new("eye", cd)
        sc.collection.objects.link(cam)
        sc.camera = cam
        cam.rotation_euler = (math.radians(85), 0, 0)
        sc.render.engine = "BLENDER_WORKBENCH"
        sc.display.shading.light, sc.display.shading.color_type = "STUDIO", "TEXTURE"
        sc.render.resolution_x, sc.render.resolution_y = 960, 540
        preview.mkdir(parents=True, exist_ok=True)
        sc.render.filepath = str(preview / f"{pose}.png")
        bpy.ops.render.render(write_still=True)
        bpy.data.objects.remove(cam)

print("CHECK PASSED" if not failures else f"CHECK FAILED: {len(failures)}")
sys.exit(1 if failures else 0)
