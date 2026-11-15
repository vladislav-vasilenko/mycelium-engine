//! Интерфейс поверх кадра (05.1): текст шрифтом-SDF и панели. Приложение
//! собирает вершины каждый кадр (`UiBuilder`), рендерер рисует их последним
//! проходом поверх тонмаппинга.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::layout::UiVertex;

#[derive(Clone, Copy, Debug, Deserialize)]
struct GlyphJson {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    ox: f32,
    oy: f32,
    adv: f32,
}

#[derive(Deserialize)]
struct FontJson {
    size: f32,
    line: f32,
    atlas: f32,
    glyphs: HashMap<String, GlyphJson>,
}

/// Шрифт: метрики глифов атласа `assets/ui/font.png` (R8, поле расстояний).
pub struct Font {
    size: f32,
    line: f32,
    atlas: f32,
    glyphs: HashMap<char, GlyphJson>,
    /// Пиксели атласа (`atlas_size`²).
    pub pixels: Vec<u8>,
    pub atlas_size: u32,
}

pub fn assets_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/ui")
}

impl Font {
    pub fn load(dir: &Path) -> Result<Font, String> {
        let text = std::fs::read_to_string(dir.join("font.json"))
            .map_err(|e| format!("font.json: {e}"))?;
        let j: FontJson = serde_json::from_str(&text).map_err(|e| format!("font.json: {e}"))?;
        let file =
            std::fs::File::open(dir.join("font.png")).map_err(|e| format!("font.png: {e}"))?;
        let mut dec = png::Decoder::new(std::io::BufReader::new(file));
        dec.set_transformations(png::Transformations::EXPAND);
        let mut r = dec.read_info().map_err(|e| format!("font.png: {e}"))?;
        let mut buf = vec![0; r.output_buffer_size().unwrap_or(0)];
        let info = r
            .next_frame(&mut buf)
            .map_err(|e| format!("font.png: {e}"))?;
        if info.color_type != png::ColorType::Grayscale || info.width != j.atlas as u32 {
            return Err("font.png: нужен серый атлас".into());
        }
        buf.truncate(info.buffer_size());
        Ok(Font {
            size: j.size,
            line: j.line,
            atlas: j.atlas,
            glyphs: j
                .glyphs
                .into_iter()
                .filter_map(|(k, g)| k.chars().next().map(|c| (c, g)))
                .collect(),
            pixels: buf,
            atlas_size: info.width,
        })
    }

    /// Ширина строки в пикселях при высоте шрифта `px`.
    pub fn width(&self, text: &str, px: f32) -> f32 {
        let k = px / self.size;
        text.chars()
            .map(|c| self.glyphs.get(&c).map_or(self.size * 0.3, |g| g.adv) * k)
            .sum()
    }
}

/// Сборщик вершин интерфейса кадра; координаты — пиксели окна, (0, 0) — верх слева.
pub struct UiBuilder<'a> {
    pub font: &'a Font,
    pub verts: Vec<UiVertex>,
    /// Масштаб интерфейса (пиксели на точку экрана Retina).
    pub scale: f32,
}

fn quad(out: &mut Vec<UiVertex>, x0: f32, y0: f32, x1: f32, y1: f32, uv: [f32; 4], c: [u8; 4]) {
    let v = |x, y, u, w| UiVertex {
        pos: [x, y],
        uv: [u, w],
        color: c,
    };
    out.extend_from_slice(&[
        v(x0, y0, uv[0], uv[1]),
        v(x1, y0, uv[2], uv[1]),
        v(x1, y1, uv[2], uv[3]),
        v(x0, y0, uv[0], uv[1]),
        v(x1, y1, uv[2], uv[3]),
        v(x0, y1, uv[0], uv[3]),
    ]);
}

impl<'a> UiBuilder<'a> {
    pub fn new(font: &'a Font, scale: f32) -> Self {
        UiBuilder {
            font,
            verts: Vec::new(),
            scale,
        }
    }

    /// Панель: прямоугольник цвета `c` (sRGB, A — непрозрачность).
    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, c: [u8; 4]) {
        let s = self.scale;
        quad(
            &mut self.verts,
            x * s,
            y * s,
            (x + w) * s,
            (y + h) * s,
            [-1.0; 4],
            c,
        );
    }

    /// Текст высотой `px` точек с верхним левым углом в (x, y); тень снизу справа.
    /// Возвращает ширину строки.
    pub fn text(&mut self, x: f32, y: f32, px: f32, c: [u8; 4], text: &str) -> f32 {
        let shadow = [0, 0, 0, (c[3] as f32 * 0.6) as u8];
        self.glyphs(x + px * 0.06, y + px * 0.06, px, shadow, text);
        self.glyphs(x, y, px, c, text)
    }

    fn glyphs(&mut self, x: f32, y: f32, px: f32, c: [u8; 4], text: &str) -> f32 {
        let f = self.font;
        let k = px / f.size * self.scale;
        let mut pen = x * self.scale;
        let top = y * self.scale;
        for ch in text.chars() {
            let Some(g) = f.glyphs.get(&ch) else {
                pen += f.size * 0.3 * k;
                continue;
            };
            if ch != ' ' {
                let a = f.atlas;
                let x0 = pen + g.ox * k;
                let y0 = top + g.oy * k;
                quad(
                    &mut self.verts,
                    x0,
                    y0,
                    x0 + g.w * k,
                    y0 + g.h * k,
                    [g.x / a, g.y / a, (g.x + g.w) / a, (g.y + g.h) / a],
                    c,
                );
            }
            pen += g.adv * k;
        }
        pen / self.scale - x
    }

    /// Высота строки при шрифте `px`.
    pub fn line(&self, px: f32) -> f32 {
        px * self.font.line / self.font.size
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn font_has_cyrillic_and_measures() {
        let f = super::Font::load(&super::assets_dir()).unwrap();
        assert!(f.width("Журнал", 20.0) > f.width("Жур", 20.0));
        let mut b = super::UiBuilder::new(&f, 2.0);
        b.text(10.0, 10.0, 20.0, [255; 4], "Гриб");
        // 4 буквы × 2 (тень) × 6 вершин.
        assert_eq!(b.verts.len(), 4 * 2 * 6);
    }
}
