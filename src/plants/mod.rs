//! Растения (02.8): сетки видов с тремя LOD из `assets/models/plants/*.glb`
//! (`tools/plants/build_plants.py`) и текстуры коры и листвы — в общие
//! буферы для отрисовки леса на GPU (02.4).

use std::path::{Path, PathBuf};

use zune_jpeg::JpegDecoder;
use zune_jpeg::zune_core::bytestream::ZCursor;
use zune_jpeg::zune_core::colorspace::ColorSpace;
use zune_jpeg::zune_core::options::DecoderOptions;

use crate::layout::PlantVertex;

/// Виды в порядке номеров на GPU (`SPECIES_*` в `forest.metal`).
pub const SPECIES: [&str; 3] = ["yakusugi", "broadleaf", "boulder"];
pub const LODS: usize = 3;
/// Сторона слоя массива текстур растений.
pub const TEXTURE_SIZE: usize = 1024;
/// Слои: 0 кора якусуги, 1 кора лиственного, 2 атлас листвы (верхняя половина).
pub const TEXTURE_LAYERS: usize = 3;

/// LOD вида в общих буферах: (первый индекс, число индексов, базовая вершина).
#[derive(Clone, Copy, Debug, Default)]
pub struct LodRange {
    pub index_start: u32,
    pub index_count: u32,
    pub base_vertex: u32,
}

pub struct PlantAssets {
    pub vertices: Vec<PlantVertex>,
    pub indices: Vec<u32>,
    pub lods: [[LodRange; LODS]; 3],
    /// RGBA8 sRGB, `TEXTURE_LAYERS` × `TEXTURE_SIZE`².
    pub albedo: Vec<u8>,
}

pub fn models_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/models/plants")
}

fn load_glb(path: &Path, out: &mut PlantAssets) -> Result<[LodRange; LODS], String> {
    let err = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
    let bytes = std::fs::read(path).map_err(|e| err(&e))?;
    let gltf = gltf::Gltf::from_slice(&bytes).map_err(|e| err(&e))?;
    let blob = gltf
        .blob
        .as_deref()
        .ok_or_else(|| err(&"нет двоичного блока"))?;
    let mut lods = [LodRange::default(); LODS];
    for mesh in gltf.document.meshes() {
        let name = mesh.name().unwrap_or("");
        let Some(l) = name
            .strip_prefix("lod")
            .and_then(|s| s.parse::<usize>().ok())
        else {
            continue;
        };
        if l >= LODS {
            continue;
        }
        let prim = mesh
            .primitives()
            .next()
            .ok_or_else(|| err(&"пустая сетка"))?;
        let r = prim.reader(|_| Some(blob));
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
        let col: Vec<[u8; 4]> = r
            .read_colors(0)
            .ok_or_else(|| err(&"нет COLOR_0"))?
            .into_rgba_u8()
            .collect();
        let idx: Vec<u32> = r
            .read_indices()
            .ok_or_else(|| err(&"нет индексов"))?
            .into_u32()
            .collect();
        lods[l] = LodRange {
            index_start: out.indices.len() as u32,
            index_count: idx.len() as u32,
            base_vertex: out.vertices.len() as u32,
        };
        out.vertices.extend((0..pos.len()).map(|i| PlantVertex {
            position: pos[i],
            normal: nrm[i],
            uv: uv[i],
            attr: col[i],
        }));
        out.indices.extend(idx);
    }
    if lods.iter().any(|l| l.index_count == 0) {
        return Err(err(&"нужны сетки lod0, lod1, lod2"));
    }
    Ok(lods)
}

fn jpeg_rgba(path: &Path) -> Result<Vec<u8>, String> {
    let data = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut dec = JpegDecoder::new_with_options(
        ZCursor::new(data),
        DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGBA),
    );
    let px = dec
        .decode()
        .map_err(|e| format!("{}: {e:?}", path.display()))?;
    if px.len() != TEXTURE_SIZE * TEXTURE_SIZE * 4 {
        return Err(format!("{}: нужен JPEG {TEXTURE_SIZE}²", path.display()));
    }
    Ok(px)
}

fn png_rgba(path: &Path) -> Result<(Vec<u8>, usize, usize), String> {
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut dec = png::Decoder::new(std::io::BufReader::new(file));
    dec.set_transformations(png::Transformations::EXPAND);
    let mut reader = dec
        .read_info()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let mut buf = vec![0; reader.output_buffer_size().unwrap_or(0)];
    let info = reader
        .next_frame(&mut buf)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        return Err(format!("{}: нужен RGBA 8 бит", path.display()));
    }
    buf.truncate(info.buffer_size());
    Ok((buf, info.width as usize, info.height as usize))
}

pub fn load(models: &Path, ambientcg: &Path) -> Result<PlantAssets, String> {
    let mut a = PlantAssets {
        vertices: Vec::new(),
        indices: Vec::new(),
        lods: [[LodRange::default(); LODS]; 3],
        albedo: Vec::with_capacity(TEXTURE_LAYERS * TEXTURE_SIZE * TEXTURE_SIZE * 4),
    };
    for (s, name) in SPECIES.iter().enumerate() {
        a.lods[s] = load_glb(&models.join(format!("{name}.glb")), &mut a)?;
    }
    a.albedo
        .extend(jpeg_rgba(&ambientcg.join("Bark012/Color.jpg"))?);
    a.albedo
        .extend(jpeg_rgba(&ambientcg.join("Bark007/Color.jpg"))?);
    // Атлас листвы 1024 × 512 — в верхнюю половину квадратного слоя.
    let (atlas, w, h) = png_rgba(&models.join("foliage.png"))?;
    if w != TEXTURE_SIZE || h * 2 != TEXTURE_SIZE {
        return Err("foliage.png: нужен 1024 × 512".into());
    }
    a.albedo.extend(atlas);
    a.albedo
        .resize(TEXTURE_LAYERS * TEXTURE_SIZE * TEXTURE_SIZE * 4, 0);
    Ok(a)
}

#[cfg(test)]
mod tests {
    #[test]
    fn plants_load_with_three_lods() {
        let a = super::load(
            &super::models_dir(),
            &crate::terrain_materials::source_dir(),
        )
        .unwrap();
        for (s, lods) in a.lods.iter().enumerate() {
            // LOD дешевеют.
            assert!(lods[0].index_count > lods[1].index_count, "вид {s}");
            assert!(lods[1].index_count > lods[2].index_count, "вид {s}");
        }
        let max = a.vertices.len() as u32;
        assert!(a.indices.iter().all(|&i| i < max));
        assert!(a.vertices.iter().all(|v| v.attr[0] <= 4));
    }
}
