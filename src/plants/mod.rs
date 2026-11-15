//! Растения (02.8): сетки видов с тремя LOD из `assets/models/plants/*.glb`
//! (`tools/plants/build_plants.py`) и текстуры коры и листвы — в общие
//! буферы для отрисовки леса на GPU (02.4).

use std::path::{Path, PathBuf};

use zune_jpeg::JpegDecoder;
use zune_jpeg::zune_core::bytestream::ZCursor;
use zune_jpeg::zune_core::colorspace::ColorSpace;
use zune_jpeg::zune_core::options::DecoderOptions;

use crate::layout::PlantVertex;

pub mod scans;

/// Виды в порядке номеров на GPU (`SPECIES_*` в `forest.metal`).
/// 0–5 деревья (три криптомерии, три лиственных), 6 валун, 7 папоротник,
/// 8 якудзаса, 9 кустарник, 10 упавший ствол, 11 полый пень, 12 вывороченное
/// дерево, 13 лесной сор, 14 подушка мха, 15 корни и камни на тропе; 16–37 —
/// варианты сканов и трава; 38 гриб — только с CPU.
pub const SPECIES: [&str; 42] = [
    "sugi_a",
    "sugi_b",
    "sugi_old",
    "broad_a",
    "broad_b",
    "broad_c",
    "boulder",
    "fern",
    "sasa",
    "shrub",
    "log",
    "stump",
    "windfall",
    "debris",
    "moss_cushion",
    "trail_roots",
    // 16–37 — варианты фотосканов (02.11, сессия 4), см. `scans::SCAN_SPECIES`.
    "boulder_v1",
    "boulder_v2",
    "boulder_v3",
    "fern_v1",
    "fern_v2",
    "fern_v3",
    "shrub_v1",
    "shrub_v2",
    "log_v1",
    "stump_v1",
    "roots_v1",
    "roots_v2",
    "roots_v3",
    "debris_v1",
    "debris_v2",
    "debris_v3",
    "debris_v4",
    "grass_a",
    "grass_b",
    "grass_c",
    "grass_d",
    "grass_e",
    // 38–40 — пучки травинок для густой травы (02.11), процедурные.
    "grass_blades_a",
    "grass_blades_b",
    "grass_blades_c",
    "mushroom",
];
pub const LODS: usize = 3;
/// Сторона слоя массива текстур растений.
pub const TEXTURE_SIZE: usize = 1024;
/// Слои: 0 кора якусуги, 1 кора лиственного, 2 атлас листвы и подлеска.
/// Кора якусуги, кора лиственных, атлас листвы, фото травинок, фото кустиков травы (02.13).
pub const TEXTURE_LAYERS: usize = 5;

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
    pub lods: [[LodRange; LODS]; SPECIES.len()],
    /// RGBA8 sRGB, `TEXTURE_LAYERS` × `TEXTURE_SIZE`².
    pub albedo: Vec<u8>,
    /// Нормали коры (OpenGL, RGBA8 линейные): 0 якусуги, 1 лиственные.
    pub bark_normal: Vec<u8>,
    /// Текстуры фотосканов подменённых видов (02.11).
    pub scans: scans::ScanTextures,
}

pub fn models_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/models/plants")
}

pub(crate) fn load_glb(path: &Path, out: &mut PlantAssets) -> Result<[LodRange; LODS], String> {
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

pub(crate) fn jpeg_rgba(path: &Path) -> Result<Vec<u8>, String> {
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
        lods: [[LodRange::default(); LODS]; SPECIES.len()],
        albedo: Vec::with_capacity(TEXTURE_LAYERS * TEXTURE_SIZE * TEXTURE_SIZE * 4),
        bark_normal: Vec::with_capacity(2 * TEXTURE_SIZE * TEXTURE_SIZE * 4),
        scans: scans::ScanTextures::default(),
    };
    // Фотосканы (02.11) — вместо процедурных сеток части видов; без каталога
    // сканов — процедурные.
    let scan_dir = scans::scans_dir();
    let catalog: Option<serde_json::Value> = std::fs::read_to_string(scan_dir.join("scans.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok());
    let mut tex = scans::ScanTextures::default();
    for (s, name) in SPECIES.iter().enumerate() {
        // Скан — если он есть в каталоге и его сетка на диске (тяжёлые
        // деревья-сканы не в git). Пихта-скан вместо криптомерий (виды 0–1)
        // — только с `ECHO_SCAN_TREES=1`: +8 мс GPU (перерисовка карточек
        // хвои), до бюджета кадра 02.13 сессии 3.
        let scan = catalog.as_ref().and_then(|c| {
            scans::SCAN_SPECIES
                .iter()
                .find(|x| {
                    x.0 == s
                        && scan_dir.join(format!("{}.glb", x.1)).exists()
                        && (x.0 >= 2 || std::env::var_os("ECHO_SCAN_TREES").is_some())
                })
                .map(|x| (c, x))
        });
        a.lods[s] = match scan {
            Some((c, &(_, variant, yaw, scale))) => {
                let mut lods =
                    scans::load_variant(&scan_dir, c, variant, yaw, scale, &mut tex, &mut a)?;
                if s < 2 {
                    // Пихта-скан вдали (> 150 м, тысячи деревьев) — LOD 2
                    // процедурной криптомерии: 0,7 тыс. треугольников вместо
                    // 7,8 тыс., силуэт — тот же конус (02.13).
                    let p = load_glb(&models.join(format!("{name}.glb")), &mut a)?;
                    lods[2] = p[2];
                }
                lods
            }
            None => load_glb(&models.join(format!("{name}.glb")), &mut a)?,
        };
    }
    a.scans = tex;
    // Кора якусуги — скан японской криптомерии (Poly Haven, 02.11).
    a.albedo.extend(jpeg_rgba(
        &ambientcg.join("PH_japanese_cedar_bark/Color.jpg"),
    )?);
    a.albedo
        .extend(jpeg_rgba(&ambientcg.join("Bark007/Color.jpg"))?);
    for bark in ["PH_japanese_cedar_bark", "Bark007"] {
        a.bark_normal
            .extend(jpeg_rgba(&ambientcg.join(bark).join("NormalGL.jpg"))?);
    }
    let (atlas, w, h) = png_rgba(&models.join("foliage.png"))?;
    if (w, h) != (TEXTURE_SIZE, TEXTURE_SIZE) {
        return Err("foliage.png: нужен 1024 × 1024".into());
    }
    a.albedo.extend(atlas);
    // Фототекстуры травы Poly Haven (02.13, `grass_photo.py`): слои 3 и 4.
    for name in ["grass_strips.png", "grass_tufts.png"] {
        let (px, w, h) = png_rgba(&models.join(name))?;
        if (w, h) != (TEXTURE_SIZE, TEXTURE_SIZE) {
            return Err(format!("{name}: нужен 1024 × 1024"));
        }
        a.albedo.extend(px);
    }
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
        // Процедурные материалы 0–8, сканы — 16 + слой (02.11).
        assert!(a.vertices.iter().all(|v| v.attr[0] <= 9 || v.attr[0] >= 16));
        assert!(a.scans.layers() > 0, "сканы не загружены");
    }
}
