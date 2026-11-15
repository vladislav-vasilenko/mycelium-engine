//! Фотосканы Poly Haven вместо процедурных сеток (02.11, сессия 3).
//!
//! Сетки — `assets/models/scans/<вариант>.glb` (`tools/blender/scan_lods.py`),
//! у каждого материала скана свои текстуры 1K. Загружаются только материалы
//! подменённых видов: цвет (sRGB) и нормаль (OpenGL) — по слою массива на
//! материал. В вершине `attr[0]` = `SCAN_MATERIAL_BASE` + слой (вместо слота
//! материала скана).

use std::path::{Path, PathBuf};

use super::{LODS, LodRange, PlantAssets, TEXTURE_SIZE, jpeg_rgba, load_glb};

/// Номер материала вершины для слоя 0 массива сканов (`plant_shade`: mat ≥ 16).
pub const SCAN_MATERIAL_BASE: u8 = 16;

/// Вид на GPU → вариант скана, поворот сетки вокруг Y, рад, и масштаб сетки
/// (пихта-скан 14–19 м → криптомерия 22–27 м, 02.13). Длинная ось
/// корней — поперёк тропы, как у процедурных). Корни — только тонкие
/// (`root_cluster_02`): у `single_root`, `pine_roots` и `root_cluster_02_a` —
/// куски тёмной почвы, на тропе они лежали «одеялом».
pub const SCAN_SPECIES: [(usize, &str, f32, f32); 30] = [
    // Деревья-сканы (02.13): пихта вместо криптомерий — только с
    // `ECHO_SCAN_TREES=1`; сетки не в git — без них виды процедурные
    // (`scan_lods.py -- --heavy`).
    (0, "fir_tree_01_c", 0.0, 1.8),
    (1, "fir_tree_01_c", 0.0, 1.55),
    (6, "rock_moss_set_01_rock04", 0.0, 1.0),
    (7, "fern_02_b", 0.0, 1.0),
    (9, "shrub_02_a", 0.0, 1.0),
    (10, "dead_tree_trunk", 0.0, 1.0),
    (11, "tree_stump_01", 0.0, 1.0),
    (15, "root_cluster_02_d", std::f32::consts::FRAC_PI_2, 1.0),
    (16, "rock_moss_set_01_rock06", 0.0, 1.0),
    (17, "rock_moss_set_02_rock10", 0.0, 1.0),
    (18, "rock_moss_set_02_rock13", 0.0, 1.0),
    (19, "fern_02_a", 0.0, 1.0),
    (20, "fern_02_c", 0.0, 1.0),
    (21, "fern_02_d", 0.0, 1.0),
    (22, "island_tree_02", 0.0, 1.2), // подрост: молодое широколиственное (02.13)
    (23, "tree_small_02", 0.0, 1.0),  // подрост (02.13)
    (24, "dead_tree_trunk_02", 0.0, 1.0),
    (25, "tree_stump_02", 0.0, 1.0),
    (26, "root_cluster_02_f", std::f32::consts::FRAC_PI_2, 1.0),
    (27, "root_cluster_02_b", std::f32::consts::FRAC_PI_2, 1.0),
    (28, "root_cluster_02_g", std::f32::consts::FRAC_PI_2, 1.0),
    (29, "dry_branches_medium_01_a", 0.0, 1.0),
    (30, "dry_branches_medium_01_b", 0.0, 1.0),
    (31, "bark_debris_01_a", 0.0, 1.0),
    (32, "stone_01", 0.0, 1.0),
    (33, "shrub_03_a", 0.0, 1.0),
    (34, "shrub_03_b", 0.0, 1.0),
    (35, "shrub_03_c", 0.0, 1.0),
    (36, "shrub_03_d", 0.0, 1.0),
    (37, "shrub_04", 0.0, 1.0),
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

    fn layer(
        &mut self,
        dir: &Path,
        albedo: &str,
        normal: Option<&str>,
        mask: Option<&str>,
    ) -> Result<usize, String> {
        if let Some(i) = self.names.iter().position(|n| n == albedo) {
            return Ok(i);
        }
        let mut rgba = jpeg_rgba(&dir.join(albedo))?;
        // Альфа-маска листвы и травы — в альфа-канал цвета (отсечение в шейдере).
        if let Some(m) = mask {
            let a = gray_png(&dir.join(m))?;
            for (px, v) in rgba.as_chunks_mut::<4>().0.iter_mut().zip(a) {
                px[3] = v;
            }
        }
        self.albedo.extend(rgba);
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
    scale: f32,
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
        layer_of_slot.push(tex.layer(dir, albedo, m["normal"].as_str(), m["mask"].as_str())?);
    }
    let first = out.vertices.len();
    let lods = load_glb(&dir.join(format!("{variant}.glb")), out)?;
    let (s, c) = yaw.sin_cos();
    for v in &mut out.vertices[first..] {
        let slot = v.attr[0] as usize;
        let layer = *layer_of_slot.get(slot).unwrap_or(&layer_of_slot[0]);
        v.attr[0] = SCAN_MATERIAL_BASE + layer as u8;
        let [x, y, z] = v.position;
        v.position = [(c * x - s * z) * scale, y * scale, (s * x + c * z) * scale];
        let [nx, ny, nz] = v.normal;
        v.normal = [c * nx - s * nz, ny, s * nx + c * nz];
    }
    Ok(lods)
}

/// Маска PNG 1024² (серая или RGBA) → яркость по пикселям.
fn gray_png(path: &Path) -> Result<Vec<u8>, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut dec = png::Decoder::new(std::io::BufReader::new(file));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = dec
        .read_info()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let mut buf = vec![0; reader.output_buffer_size().unwrap_or(0)];
    let info = reader
        .next_frame(&mut buf)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if (info.width as usize, info.height as usize) != (TEXTURE_SIZE, TEXTURE_SIZE) {
        return Err(format!("{}: нужна маска {TEXTURE_SIZE}²", path.display()));
    }
    let ch = info.color_type.samples();
    Ok(buf[..info.buffer_size()]
        .chunks_exact(ch)
        .map(|p| p[0])
        .collect())
}
