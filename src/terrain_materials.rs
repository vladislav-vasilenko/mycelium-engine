//! Материалы ландшафта: фотосканы ambientCG → два массива текстур.
//!
//! Источник — `assets/source/ambientcg/<Id>/{Color,NormalGL,Roughness,Displacement}.jpg`
//! (CC0, 1024 × 1024). Сборка на CPU при запуске:
//!
//! - `albedo_height`: RGBA8, RGB — цвет (sRGB), A — высота рельефа материала
//!   для смешивания по высоте; на GPU формат `RGBA8Unorm_sRGB`, альфа линейная;
//! - `normal_rough`: RGBA8 линейный, RG — нормаль в касательном пространстве
//!   (OpenGL, +Y вверх по картинке), B — шероховатость, A — резерв (255).
//!
//! Номера слоёв совпадают с константами `MAT_*` в `shaders/metal/terrain.metal`.

use std::path::{Path, PathBuf};

use zune_jpeg::JpegDecoder;
use zune_jpeg::zune_core::bytestream::ZCursor;
use zune_jpeg::zune_core::colorspace::ColorSpace;
use zune_jpeg::zune_core::options::DecoderOptions;

/// Слой массива: имя материала ambientCG и роль в движке.
pub struct MaterialLayer {
    pub id: &'static str,
    pub role: &'static str,
}

/// Порядок слоёв = индекс в массиве текстур = `MAT_*` в MSL. Размер повтора
/// каждого слоя на земле — `MAT_TILE_M` в `terrain.metal`.
pub const LAYERS: [MaterialLayer; 5] = [
    MaterialLayer {
        id: "Rock062",
        role: "камень",
    },
    MaterialLayer {
        id: "Ground093A",
        role: "песок",
    },
    MaterialLayer {
        id: "Ground082S",
        role: "почва",
    },
    MaterialLayer {
        id: "Moss002",
        role: "мох",
    },
    MaterialLayer {
        id: "Ground037",
        role: "лесная подстилка",
    },
];

/// Сторона каждой текстуры, пиксели.
pub const MATERIAL_SIZE: usize = 1024;

pub struct MaterialArrays {
    pub size: usize,
    pub layers: usize,
    /// `layers × size × size × 4` байт, слой за слоем.
    pub albedo_height: Vec<u8>,
    pub normal_rough: Vec<u8>,
}

/// Каталог исходников: рядом с бинарником в бандле или в корне проекта.
pub fn source_dir() -> PathBuf {
    if let Ok(exe) = std::env::current_exe()
        && let Some(res) = exe
            .parent()
            .and_then(|p| p.parent())
            .map(|c| c.join("Resources/ambientcg"))
        && res.is_dir()
    {
        return res;
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/source/ambientcg")
}

/// Декодирует JPEG в `channels` каналов (1 — яркость, 3 — RGB).
fn decode(path: &Path, channels: usize) -> Result<Vec<u8>, String> {
    let data = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let cs = if channels == 1 {
        ColorSpace::Luma
    } else {
        ColorSpace::RGB
    };
    let mut dec = JpegDecoder::new_with_options(
        ZCursor::new(data),
        DecoderOptions::default().jpeg_set_out_colorspace(cs),
    );
    let pixels = dec
        .decode()
        .map_err(|e| format!("{}: {e:?}", path.display()))?;
    let info = dec
        .info()
        .ok_or_else(|| format!("{}: нет заголовка", path.display()))?;
    if (info.width as usize, info.height as usize) != (MATERIAL_SIZE, MATERIAL_SIZE) {
        return Err(format!(
            "{}: размер {}×{}, нужно {MATERIAL_SIZE}×{MATERIAL_SIZE}",
            path.display(),
            info.width,
            info.height
        ));
    }
    if pixels.len() != MATERIAL_SIZE * MATERIAL_SIZE * channels {
        return Err(format!("{}: неожиданное число каналов", path.display()));
    }
    Ok(pixels)
}

/// Слой после упаковки: (альбедо + высота, нормаль + шероховатость).
type PackedLayer = (Vec<u8>, Vec<u8>);

/// Читает все слои из `dir` и собирает массивы. Слои декодируются параллельно.
pub fn load(dir: &Path) -> Result<MaterialArrays, String> {
    let n = MATERIAL_SIZE * MATERIAL_SIZE;
    let layers: Vec<Result<PackedLayer, String>> = std::thread::scope(|scope| {
        let jobs: Vec<_> = LAYERS
            .iter()
            .map(|layer| {
                scope.spawn(move || {
                    let d = dir.join(layer.id);
                    let ctx = |e: String| format!("материал «{}»: {e}", layer.role);
                    let color = decode(&d.join("Color.jpg"), 3).map_err(ctx)?;
                    let normal = decode(&d.join("NormalGL.jpg"), 3).map_err(ctx)?;
                    let rough = decode(&d.join("Roughness.jpg"), 1).map_err(ctx)?;
                    let height = decode(&d.join("Displacement.jpg"), 1).map_err(ctx)?;
                    let mut ah = vec![0u8; n * 4];
                    let mut nr = vec![0u8; n * 4];
                    for i in 0..n {
                        ah[i * 4..i * 4 + 4].copy_from_slice(&[
                            color[i * 3],
                            color[i * 3 + 1],
                            color[i * 3 + 2],
                            height[i],
                        ]);
                        nr[i * 4..i * 4 + 4].copy_from_slice(&[
                            normal[i * 3],
                            normal[i * 3 + 1],
                            rough[i],
                            255,
                        ]);
                    }
                    Ok((ah, nr))
                })
            })
            .collect();
        jobs.into_iter()
            .map(|j| j.join().expect("поток декодирования"))
            .collect()
    });
    let mut albedo_height = Vec::with_capacity(LAYERS.len() * n * 4);
    let mut normal_rough = Vec::with_capacity(LAYERS.len() * n * 4);
    for layer in layers {
        let (ah, nr) = layer?;
        albedo_height.extend_from_slice(&ah);
        normal_rough.extend_from_slice(&nr);
    }
    Ok(MaterialArrays {
        size: MATERIAL_SIZE,
        layers: LAYERS.len(),
        albedo_height,
        normal_rough,
    })
}

/// Режим вида ландшафта (`FrameUniforms.view_mode`). Отладочные виды выводят
/// данные без освещения; `Raw` пишет числа для самопроверки.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum TerrainView {
    Lit = 0,
    Height = 1,
    Slope = 2,
    Biome = 3,
    Substrate = 4,
    CoastType = 5,
    Wetness = 6,
    Materials = 7,
    /// Вода: глубина спокойной воды.
    WaterDepth = 8,
    /// Вода: направление к берегу и вес береговой волны.
    WaterDirection = 9,
    /// Вода: пена, заплеск, береговая волна.
    Surf = 10,
    /// Кластеры рельефа разными цветами (02.3).
    Clusters = 11,
    /// Уровень иерархии кластера: синий — исходная сетка, красный — грубый.
    Levels = 12,
    /// HDR = (высота над камерой м, уклон градусы, индекс доминирующего биома, тип берега).
    Raw = 15,
}

