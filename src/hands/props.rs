//! Предметы в руках (этап 10): сканы Poly Haven (CC0) жёстко привязаны к кости
//! кисти и рисуются тем же проходом `viewmodel`, что и руки.
//!
//! Сетка предмета — `SkinnedVertex` с одной костью и весом 1. Вершины лежат
//! в пространстве привязки: `v_bind = inverse(inverse_bind_j) · local · v`,
//! поэтому палитра кадра даёт `root · G_j(поза) · local · v` — предмет следует
//! за кистью (позы, покачивание, отставание).
//!
//! `local` задаётся в `assets/models/props/props.json` в пространстве глаза:
//! где предмет висит относительно кисти в позе `grip` (сдвиг, повороты XYZ в
//! градусах) — так его удобно подбирать по снимку.

use std::path::{Path, PathBuf};

use glam::{EulerRot, Mat4, Quat, Vec3};
use serde::Deserialize;
use zune_jpeg::JpegDecoder;
use zune_jpeg::zune_core::bytestream::ZCursor;
use zune_jpeg::zune_core::colorspace::ColorSpace;
use zune_jpeg::zune_core::options::DecoderOptions;

use super::HandsAsset;
use crate::layout::SkinnedVertex;

/// Описание предмета в `props.json`.
#[derive(Deserialize, Clone, Debug)]
pub struct PropSpec {
    /// Файл `<name>.glb` в `assets/models/props`.
    pub name: String,
    /// Название в интерфейсе.
    pub label: String,
    /// Кость, к которой привязан предмет (`hand.R` / `hand.L`).
    pub joint: String,
    /// Сдвиг от начала кисти в позе `grip`, м, в пространстве глаза.
    pub offset: [f32; 3],
    /// Поворот XYZ, градусы, в пространстве глаза.
    pub rotation_deg: [f32; 3],
    #[serde(default = "one")]
    pub scale: f32,
}

fn one() -> f32 {
    1.0
}

/// Предмет, готовый к загрузке на GPU.
pub struct HandProp {
    pub spec: PropSpec,
    pub vertices: Vec<SkinnedVertex>,
    pub indices: Vec<u32>,
    /// RGBA8 sRGB, квадрат `albedo_size`².
    pub albedo: Vec<u8>,
    pub albedo_size: u32,
}

pub fn props_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/models/props")
}

/// Загружает все предметы из `props.json`, привязывая их к кистям в позе
/// `grip_pose` (индекс в `POSE_NAMES`, позы — из ассета рук).
pub fn load_props(hands: &HandsAsset, grip_pose: &str) -> Result<Vec<HandProp>, String> {
    let dir = props_dir();
    let cfg = dir.join("props.json");
    let text = std::fs::read_to_string(&cfg).map_err(|e| format!("{}: {e}", cfg.display()))?;
    let specs: Vec<PropSpec> =
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", cfg.display()))?;
    let pose = hands
        .pose(grip_pose)
        .ok_or(format!("нет позы {grip_pose}"))?;
    let globals = hands.skeleton.globals(pose);
    specs
        .into_iter()
        .map(|spec| {
            let j = hands
                .skeleton
                .joint(&spec.joint)
                .ok_or(format!("предмет {}: нет кости {}", spec.name, spec.joint))?;
            let g = globals[j];
            let r = spec.rotation_deg.map(f32::to_radians);
            // Где предмет в глазу в позе grip: у начала кисти со сдвигом.
            let placed = Mat4::from_scale_rotation_translation(
                Vec3::splat(spec.scale),
                Quat::from_euler(EulerRot::XYZ, r[0], r[1], r[2]),
                g.w_axis.truncate() + Vec3::from(spec.offset),
            );
            let local = g.inverse() * placed;
            let to_bind = hands.skeleton.joints[j].inverse_bind.inverse() * local;
            load_mesh(&dir.join(format!("{}.glb", spec.name)), j as u8, to_bind).map(
                |(vertices, indices, albedo, albedo_size)| HandProp {
                    spec,
                    vertices,
                    indices,
                    albedo,
                    albedo_size,
                },
            )
        })
        .collect()
}

type Mesh = (Vec<SkinnedVertex>, Vec<u32>, Vec<u8>, u32);

fn load_mesh(path: &Path, joint: u8, to_bind: Mat4) -> Result<Mesh, String> {
    let err = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
    let bytes = std::fs::read(path).map_err(|e| err(&e))?;
    let gltf = gltf::Gltf::from_slice(&bytes).map_err(|e| err(&e))?;
    let blob = gltf.blob.as_deref().ok_or_else(|| err(&"нет блока GLB"))?;
    let buffer = |_: gltf::Buffer| Some(blob);
    let mesh = gltf
        .document
        .meshes()
        .next()
        .ok_or_else(|| err(&"нет сетки"))?;
    let prim = mesh
        .primitives()
        .next()
        .ok_or_else(|| err(&"нет примитива"))?;
    let r = prim.reader(buffer);
    let pos: Vec<[f32; 3]> = r
        .read_positions()
        .ok_or_else(|| err(&"нет POSITION"))?
        .collect();
    let nrm: Vec<[f32; 3]> = r
        .read_normals()
        .ok_or_else(|| err(&"нет NORMAL"))?
        .collect();
    let uv: Vec<[f32; 2]> = r
        .read_tex_coords(0)
        .ok_or_else(|| err(&"нет TEXCOORD_0"))?
        .into_f32()
        .collect();
    let indices: Vec<u32> = r
        .read_indices()
        .ok_or_else(|| err(&"нет индексов"))?
        .into_u32()
        .collect();
    let vertices = pos
        .iter()
        .zip(&nrm)
        .zip(&uv)
        .map(|((p, n), t)| SkinnedVertex {
            position: to_bind.transform_point3(Vec3::from(*p)).to_array(),
            normal: to_bind
                .transform_vector3(Vec3::from(*n))
                .normalize_or_zero()
                .to_array(),
            uv: *t,
            joints: [joint, 0, 0, 0],
            weights: [1.0, 0.0, 0.0, 0.0],
        })
        .collect();
    let image = gltf
        .document
        .images()
        .next()
        .ok_or_else(|| err(&"нет текстуры"))?;
    let gltf::image::Source::View { view, .. } = image.source() else {
        return Err(err(&"текстура не внутри GLB"));
    };
    let jpeg = &blob[view.offset()..view.offset() + view.length()];
    let mut dec = JpegDecoder::new_with_options(
        ZCursor::new(jpeg),
        DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGBA),
    );
    let rgba = dec.decode().map_err(|e| err(&format!("{e:?}")))?;
    let info = dec.info().ok_or_else(|| err(&"нет заголовка JPEG"))?;
    if info.width != info.height || rgba.len() != info.width as usize * info.height as usize * 4 {
        return Err(err(&"текстура не квадратная RGBA"));
    }
    Ok((vertices, indices, rgba, info.width as u32))
}
