//! Загрузка рук из `.glb` (крейт `gltf`): сетка со скином, скелет, позы
//! (анимации из одного ключа) и текстура кожи.

use std::collections::HashMap;
use std::path::Path;

use glam::{Mat4, Quat, Vec3};
use gltf::animation::util::ReadOutputs;

use super::skeleton::{Joint, Pose, Skeleton, Trs};
use crate::layout::SkinnedVertex;

/// Руки, готовые к загрузке на GPU и к анимации.
pub struct HandsAsset {
    pub vertices: Vec<SkinnedVertex>,
    pub indices: Vec<u16>,
    pub skeleton: Skeleton,
    /// Позы по именам анимаций: `relaxed`, `grip`, `point`, `reach`.
    pub poses: Vec<(String, Pose)>,
    /// Альбедо кожи RGBA8 (sRGB), квадрат `albedo_size`².
    pub albedo: Vec<u8>,
    pub albedo_size: u32,
}

impl HandsAsset {
    pub fn pose(&self, name: &str) -> Option<&Pose> {
        self.poses.iter().find(|(n, _)| n == name).map(|(_, p)| p)
    }
}

fn trs(node: &gltf::Node) -> Trs {
    let (t, r, s) = node.transform().decomposed();
    Trs {
        t: Vec3::from(t),
        r: Quat::from_array(r),
        s: Vec3::from(s),
    }
}

