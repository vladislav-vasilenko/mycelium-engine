//! Слои карты: экспорт мира в PNG и сборка мира из PNG.
//!
//! Каталог слоёв содержит `world.json` и изображения размером `nx × nz`
//! (1025 × 1025 для карты 16 × 16 тайлов, с общими краями). Шаг сетки —
//! `sample_step_m` (1 м у генератора, 30 м у дальнего рельефа Якусимы).
//! Пиксель (i, j) — точка мировой сетки x = i · шаг, z = j · шаг; z растёт вниз.
//!
//! | Файл | Формат | Роль | Каналы |
//! | --- | --- | --- | --- |
//! | `height.png` | серый 16 бит | источник | высота в `height_range_m` |
//! | `masks.png` | RGB 8 бит | источник | тропа, русло, чаша торфяника |
//! | `climate.png` | RGB 8 бит | источник | влажность 0..1, температура в `temperature_range_c`, близость берега моря 0..1 |
//! | `coast_type.png` | серый 8 бит | источник | тип берега: 0 песок, 255 камень |
//! | `coast_distance.png` | серый 16 бит | источник | знаковое расстояние до кромки в `coast_distance_range_m` (> 0 — суша) |
//! | `biome.png` | RGB 8 бит | замена по `overrides` | лес, торф, берег; сумма каналов 255 |
//! | `substrate.png` | серый 8 бит | замена по `overrides` | индекс субстрата × 60 |
//! | `preview.png` | RGB 8 бит | только просмотр | цветная карта с маркерами |
//!
//! Источники определяют мир. Веса и субстрат пересчитываются из них, если
//! имя слоя не указано в `overrides` файла `world.json`.

use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::path::Path;

use glam::{Vec2, Vec3};
use serde::{Deserialize, Serialize};

use super::generator::{Fields, Generator};
use super::{
    BiomeWeights, DerivedOverrides, FieldGrid, GENERATOR_VERSION, Ground, Marker, MarkerKind,
    Substrate, TILE_SAMPLES, World,
};

/// Версия формата каталога слоёв.
pub const FORMAT_VERSION: u32 = 3;
/// Диапазон расстояния до кромки в `coast_distance.png`, м: шаг ≈ 12 см.
pub const COAST_RANGE_M: [f32; 2] = [-4000.0, 4000.0];
/// Диапазон высот подбирается под данные с запасом и округлением до 10 м,
/// поэтому повторный экспорт импортированного мира даёт тот же диапазон.
fn height_range(world: &World) -> [f32; 2] {
    let (lo, hi) = world.fields.iter().fold((f32::MAX, f32::MIN), |(a, b), f| {
        (a.min(f.height), b.max(f.height))
    });
    [
        (lo / 10.0).floor() * 10.0 - 10.0,
        (hi / 10.0).ceil() * 10.0 + 10.0,
    ]
}
pub const TEMPERATURE_RANGE_C: [f32; 2] = [-10.0, 40.0];
/// Шаг кода субстрата в `substrate.png`: 0, 60, 120, 180, 240.
pub const SUBSTRATE_STEP: u8 = 60;

pub const MANIFEST: &str = "world.json";

/// Описание каталога слоёв.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub format: u32,
    pub seed: u64,
    pub generator_version: u32,
    pub tiles_x: i32,
    pub tiles_z: i32,
    pub sample_step_m: f32,
    /// Размер слоя в точках: [nx, nz].
    pub size_samples: [usize; 2],
    pub height_range_m: [f32; 2],
    pub temperature_range_c: [f32; 2],
    /// Порядок кодов `substrate.png`: индекс × 60.
    pub substrate_codes: Vec<String>,
    /// Производные слои, которые при импорте заменяют вычисленные: `biome`, `substrate`.
    pub overrides: Vec<String>,
    /// Контрольная сумма мира, из которого сделан экспорт (для справки).
    pub exported_checksum: String,
    pub markers: Vec<MarkerEntry>,
    pub coast_distance_range_m: [f32; 2],
    /// Привязка к Земле для мира из реальных данных (02.7).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geo: Option<GeoRef>,
    /// Поле высот мельче сетки слоёв (02.3), необязательное.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height_fine: Option<fine::HeightFineMeta>,
    /// Слой объектов уже шага сетки (02.7), необязательный.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub features: Option<super::features::FeaturesMeta>,
}

