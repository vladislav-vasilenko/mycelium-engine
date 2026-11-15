//! `vegetation.png` + `vegetation.json` — сообщества карты растительности
//! 1 : 25 000 (02.7, AD-61): номер сообщества на узел сетки поясов и параметры
//! леса сообщества для расстановки на GPU (02.4).

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::read_png;
use crate::world::VegetationLayer;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VegetationMeta {
    pub file: String,
    pub legend: String,
    pub size: u32,
}

#[derive(Deserialize)]
struct Legend {
    legend: Vec<Entry>,
}

#[derive(Deserialize)]
struct Entry {
    id: u32,
    /// Густота, доля криптомерии, масштаб от и до, частота валунов.
    forest: [f32; 5],
}

pub fn import(dir: &Path, meta: &VegetationMeta) -> Result<VegetationLayer, String> {
    let img = read_png(&dir.join(&meta.file))?;
    let n = meta.size as usize;
    if (img.width, img.height) != (n, n) || img.channels != 1 || img.max != 255 {
        return Err(format!("{}: нужен серый 8 бит {n}²", meta.file));
    }
    let text = std::fs::read_to_string(dir.join(&meta.legend))
        .map_err(|e| format!("{}: {e}", meta.legend))?;
    let legend: Legend =
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", meta.legend))?;
    let mut forest = vec![[0.0f32; 8]; 256];
    for e in legend.legend {
        let f = e.forest;
        forest[(e.id as usize).min(255)] = [f[0], f[1], f[2], f[3], f[4], 0.0, 0.0, 0.0];
    }
    Ok(VegetationLayer {
        size: meta.size,
        ids: img.data.iter().map(|&v| v as u8).collect(),
        forest,
    })
}
