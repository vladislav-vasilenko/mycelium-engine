"""Экспорт рук от первого лица из исходного .blend в glTF (.glb).

Запуск (без интерфейса, автозапуск скриптов файла выключен):

    Blender -b --disable-autoexec assets/source/fps-arms/fps-arms-rig.blend \
        --python tools/blender/export_arms.py -- \
        tools/blender/arms_poses.json assets/models/fps-arms.glb [папка_превью]

Что делает:
1. Убирает анимационные данные файла (клип T-Pose на дорожке NLA сбрасывает позу),
   камеру и лампы.
2. Переводит модель в метры (× 0,1) и ставит начало координат в точку глаза.
   Оси Blender: x вправо, y вперёд, z вверх. Экспорт glTF переводит их в
   +Y вверх, взгляд в −Z, как камера движка.
3. Для каждой позы из JSON ставит кисти через IK-кости `hand.*.control`, сгибает
   пальцы поворотом первой фаланги (остальные копируют её ограничением рига)
   и запекает видимые трансформации всех костей в отдельный action.
4. Снимает ограничения (IK и Copy Rotation в glTF не переносятся) и экспортирует
   сетку, скин и все позы как анимации из одного ключа.
5. Если указана папка превью — рендерит каждую позу с камеры глаза (FOV 60°).
"""

import json
import math
import sys
from pathlib import Path

import bpy
from mathutils import Matrix, Quaternion, Vector

ARMATURE = "caucasian_male_1"
FINGERS = ["index", "middle", "ring", "pinky"]
PREVIEW_FOV_DEG = 60.0


def args():
    a = sys.argv[sys.argv.index("--") + 1 :]
    return Path(a[0]), Path(a[1]), (Path(a[2]) if len(a) > 2 else None)


def prepare(cfg):
    for o in list(bpy.data.objects):
        if o.type in {"CAMERA", "LIGHT"}:
            bpy.data.objects.remove(o)
    arm = bpy.data.objects[ARMATURE]
    arm.animation_data_clear()
    for a in list(bpy.data.actions):
        bpy.data.actions.remove(a)

    # Метры и начало координат в глазу. Трансформация впекается прямо в кости и
    # вершины (Armature.transform, Mesh.transform): операторы transform_apply без
    # интерфейса недоступны. Объекты получают единичные матрицы.
    eye = Vector(cfg["eye_source_dm"])
    to_eye_m = Matrix.Scale(cfg["source_scale_m"], 4) @ Matrix.Translation(-eye)
    body = bpy.data.objects[f"{ARMATURE}:Body"]
    body_world = body.matrix_world.copy()
    arm.data.transform(to_eye_m @ arm.matrix_world)
    arm.matrix_world = Matrix.Identity(4)
    body.data.transform(to_eye_m @ body_world)
    body.matrix_parent_inverse = Matrix.Identity(4)
    body.matrix_basis = Matrix.Identity(4)
    bpy.context.view_layer.update()
    return arm


def skin_material():
    """Старый файл хранит текстуру вне узлов; собираем Principled BSDF с
    `new_diff.png`, чтобы glTF встроил базовый цвет кожи."""
    body = bpy.data.objects[f"{ARMATURE}:Body"]
    mat = body.material_slots[0].material
    mat.use_nodes = True
    nt = mat.node_tree
    nt.nodes.clear()
    out = nt.nodes.new("ShaderNodeOutputMaterial")
    bsdf = nt.nodes.new("ShaderNodeBsdfPrincipled")
    tex = nt.nodes.new("ShaderNodeTexImage")
    tex.image = bpy.data.images.load(bpy.path.abspath("//new_diff.png"), check_existing=True)
    bsdf.inputs["Roughness"].default_value = 0.6
    nt.links.new(tex.outputs["Color"], bsdf.inputs["Base Color"])
    nt.links.new(bsdf.outputs["BSDF"], out.inputs["Surface"])
    mat.name = "skin"


def set_pose(arm, pose):
    for pb in arm.pose.bones:
        pb.matrix_basis = Matrix.Identity(4)
    mw = arm.matrix_world
    for side, h in pose.items():
        ctrl = arm.pose.bones[f"hand.{side}.control"]
        hand = arm.pose.bones[f"hand.{side}"]
        fwd = Vector(h["fwd"]).normalized()
        rest_dir = (mw.to_3x3() @ (hand.bone.tail_local - hand.bone.head_local)).normalized()
        q = Quaternion(fwd, math.radians(h["roll"])) @ rest_dir.rotation_difference(fwd)
        head_w = mw @ ctrl.bone.head_local
        rest_w = mw @ ctrl.bone.matrix_local
        new_w = (
            Matrix.Translation(Vector(h["target"]))
            @ q.to_matrix().to_4x4()
            @ Matrix.Translation(-head_w)
            @ rest_w
        )
        ctrl.matrix_basis = ctrl.bone.matrix_local.inverted() @ (mw.inverted() @ new_w)
        for f in FINGERS:
            arm.pose.bones[f"f_{f}.01.{side}"].rotation_euler.x = math.radians(h["curl"][f])
        arm.pose.bones[f"thumb.02.{side}"].rotation_euler.x = math.radians(h["curl"]["thumb"])
    bpy.context.view_layer.update()