/// Привязка мира к географическим координатам.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GeoRef {
    /// Центр локальной проекции (WGS84), градусы.
    pub center_lat: f64,
    pub center_lon: f64,
    /// Мировые координаты центра проекции, м. Мировая точка (0, 0) — угол карты.
    pub center_world_m: [f32; 2],
    /// Описание проекции: «local tangent plane: x — восток, z — юг».
    pub projection: String,
    /// Источники данных с указанием авторства.
    pub sources: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MarkerEntry {
    pub name: String,
    pub kind: MarkerKind,
    /// Мировые координаты, м. Высота при импорте берётся с грунта.
    pub x: f32,
    pub z: f32,
}

const SUBSTRATES: [Substrate; 5] = [
    Substrate::Soil,
    Substrate::Peat,
    Substrate::Sand,
    Substrate::Rock,
    Substrate::Gravel,
];

fn substrate_index(s: Substrate) -> u8 {
    s as u8
}

fn quantize(v: f32, [lo, hi]: [f32; 2], max: f32) -> u16 {
    ((v - lo) / (hi - lo) * max).round().clamp(0.0, max) as u16
}

fn dequantize(q: u16, [lo, hi]: [f32; 2], max: f32) -> f32 {
    lo + q as f32 / max * (hi - lo)
}

fn unit8(v: f32) -> u8 {
    quantize(v, [0.0, 1.0], 255.0) as u8
}

/// Веса в три байта с суммой ровно 255 (метод наибольших остатков), чтобы
/// повторный экспорт импортированных весов давал те же байты.
fn encode_weights(w: &BiomeWeights) -> [u8; 3] {
    let raw = [w.forest, w.peat, w.shore].map(|v| v.max(0.0));
    let sum: f32 = raw.iter().sum::<f32>().max(1e-6);
    let scaled = raw.map(|v| v / sum * 255.0);
    let mut out = scaled.map(|v| v.floor() as u16);
    let mut rest = 255 - out.iter().sum::<u16>();
    let mut order = [0usize, 1, 2];
    order.sort_by(|&a, &b| {
        (scaled[b] - scaled[b].floor())
            .total_cmp(&(scaled[a] - scaled[a].floor()))
            .then(a.cmp(&b))
    });
    for &i in order.iter().cycle() {
        if rest == 0 {
            break;
        }
        out[i] += 1;
        rest -= 1;
    }
    out.map(|v| v as u8)
}

fn decode_weights(c: [u16; 3]) -> BiomeWeights {
    let sum = (c[0] + c[1] + c[2]) as f32;
    if sum == 0.0 {
        return BiomeWeights {
            forest: 1.0,
            peat: 0.0,
            shore: 0.0,
        };
    }
    BiomeWeights {
        forest: c[0] as f32 / sum,
        peat: c[1] as f32 / sum,
        shore: c[2] as f32 / sum,
    }
}

/// Изображение после декодирования: значения каналов как u16, `max` — 255 или 65535.
struct Image {
    width: usize,
    height: usize,
    channels: usize,
    max: u16,
    data: Vec<u16>,
}

impl Image {
    fn get(&self, i: usize, j: usize, c: usize) -> u16 {
        self.data[(j * self.width + i) * self.channels + c.min(self.channels - 1)]
    }
}

fn write_png(
    path: &Path,
    (w, h): (usize, usize),
    color: png::ColorType,
    depth: png::BitDepth,
    data: &[u8],
) -> Result<(), String> {
    let file = File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut enc = png::Encoder::new(BufWriter::new(file), w as u32, h as u32);
    enc.set_color(color);
    enc.set_depth(depth);
    let mut writer = enc
        .write_header()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    writer
        .write_image_data(data)
        .map_err(|e| format!("{}: {e}", path.display()))
}

fn read_png(path: &Path) -> Result<Image, String> {
    let err = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
    let file = File::open(path).map_err(|e| err(&e))?;
    let mut dec = png::Decoder::new(BufReader::new(file));
    // Палитру разворачиваем в RGB, разрядность оставляем как есть.
    dec.set_transformations(png::Transformations::EXPAND);
    let mut reader = dec.read_info().map_err(|e| err(&e))?;
    let size = reader
        .output_buffer_size()
        .ok_or_else(|| err(&"слишком большое изображение"))?;
    let mut buf = vec![0; size];
    let info = reader.next_frame(&mut buf).map_err(|e| err(&e))?;
    buf.truncate(info.buffer_size());
    let (color, depth) = reader.output_color_type();
    let channels = color.samples();
    let (max, data) = match depth {
        png::BitDepth::Sixteen => (
            u16::MAX,
            buf.as_chunks::<2>()
                .0
                .iter()
                .map(|&b| u16::from_be_bytes(b))
                .collect(),
        ),
        png::BitDepth::Eight => (255, buf.iter().map(|&b| b as u16).collect()),
        other => {
            return Err(err(&format!(
                "разрядность {other:?} не поддерживается, нужна 8 или 16 бит"
            )));
        }
    };
    Ok(Image {
        width: info.width as usize,
        height: info.height as usize,
        channels,
        max,
        data,
    })
}