impl TerrainView {
    const CYCLE: [TerrainView; 13] = [
        TerrainView::Lit,
        TerrainView::Height,
        TerrainView::Slope,
        TerrainView::Biome,
        TerrainView::Substrate,
        TerrainView::CoastType,
        TerrainView::Wetness,
        TerrainView::Materials,
        TerrainView::WaterDepth,
        TerrainView::WaterDirection,
        TerrainView::Surf,
        TerrainView::Clusters,
        TerrainView::Levels,
    ];

    /// Следующий вид по клавише V (`back` — Shift+V).
    pub fn next(self, back: bool) -> TerrainView {
        let i = Self::CYCLE.iter().position(|&v| v == self).unwrap_or(0);
        let n = Self::CYCLE.len();
        Self::CYCLE[if back { (i + n - 1) % n } else { (i + 1) % n }]
    }

    /// Имя для аргумента `--view`.
    pub fn parse(name: &str) -> Option<TerrainView> {
        Some(match name {
            "lit" => TerrainView::Lit,
            "height" => TerrainView::Height,
            "slope" => TerrainView::Slope,
            "biome" => TerrainView::Biome,
            "substrate" => TerrainView::Substrate,
            "coast" => TerrainView::CoastType,
            "wetness" => TerrainView::Wetness,
            "materials" => TerrainView::Materials,
            "water-depth" => TerrainView::WaterDepth,
            "water-dir" => TerrainView::WaterDirection,
            "surf" => TerrainView::Surf,
            "clusters" => TerrainView::Clusters,
            "levels" => TerrainView::Levels,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            TerrainView::Lit => "освещение",
            TerrainView::Height => "высота",
            TerrainView::Slope => "уклон",
            TerrainView::Biome => "веса биомов",
            TerrainView::Substrate => "субстрат",
            TerrainView::CoastType => "тип берега",
            TerrainView::Wetness => "влажность",
            TerrainView::Materials => "слои материалов",
            TerrainView::WaterDepth => "вода: глубина",
            TerrainView::WaterDirection => "вода: направление к берегу",
            TerrainView::Surf => "вода: пена и заплеск",
            TerrainView::Clusters => "кластеры",
            TerrainView::Levels => "уровни кластеров",
            TerrainView::Raw => "числа (самопроверка)",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layers_load_and_look_like_terrain_materials() {
        let m = load(&source_dir()).expect("материалы ambientCG");
        let n = m.size * m.size;
        assert_eq!(m.albedo_height.len(), m.layers * n * 4);
        for (l, layer) in LAYERS.iter().enumerate() {
            let a = &m.albedo_height[l * n * 4..(l + 1) * n * 4];
            let nr = &m.normal_rough[l * n * 4..(l + 1) * n * 4];
            // Нормали в среднем смотрят наружу: x и y около 0,5, плоская часть велика.
            let (mut nx, mut ny, mut rough, mut hmin, mut hmax) = (0u64, 0u64, 0u64, 255u8, 0u8);
            for p in 0..n {
                nx += nr[p * 4] as u64;
                ny += nr[p * 4 + 1] as u64;
                rough += nr[p * 4 + 2] as u64;
                hmin = hmin.min(a[p * 4 + 3]);
                hmax = hmax.max(a[p * 4 + 3]);
            }
            let (nx, ny) = (nx as f32 / n as f32 / 255.0, ny as f32 / n as f32 / 255.0);
            assert!(
                (nx - 0.5).abs() < 0.08 && (ny - 0.5).abs() < 0.08,
                "{}: средняя нормаль ({nx}, {ny})",
                layer.id
            );
            let rough = rough as f32 / n as f32 / 255.0;
            assert!(
                (0.2..=1.0).contains(&rough),
                "{}: шероховатость {rough}",
                layer.id
            );
            assert!(hmax > hmin + 40, "{}: высота без перепада", layer.id);
        }
    }
}