def bake_pose(arm, name):
    """Видимые трансформации костей (с учётом IK и ограничений) → action из одного ключа."""
    visual = {pb.name: pb.matrix.copy() for pb in arm.pose.bones}
    action = bpy.data.actions.new(name)
    action.use_fake_user = True
    return action, visual


def apply_visual(arm, visual):
    # Родители раньше детей: matrix в пространстве арматуры пересчитывает basis.
    def depth(pb):
        d, p = 0, pb.parent
        while p:
            d, p = d + 1, p.parent
        return d

    for pb in sorted(arm.pose.bones, key=depth):
        pb.matrix = visual[pb.name]
        bpy.context.view_layer.update()


def key_all(arm, action):
    arm.animation_data_create()
    arm.animation_data.action = action
    for pb in arm.pose.bones:
        pb.rotation_mode = "QUATERNION"
        for path in ("location", "rotation_quaternion", "scale"):
            pb.keyframe_insert(path, frame=1, group=pb.name)


def render(arm, out):
    sc = bpy.context.scene
    cd = bpy.data.cameras.new("eye")
    cd.lens_unit = "FOV"
    cd.sensor_fit = "VERTICAL"
    cd.angle = math.radians(PREVIEW_FOV_DEG)
    cd.clip_start = 0.01
    cam = bpy.data.objects.new("eye", cd)
    sc.collection.objects.link(cam)
    sc.camera = cam
    cam.location = (0, 0, 0)
    cam.rotation_euler = (math.radians(90 - 5), 0, 0)
    sc.render.engine = "BLENDER_WORKBENCH"
    sc.display.shading.light = "STUDIO"
    sc.display.shading.color_type = "TEXTURE"
    sc.render.resolution_x, sc.render.resolution_y = 960, 540
    sc.render.filepath = str(out)
    bpy.ops.render.render(write_still=True)
    bpy.data.objects.remove(cam)


def main():
    poses_path, out_path, preview_dir = args()
    cfg = json.loads(poses_path.read_text())
    arm = prepare(cfg)
    skin_material()

    baked = []
    reference = {}
    for name, pose in cfg["poses"].items():
        set_pose(arm, pose)
        if preview_dir:
            preview_dir.mkdir(parents=True, exist_ok=True)
            render(arm, preview_dir / f"{name}.png")
        baked.append(bake_pose(arm, name))
        reference[name] = {
            pb.name: list(arm.matrix_world @ pb.head) for pb in arm.pose.bones if pb.name.startswith(("f_", "thumb", "hand."))
        }

    # Ограничения больше не нужны: позы запечены в матрицах.
    for pb in arm.pose.bones:
        for c in list(pb.constraints):
            pb.constraints.remove(c)

    for action, visual in baked:
        apply_visual(arm, visual)
        key_all(arm, action)
    # Покой — первая поза; активный action снимаем, чтобы экспорт взял все.
    arm.animation_data.action = None
    for pb in arm.pose.bones:
        pb.matrix_basis = Matrix.Identity(4)

    out_path.parent.mkdir(parents=True, exist_ok=True)
    bpy.ops.export_scene.gltf(
        filepath=str(out_path),
        export_format="GLB",
        use_selection=False,
        export_cameras=False,
        export_lights=False,
        export_skins=True,
        export_animations=True,
        export_animation_mode="ACTIONS",
        export_force_sampling=True,
        export_yup=True,
        export_texcoords=True,
        export_normals=True,
        export_apply=False,
    )
    # Эталон для tools/blender/check_arms.py: начала костей кисти в каждой позе, м
    # (glTF хранит только суставы, концы костей при импорте угадываются).
    ref_path = out_path.with_suffix(".poses.json")
    ref_path.write_text(json.dumps(reference, indent=1, ensure_ascii=False) + "\n")
    mesh = bpy.data.objects[f"{ARMATURE}:Body"].data
    print(
        f"EXPORT {out_path}: вершин {len(mesh.vertices)}, костей {len(arm.data.bones)}, "
        f"поз {len(baked)}: {', '.join(a.name for a, _ in baked)}"
    )


main()