/// Записывает слои мира в каталог `dir` (создаёт его).
pub fn export(world: &World, dir: &Path) -> Result<Manifest, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let nx = world.tiles_x as usize * (TILE_SAMPLES - 1) + 1;
    let nz = world.tiles_z as usize * (TILE_SAMPLES - 1) + 1;
    let size = (nx, nz);
    // Точка мировой сетки → тайл, которому она принадлежит (на стыке — любой из равных).
    let sample = |i: usize, j: usize| {
        let tx = (i / (TILE_SAMPLES - 1)).min(world.tiles_x as usize - 1);
        let tz = (j / (TILE_SAMPLES - 1)).min(world.tiles_z as usize - 1);
        let tile = &world.tiles[tz * world.tiles_x as usize + tx];
        tile.at(i - tx * (TILE_SAMPLES - 1), j - tz * (TILE_SAMPLES - 1))
    };
    let grid = &world.fields;

    let mut height = Vec::with_capacity(nx * nz * 2);
    let mut masks = Vec::with_capacity(nx * nz * 3);
    let mut climate = Vec::with_capacity(nx * nz * 3);
    let mut biome = Vec::with_capacity(nx * nz * 3);
    let mut substrate = Vec::with_capacity(nx * nz);
    let mut coast_type = Vec::with_capacity(nx * nz);
    let mut coast_distance = Vec::with_capacity(nx * nz * 2);
    let h_range = height_range(world);
    for j in 0..nz {
        for i in 0..nx {
            let s = sample(i, j);
            let f = &grid[j * nx + i];
            height.extend_from_slice(&quantize(s.height, h_range, 65535.0).to_be_bytes());
            coast_distance
                .extend_from_slice(&quantize(f.coast_d, COAST_RANGE_M, 65535.0).to_be_bytes());
            masks.extend_from_slice(&[unit8(f.trail), unit8(f.river), unit8(f.basin)]);
            climate.extend_from_slice(&[
                unit8(s.moisture),
                quantize(s.temperature, TEMPERATURE_RANGE_C, 255.0) as u8,
                unit8(f.coast),
            ]);
            biome.extend_from_slice(&encode_weights(&s.weights));
            substrate.push(substrate_index(s.substrate) * SUBSTRATE_STEP);
            coast_type.push(unit8(f.rocky));
        }
    }
    use png::{BitDepth::*, ColorType::*};
    write_png(&dir.join("height.png"), size, Grayscale, Sixteen, &height)?;
    write_png(&dir.join("masks.png"), size, Rgb, Eight, &masks)?;
    write_png(&dir.join("climate.png"), size, Rgb, Eight, &climate)?;
    write_png(
        &dir.join("coast_type.png"),
        size,
        Grayscale,
        Eight,
        &coast_type,
    )?;
    write_png(
        &dir.join("coast_distance.png"),
        size,
        Grayscale,
        Sixteen,
        &coast_distance,
    )?;
    write_png(&dir.join("biome.png"), size, Rgb, Eight, &biome)?;
    write_png(
        &dir.join("substrate.png"),
        size,
        Grayscale,
        Eight,
        &substrate,
    )?;
    write_png(
        &dir.join("preview.png"),
        size,
        Rgb,
        Eight,
        &preview(world, size),
    )?;

    // Объекты карты переносятся как есть: они не выводятся из полей сетки.
    let features = match &world.features {
        Some(f) => {
            std::fs::write(dir.join("features.json"), &f.json).map_err(|e| e.to_string())?;
            Some(super::features::FeaturesMeta {
                file: "features.json".into(),
            })
        }
        None => None,
    };
    let manifest = Manifest {
        format: FORMAT_VERSION,
        seed: world.seed,
        generator_version: GENERATOR_VERSION,
        tiles_x: world.tiles_x,
        tiles_z: world.tiles_z,
        sample_step_m: world.step_m,
        size_samples: [nx, nz],
        height_range_m: h_range,
        temperature_range_c: TEMPERATURE_RANGE_C,
        substrate_codes: SUBSTRATES.iter().map(|s| format!("{s:?}")).collect(),
        overrides: Vec::new(),
        exported_checksum: format!("{:016x}", world.checksum()),
        markers: world
            .markers
            .iter()
            .map(|m| MarkerEntry {
                name: m.name.clone(),
                kind: m.kind,
                x: m.position.x,
                z: m.position.z,
            })
            .collect(),
        coast_distance_range_m: COAST_RANGE_M,
        geo: world.geo.clone(),
        height_fine: fine::export(dir, &world.heightfield, nx, h_range)?,
        features,
    };
    let json = serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(MANIFEST), json + "\n").map_err(|e| e.to_string())?;
    Ok(manifest)
}

