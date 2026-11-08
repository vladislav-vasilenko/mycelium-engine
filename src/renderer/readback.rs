//! Readback: раскладка буфера копий и разбор результата на CPU.

use crate::layout::{LAYOUT_PROBE_LEN, WATER_PROBES};
use crate::reference::PATTERN_SIZE;

use super::resources::f16_to_f32;

// Раскладка буфера readback.
pub(super) const RB_PATTERN_BYTES: usize = (PATTERN_SIZE * PATTERN_SIZE * 4) as usize;
pub(super) const RB_DEPTH_OFFSET: usize = RB_PATTERN_BYTES;
pub(super) const RB_PROBE_OFFSET: usize = RB_DEPTH_OFFSET + 256;
/// Пиксели HDR-буфера (RGBA16Float, 8 байт) в тех же точках, что и глубина.
pub(super) const RB_HDR_OFFSET: usize = RB_PROBE_OFFSET + 256;
/// Результаты ядра `water_probe`: `WATER_PROBES` × float4.
pub(super) const RB_WATER_OFFSET: usize = RB_HDR_OFFSET + 256;
pub(super) const RB_BYTES: usize = RB_WATER_OFFSET + 256;

/// Точки проверки глубины в долях кадра для стартовой камеры самопроверки:
/// куб A, куб B (ближе к камере, перекрывает A) и фон.
pub const DEPTH_SAMPLES: [(f32, f32); 9] = [
    (0.42, 0.50),
    (0.45, 0.67),
    (0.46, 0.55),
    (0.52, 0.47),
    (0.55, 0.45),
    (0.58, 0.60),
    (0.05, 0.05),
    (0.95, 0.90),
    (0.50, 0.20),
];

/// Пиксели `DEPTH_SAMPLES` для цели `width × height`.
pub(super) fn sample_pixels(width: u32, height: u32) -> Vec<(u32, u32)> {
    DEPTH_SAMPLES
        .iter()
        .map(|(fx, fy)| {
            (
                ((width as f32 * fx) as u32).min(width - 1),
                ((height as f32 * fy) as u32).min(height - 1),
            )
        })
        .collect()
}

pub struct Readback {
    pub frame: u64,
    pub pattern_time: f32,
    pub width: u32,
    pub height: u32,
    pub pattern: Vec<u8>,
    /// (x, y, глубина) в пикселях целевого буфера.
    pub depth: Vec<(u32, u32, f32)>,
    /// Значения HDR-буфера (до тонмаппинга) в тех же точках.
    pub hdr: Vec<[f32; 4]>,
    /// Ядро `water_probe`: смещённая точка поверхности и пена в точках `SeaParams.probe`.
    pub water_probe: [[f32; 4]; WATER_PROBES],
    pub probe: [u32; LAYOUT_PROBE_LEN],
}

/// Readback в полёте: читать можно после сигнала `signal_value`.
pub(super) struct PendingReadback {
    pub signal_value: u64,
    pub frame: u64,
    pub pattern_time: f32,
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<(u32, u32)>,
}

impl PendingReadback {
    /// Разбирает байты буфера readback (`RB_BYTES`) в результат.
    pub fn decode(self, bytes: &[u8]) -> Readback {
        let f32_at = |o: usize| f32::from_le_bytes(bytes[o..o + 4].try_into().unwrap());
        let pattern = bytes[..RB_PATTERN_BYTES].to_vec();
        let depth = self
            .pixels
            .iter()
            .enumerate()
            .map(|(i, &(x, y))| (x, y, f32_at(RB_DEPTH_OFFSET + i * 4)))
            .collect();
        let hdr = (0..self.pixels.len())
            .map(|i| {
                let o = RB_HDR_OFFSET + i * 8;
                std::array::from_fn(|c| {
                    f16_to_f32(u16::from_le_bytes(
                        bytes[o + c * 2..o + c * 2 + 2].try_into().unwrap(),
                    ))
                })
            })
            .collect();
        let mut probe = [0u32; LAYOUT_PROBE_LEN];
        for (i, v) in probe.iter_mut().enumerate() {
            let o = RB_PROBE_OFFSET + i * 4;
            *v = u32::from_le_bytes(bytes[o..o + 4].try_into().unwrap());
        }
        let water_probe = std::array::from_fn(|i| {
            std::array::from_fn(|c| f32_at(RB_WATER_OFFSET + i * 16 + c * 4))
        });
        Readback {
            water_probe,
            frame: self.frame,
            pattern_time: self.pattern_time,
            width: self.width,
            height: self.height,
            pattern,
            depth,
            hdr,
            probe,
        }
    }
}
