//! Фотосканы Poly Haven вместо процедурных сеток (02.11, сессия 3).
//!
//! Сетки — `assets/models/scans/<вариант>.glb` (`tools/blender/scan_lods.py`),
//! у каждого материала скана свои текстуры 1K. Загружаются только материалы
//! подменённых видов: цвет (sRGB) и нормаль (OpenGL) — по слою массива на
//! материал. В вершине `attr[0]` = `SCAN_MATERIAL_BASE` + слой (вместо слота
//! материала скана).

use std::path::{Path, PathBuf};

use super::{LODS, LodRange, PlantAssets, TEXTURE_SIZE, jpeg_rgba, load_glb};

/// Номер материала вершины для слоя 0 массива сканов (`plant_fs`: mat ≥ 16).
pub const SCAN_MATERIAL_BASE: u8 = 16;

/// Вид на GPU → вариант скана и поворот сетки вокруг Y, рад (длинная ось
/// корней — поперёк тропы, как у процедурных).
pub const SCAN_SPECIES: [(usize, &str, f32); 6] = [
    (6, "rock_moss_set_01_rock04", 0.0),
    (7, "fern_02_b", 0.0),
    (9, "shrub_02_a", 0.0),
    (10, "dead_tree_trunk", 0.0),
    (11, "tree_stump_01", 0.0),
    (15, "single_root", std::f32::consts::FRAC_PI_2),
];

pub fn scans_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/models/scans")
}

/// Массивы текстур сканов: слой на материал.
#[derive(Default)]
pub struct ScanTextures {
    /// Файл цвета каждого слоя (ключ — без повторов).
    pub names: Vec<String>,
    /// RGBA8 sRGB, слои по `TEXTURE_SIZE`².
    pub albedo: Vec<u8>,
    /// RGBA8 линейные, нормаль OpenGL.
    pub normal: Vec<u8>,
}

impl ScanTextures {
    pub fn layers(&self) -> usize {
        self.names.len()
    }

    fn layer(&mut self, dir: &Path, albedo: &str, normal: Option<&str>) -> Result<usize, String> {
        if let Some(i) = self.names.iter().position(|n| n == albedo) {
            return Ok(i);
        }
        self.albedo.extend(jpeg_rgba(&dir.join(albedo))?);
        match normal {
            Some(n) => self.normal.extend(jpeg_rgba(&dir.join(n))?),
            // Без нормали — плоская (0,5; 0,5; 1).
            None => self
                .normal
                .extend([128u8, 128, 255, 255].repeat(TEXTURE_SIZE * TEXTURE_SIZE)),
        }
        self.names.push(albedo.to_string());
        Ok(self.names.len() - 1)
    }
}

/// Загружает вариант скана в общие буферы растений; материалы — в массивы.
pub fn load_variant(
    dir: &Path,
    catalog: &serde_json::Value,
    variant: &str,
    yaw: f32,
    tex: &mut ScanTextures,
    out: &mut PlantAssets,
) -> Result<[LodRange; LODS], String> {
    let entry = catalog
        .get(variant)
        .ok_or_else(|| format!("scans.json: нет варианта {variant}"))?;
    let mats = entry["materials"]
        .as_array()
        .ok_or_else(|| format!("scans.json: {variant} без материалов"))?;
    let mut layer_of_slot = Vec::new();
    for m in mats {
        let albedo = m["albedo"]
            .as_str()
            .ok_or_else(|| format!("{variant}: материал без цвета"))?;
        layer_of_slot.push(tex.layer(dir, albedo, m["normal"].as_str())?);
    }
    let first = out.vertices.len();
    let lods = load_glb(&dir.join(format!("{variant}.glb")), out)?;
    let (s, c) = yaw.sin_cos();
    for v in &mut out.vertices[first..] {
        let slot = v.attr[0] as usize;
        let layer = *layer_of_slot.get(slot).unwrap_or(&layer_of_slot[0]);
        v.attr[0] = SCAN_MATERIAL_BASE + layer as u8;
        let [x, y, z] = v.position;
        v.position = [c * x - s * z, y, s * x + c * z];
        let [nx, ny, nz] = v.normal;
        v.normal = [c * nx - s * nz, ny, s * nx + c * nz];
    }
    Ok(lods)
}