/// Собирает мир из каталога слоёв.
pub fn import(dir: &Path) -> Result<World, String> {
    let text = std::fs::read_to_string(dir.join(MANIFEST))
        .map_err(|e| format!("{}: {e}", dir.join(MANIFEST).display()))?;
    let m: Manifest = serde_json::from_str(&text).map_err(|e| format!("{MANIFEST}: {e}"))?;
    if m.format != FORMAT_VERSION {
        return Err(format!(
            "{MANIFEST}: формат {} не поддерживается, ожидается {FORMAT_VERSION}",
            m.format
        ));
    }
    let nx = m.tiles_x as usize * (TILE_SAMPLES - 1) + 1;
    let nz = m.tiles_z as usize * (TILE_SAMPLES - 1) + 1;
    if m.size_samples != [nx, nz] {
        return Err(format!(
            "{MANIFEST}: size_samples {:?}, для {}×{} тайлов нужно [{nx}, {nz}]",
            m.size_samples, m.tiles_x, m.tiles_z
        ));
    }
    let load = |name: &str, min_channels: usize| -> Result<Image, String> {
        let img = read_png(&dir.join(name))?;
        if (img.width, img.height) != (nx, nz) {
            return Err(format!(
                "{name}: размер {}×{}, нужно {nx}×{nz}",
                img.width, img.height
            ));
        }
        if img.channels < min_channels {
            return Err(format!(
                "{name}: каналов {}, нужно не меньше {min_channels}",
                img.channels
            ));
        }
        Ok(img)
    };

    let height = load("height.png", 1)?;
    if height.max != u16::MAX {
        return Err(
            "height.png: нужна разрядность 16 бит; 8 бит дают шаг высоты ~30 см и террасы на склонах"
                .into(),
        );
    }
    let masks = load("masks.png", 3)?;
    let climate = load("climate.png", 3)?;
    let coast_type = load("coast_type.png", 1)?;
    let coast_distance = load("coast_distance.png", 1)?;
    if coast_distance.max != u16::MAX {
        return Err("coast_distance.png: нужна разрядность 16 бит".into());
    }
    if m.sample_step_m.is_nan() || m.sample_step_m <= 0.0 {
        return Err(format!("{MANIFEST}: sample_step_m должен быть больше нуля"));
    }
    let unit = |img: &Image, i, j, c| img.get(i, j, c) as f32 / img.max as f32;

    let mut fields = Vec::with_capacity(nx * nz);
    for j in 0..nz {
        for i in 0..nx {
            fields.push(Fields {
                height: dequantize(height.get(i, j, 0), m.height_range_m, 65535.0),
                moisture: unit(&climate, i, j, 0),
                temperature: dequantize(
                    (unit(&climate, i, j, 1) * 255.0).round() as u16,
                    m.temperature_range_c,
                    255.0,
                ),
                trail: unit(&masks, i, j, 0),
                river: unit(&masks, i, j, 1),
                basin: unit(&masks, i, j, 2),
                coast: unit(&climate, i, j, 2),
                rocky: unit(&coast_type, i, j, 0),
                coast_d: dequantize(
                    coast_distance.get(i, j, 0),
                    m.coast_distance_range_m,
                    65535.0,
                ),
            });
        }
    }

    let mut overrides = DerivedOverrides::default();
    for name in &m.overrides {
        match name.as_str() {
            "biome" => {
                let img = load("biome.png", 3)?;
                let to8 = |v: u16| (v as u32 * 255 / img.max as u32) as u16;
                overrides.weights = Some(
                    (0..nx * nz)
                        .map(|k| {
                            let (i, j) = (k % nx, k / nx);
                            decode_weights([0, 1, 2].map(|c| to8(img.get(i, j, c))))
                        })
                        .collect(),
                );
            }
            "substrate" => {
                let img = load("substrate.png", 1)?;
                let mut out = Vec::with_capacity(nx * nz);
                for k in 0..nx * nz {
                    let v = img.get(k % nx, k / nx, 0) as f32 * 255.0 / img.max as f32;
                    let idx = (v / SUBSTRATE_STEP as f32).round() as usize;
                    out.push(*SUBSTRATES.get(idx).ok_or_else(|| {
                        format!(
                            "substrate.png: значение {v} в ({}, {}) не код субстрата",
                            k % nx,
                            k / nx
                        )
                    })?);
                }
                overrides.substrate = Some(out);
            }
            other => {
                return Err(format!(
                    "{MANIFEST}: неизвестный слой в overrides: {other} (допустимы biome, substrate)"
                ));
            }
        }
    }

    let generator = Generator::first_map(m.seed);
    let markers = m
        .markers
        .iter()
        .map(|e| Marker {
            name: e.name.clone(),
            kind: e.kind,
            position: Vec3::new(e.x, 0.0, e.z),
        })
        .collect();
    let grid = FieldGrid {
        nx,
        nz,
        step_m: m.sample_step_m,
        fields,
    };
    let mut world = World::from_grid(m.seed, generator, grid, &overrides, Some(markers));
    world.geo = m.geo;
    if let Some(meta) = &m.features {
        world.features = Some(import_features(dir, meta, world.size_m().x)?);
    }
    if let Some(meta) = &m.height_fine {
        world.heightfield = fine::import(dir, meta, m.height_range_m, world.size_m().x)?;
    }
    Ok(world)
}

