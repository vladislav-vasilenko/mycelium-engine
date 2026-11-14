//! `height_fine.png` — поле высот полного разрешения (02.3), если оно мельче
//! сетки слоёв (Якусима: 7,5 м при слоях 30 м). Квантование — тем же
//! `height_range_m`, что `height.png`.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::{read_png, write_png};
use crate::world::heightfield::HeightField;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HeightFineMeta {
    pub file: String,
    pub size: usize,
    pub step_m: f32,
}

/// Читает поле и проверяет, что оно покрывает ту же карту, что и слои.
pub fn import(
    dir: &Path,
    meta: &HeightFineMeta,
    range: [f32; 2],
    map_size_m: f32,
) -> Result<HeightField, String> {
    let img = read_png(&dir.join(&meta.file))?;
    let n = meta.size;
    if (img.width, img.height) != (n, n) || img.max != u16::MAX {
        return Err(format!(
            "{}: нужно 16 бит {n}², а не {}×{}",
            meta.file, img.width, img.height
        ));
    }
    let covered = (n - 1) as f32 * meta.step_m;
    if (covered - map_size_m).abs() > 0.01 {
        return Err(format!(
            "{}: поле покрывает {covered} м, карта — {map_size_m} м",
            meta.file
        ));
    }
    let k = (range[1] - range[0]) / u16::MAX as f32;
    let heights = (0..n * n)
        .map(|i| range[0] + img.data[i * img.channels] as f32 * k)
        .collect();
    Ok(HeightField::new(n, n, meta.step_m, heights))
}

/// Пишет поле, если оно мельче сетки слоев; иначе `None` (поле = сетка мира).
pub fn export(
    dir: &Path,
    hf: &HeightField,
    grid_nx: usize,
    range: [f32; 2],
) -> Result<Option<HeightFineMeta>, String> {
    if hf.nx == grid_nx {
        return Ok(None);
    }
    let k = u16::MAX as f32 / (range[1] - range[0]);
    let mut bytes = Vec::with_capacity(hf.heights.len() * 2);
    for &h in &hf.heights {
        let q = ((h - range[0]) * k).round().clamp(0.0, u16::MAX as f32) as u16;
        bytes.extend_from_slice(&q.to_be_bytes());
    }
    write_png(
        &dir.join("height_fine.png"),
        (hf.nx, hf.nz),
        png::ColorType::Grayscale,
        png::BitDepth::Sixteen,
        &bytes,
    )?;
    Ok(Some(HeightFineMeta {
        file: "height_fine.png".into(),
        size: hf.nx,
        step_m: hf.step_m,
    }))
}

/// `height_zone.png` — игровая зона (02.7, шаг 5): клетки острова
/// `[origin_cell, origin_cell + cells)`, каждая делится на `ratio`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HeightZoneMeta {
    pub file: String,
    pub origin_cell: [usize; 2],
    pub cells: usize,
    pub ratio: usize,
    pub height_range_m: [f32; 2],
    #[serde(default)]
    pub source: String,
}

/// Читает зону и выравнивает высоты совпадающих узлов острова по зоне —
/// так стык зоны и острова совпадает до бита.
pub fn import_zone(
    dir: &Path,
    meta: &HeightZoneMeta,
    far: &mut HeightField,
) -> Result<crate::world::zone::ZoneField, String> {
    let img = read_png(&dir.join(&meta.file))?;
    let n = meta.cells * meta.ratio + 1;
    if (img.width, img.height) != (n, n) || img.max != u16::MAX {
        return Err(format!(
            "{}: нужно 16 бит {n}², а не {}×{}",
            meta.file, img.width, img.height
        ));
    }
    let [i0, j0] = meta.origin_cell;
    if i0 + meta.cells >= far.nx || j0 + meta.cells >= far.nz || i0 == 0 || j0 == 0 {
        return Err(format!("{}: зона выходит за поле острова", meta.file));
    }
    let [lo, hi] = meta.height_range_m;
    let k = (hi - lo) / u16::MAX as f32;
    let heights: Vec<f32> = (0..n * n)
        .map(|i| lo + img.data[i * img.channels] as f32 * k)
        .collect();
    let r = meta.ratio;
    for b in (0..n).step_by(r) {
        for a in (0..n).step_by(r) {
            far.heights[(j0 + b / r) * far.nx + i0 + a / r] = heights[b * n + a];
        }
    }
    Ok(crate::world::zone::ZoneField {
        i0,
        j0,
        cells: meta.cells,
        ratio: r,
        grid: HeightField::new(n, n, far.step_m / r as f32, heights),
    })
}
