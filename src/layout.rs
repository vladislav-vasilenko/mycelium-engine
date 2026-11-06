//! Раскладка данных, общих для CPU и GPU.
//!
//! Каждая структура повторена в `shaders/metal/common.h`. Размеры и смещения
//! проверяются на CPU (тесты ниже) и на GPU (ядро `layout_probe`, режим `--selftest`).

use bytemuck::{Pod, Zeroable};
use core::mem::{align_of, offset_of, size_of};

/// Параметры кадра. Матрица хранится по столбцам, как `float4x4` в MSL.
#[repr(C, align(16))]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct FrameUniforms {
    pub view_proj: [[f32; 4]; 4],
    pub camera_pos: [f32; 4],
    /// Направление от источника к сцене, нормированное.
    pub light_dir: [f32; 4],
    pub time: f32,
    pub exposure: f32,
    pub pattern_time: f32,
    pub frame_index: u32,
}

#[repr(C, align(16))]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct InstanceData {
    pub model: [[f32; 4]; 4],
    pub tint: [f32; 4],
}

/// Вершина: `packed_float3` позиции и нормали плюс `float2` UV, 32 байта.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
}

/// Ожидаемые значения, которые ядро `layout_probe` записывает в буфер.
/// Порядок полей совпадает с порядком записи в MSL.
pub fn expected_probe() -> [u32; LAYOUT_PROBE_LEN] {
    [
        size_of::<FrameUniforms>() as u32,
        align_of::<FrameUniforms>() as u32,
        offset_of!(FrameUniforms, camera_pos) as u32,
        offset_of!(FrameUniforms, light_dir) as u32,
        offset_of!(FrameUniforms, time) as u32,
        offset_of!(FrameUniforms, exposure) as u32,
        offset_of!(FrameUniforms, pattern_time) as u32,
        offset_of!(FrameUniforms, frame_index) as u32,
        size_of::<InstanceData>() as u32,
        offset_of!(InstanceData, tint) as u32,
        size_of::<Vertex>() as u32,
        offset_of!(Vertex, normal) as u32,
        offset_of!(Vertex, uv) as u32,
    ]
}

pub const LAYOUT_PROBE_LEN: usize = 13;

pub const LAYOUT_PROBE_NAMES: [&str; LAYOUT_PROBE_LEN] = [
    "sizeof(FrameUniforms)",
    "alignof(FrameUniforms)",
    "FrameUniforms.camera_pos",
    "FrameUniforms.light_dir",
    "FrameUniforms.time",
    "FrameUniforms.exposure",
    "FrameUniforms.pattern_time",
    "FrameUniforms.frame_index",
    "sizeof(InstanceData)",
    "InstanceData.tint",
    "sizeof(Vertex)",
    "Vertex.normal",
    "Vertex.uv",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_layout_matches_msl_rules() {
        // float4x4 = 64 байта, float4 выравнивается по 16.
        assert_eq!(
            expected_probe(),
            [112, 16, 64, 80, 96, 100, 104, 108, 80, 64, 32, 12, 24]
        );
    }
}