/// `features.json`: векторы троп, рек и дорог (02.7).
fn import_features(
    dir: &Path,
    meta: &super::features::FeaturesMeta,
    map_size_m: f32,
) -> Result<super::features::FeatureSet, String> {
    let path = dir.join(&meta.file);
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    super::features::FeatureSet::from_json(&text, map_size_m)
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// Цветная карта сверху: вода, тропа, субстрат, отмывка рельефа, маркеры.
pub fn preview(world: &World, (nx, nz): (usize, usize)) -> Vec<u8> {
    let light = Vec3::new(-0.5, 0.8, -0.3).normalize();
    let mut rgb = Vec::with_capacity(nx * nz * 3);
    for z in 0..nz {
        for x in 0..nx {
            let p = Vec2::new(x as f32, z as f32) * world.step_m;
            let Some(s) = world.sample_ground(p.x.min(world.size_m().x), p.y.min(world.size_m().y))
            else {
                rgb.extend_from_slice(&[0, 0, 0]);
                continue;
            };
            let shade = (0.55 + 0.45 * s.normal.dot(light)).clamp(0.2, 1.0);
            let base: [f32; 3] = if s.height < 0.0 {
                let d = (-s.height / 7.0).clamp(0.0, 1.0);
                [0.1, 0.35 - 0.2 * d, 0.6 - 0.25 * d]
            } else if s.wet {
                [0.2, 0.45, 0.75]
            } else if s.on_trail {
                [0.85, 0.7, 0.45]
            } else {
                match s.substrate {
                    Substrate::Soil => [0.25, 0.42, 0.18],
                    Substrate::Peat => [0.38, 0.3, 0.18],
                    Substrate::Sand => [0.85, 0.8, 0.6],
                    Substrate::Rock => [0.55, 0.55, 0.55],
                    Substrate::Gravel => [0.6, 0.55, 0.45],
                }
            };
            let hl = (s.height / 40.0).clamp(0.0, 1.0) * 0.25;
            for c in base {
                rgb.push(((c + hl) * shade * 255.0).clamp(0.0, 255.0) as u8);
            }
        }
    }
    for m in &world.markers {
        for dz in -4i32..=4 {
            for dx in -4i32..=4 {
                let (x, z) = (m.position.x as i32 + dx, m.position.z as i32 + dz);
                if (0..nx as i32).contains(&x) && (0..nz as i32).contains(&z) {
                    let i = (z as usize * nx + x as usize) * 3;
                    rgb[i..i + 3].copy_from_slice(&[230, 40, 40]);
                }
            }
        }
    }
    rgb
}

mod fine;
#[cfg(test)]
mod tests;
