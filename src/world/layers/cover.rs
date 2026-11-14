//! `cover.png` — пояса растительности (02.7, шаг 4): RGBA 8 бит, сумма на
//! суше 255. R — низинный лес, G — кедровый лес якусуги, B — якудзаса и
//! луга, A — гранит и застройка. Квадрат на всю карту, узел — тексель.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::{read_png, write_png};
use crate::world::CoverLayer;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CoverMeta {
    pub file: String,
    pub size: u32,
    #[serde(default)]
    pub channels: String,
}

pub fn import(dir: &Path, meta: &CoverMeta) -> Result<CoverLayer, String> {
    let img = read_png(&dir.join(&meta.file))?;
    let n = meta.size as usize;
    if (img.width, img.height) != (n, n) || img.channels != 4 || img.max != 255 {
        return Err(format!(
            "{}: нужно RGBA 8 бит {n}², а не {}×{} × {} каналов",
            meta.file, img.width, img.height, img.channels
        ));
    }
    Ok(CoverLayer {
        size: meta.size,
        rgba: img.data.iter().map(|&v| v as u8).collect(),
    })
}

/// Переносит слой при экспорте без изменений: он не выводится из полей сетки.
pub fn export(dir: &Path, cover: &Option<CoverLayer>) -> Result<Option<CoverMeta>, String> {
    export_as(
        dir,
        cover,
        "cover.png",
        "R низинный лес, G кедровый лес якусуги, B якудзаса и луга, A гранит и застройка",
    )
}

/// Любой слой RGBA8 на всю карту (пояса, `macro.png` 02.9) — без изменений.
pub fn export_as(
    dir: &Path,
    layer: &Option<CoverLayer>,
    file: &str,
    channels: &str,
) -> Result<Option<CoverMeta>, String> {
    let Some(c) = layer else { return Ok(None) };
    let n = c.size as usize;
    write_png(
        &dir.join(file),
        (n, n),
        png::ColorType::Rgba,
        png::BitDepth::Eight,
        &c.rgba,
    )?;
    Ok(Some(CoverMeta {
        file: file.into(),
        size: c.size,
        channels: channels.into(),
    }))
}