pub fn load(path: &Path) -> Result<HandsAsset, String> {
    let err = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
    let bytes = std::fs::read(path).map_err(|e| err(&e))?;
    let gltf = gltf::Gltf::from_slice(&bytes).map_err(|e| err(&e))?;
    let blob = gltf
        .blob
        .as_deref()
        .ok_or_else(|| err(&"нет двоичного блока GLB"))?;
    let buffer = |b: gltf::Buffer| match b.source() {
        gltf::buffer::Source::Bin => Some(blob),
        gltf::buffer::Source::Uri(_) => None,
    };
    let doc = &gltf.document;

    // Родители узлов: glTF хранит только детей.
    let mut parent = HashMap::new();
    for n in doc.nodes() {
        for c in n.children() {
            parent.insert(c.index(), n.index());
        }
    }
    let nodes: Vec<gltf::Node> = doc.nodes().collect();

    let skinned = nodes
        .iter()
        .find(|n| n.mesh().is_some() && n.skin().is_some())
        .ok_or_else(|| err(&"нет сетки со скином"))?;
    let skin = skinned.skin().unwrap();
    let mesh = skinned.mesh().unwrap();
    let joint_nodes: Vec<usize> = skin.joints().map(|j| j.index()).collect();
    let joint_of: HashMap<usize, usize> = joint_nodes
        .iter()
        .enumerate()
        .map(|(i, &n)| (n, i))
        .collect();
    let ibm: Vec<Mat4> = skin
        .reader(buffer)
        .read_inverse_bind_matrices()
        .ok_or_else(|| err(&"нет обратных матриц привязки"))?
        .map(|m| Mat4::from_cols_array_2d(&m))
        .collect();

    let mut joints = Vec::with_capacity(joint_nodes.len());
    for (i, &n) in joint_nodes.iter().enumerate() {
        // Вверх по узлам до сустава-родителя, копя неподвижные узлы.
        let mut pre = Mat4::IDENTITY;
        let mut up = parent.get(&n).copied();
        let mut parent_joint = None;
        while let Some(p) = up {
            if let Some(&j) = joint_of.get(&p) {
                parent_joint = Some(j);
                break;
            }
            pre = Mat4::from_cols_array_2d(&nodes[p].transform().matrix()) * pre;
            up = parent.get(&p).copied();
        }
        joints.push(Joint {
            name: nodes[n].name().unwrap_or("").to_string(),
            parent: parent_joint,
            pre,
            rest: trs(&nodes[n]),
            inverse_bind: ibm.get(i).copied().unwrap_or(Mat4::IDENTITY),
        });
    }
    let skeleton = Skeleton::new(joints)?;

    let mut prims = mesh.primitives();
    let prim = prims.next().ok_or_else(|| err(&"сетка без примитивов"))?;
    if prims.next().is_some() {
        return Err(err(&"ожидался один примитив сетки"));
    }
    let r = prim.reader(buffer);
    let positions: Vec<[f32; 3]> = r
        .read_positions()
        .ok_or_else(|| err(&"нет POSITION"))?
        .collect();
    let normals: Vec<[f32; 3]> = r
        .read_normals()
        .ok_or_else(|| err(&"нет NORMAL"))?
        .collect();
    let uvs: Vec<[f32; 2]> = r
        .read_tex_coords(0)
        .ok_or_else(|| err(&"нет TEXCOORD_0"))?
        .into_f32()
        .collect();
    let joints_attr: Vec<[u16; 4]> = r
        .read_joints(0)
        .ok_or_else(|| err(&"нет JOINTS_0"))?
        .into_u16()
        .collect();
    let weights: Vec<[f32; 4]> = r
        .read_weights(0)
        .ok_or_else(|| err(&"нет WEIGHTS_0"))?
        .into_f32()
        .collect();
    let joint_count = skeleton.joints.len();
    let mut vertices = Vec::with_capacity(positions.len());
    for i in 0..positions.len() {
        let j = joints_attr[i];
        if j.iter().any(|&x| x as usize >= joint_count || x > 255) {
            return Err(err(&format!("вершина {i}: индекс кости вне скелета")));
        }
        // Веса нормируются: экспортёр даёт сумму 1 ± 1e-3.
        let w = weights[i];
        let sum: f32 = w.iter().sum();
        vertices.push(SkinnedVertex {
            position: positions[i],
            normal: normals[i],
            uv: uvs[i],
            joints: j.map(|x| x as u8),
            weights: w.map(|x| if sum > 0.0 { x / sum } else { 0.0 }),
        });
    }
    let indices: Vec<u16> = r
        .read_indices()
        .ok_or_else(|| err(&"нет индексов"))?
        .into_u32()
        .map(|i| u16::try_from(i).map_err(|_| err(&"индекс больше u16")))
        .collect::<Result<_, _>>()?;

    // Позы: каждая анимация задаёт по одному ключу на кость.
    let mut poses = Vec::new();
    for anim in doc.animations() {
        let mut pose = skeleton.rest_pose();
        for ch in anim.channels() {
            let Some(&j) = joint_of.get(&ch.target().node().index()) else {
                continue;
            };
            let t = &mut pose.0[j];
            match ch.reader(buffer).read_outputs() {
                Some(ReadOutputs::Translations(mut v)) => {
                    if let Some(x) = v.next() {
                        t.t = Vec3::from(x);
                    }
                }
                Some(ReadOutputs::Rotations(v)) => {
                    if let Some(x) = v.into_f32().next() {
                        t.r = Quat::from_array(x).normalize();
                    }
                }
                Some(ReadOutputs::Scales(mut v)) => {
                    if let Some(x) = v.next() {
                        t.s = Vec3::from(x);
                    }
                }
                _ => {}
            }
        }
        poses.push((anim.name().unwrap_or("").to_string(), pose));
    }

    let (albedo, albedo_size) = load_albedo(doc, blob).map_err(|e| err(&e))?;
    Ok(HandsAsset {
        vertices,
        indices,
        skeleton,
        poses,
        albedo,
        albedo_size,
    })
}

/// Первая текстура (PNG во встроенном буфере) → RGBA8.
fn load_albedo(doc: &gltf::Document, blob: &[u8]) -> Result<(Vec<u8>, u32), String> {
    let image = doc.images().next().ok_or("нет текстуры кожи")?;
    let gltf::image::Source::View { view, .. } = image.source() else {
        return Err("текстура не встроена в GLB".into());
    };
    let data = &blob[view.offset()..view.offset() + view.length()];
    let mut dec = png::Decoder::new(std::io::Cursor::new(data));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = dec.read_info().map_err(|e| e.to_string())?;
    let size = reader
        .output_buffer_size()
        .ok_or("слишком большая текстура")?;
    let mut buf = vec![0; size];
    let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    buf.truncate(info.buffer_size());
    if info.width != info.height {
        return Err(format!(
            "текстура {}×{} не квадратная",
            info.width, info.height
        ));
    }
    let channels = reader.output_color_type().0.samples();
    let rgba = match channels {
        4 => buf,
        3 => buf
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        1 => buf.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        2 => buf
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[0], p[0], p[1]])
            .collect(),
        _ => return Err(format!("{channels} каналов в текстуре")),
    };
    Ok((rgba, info.width))
}
