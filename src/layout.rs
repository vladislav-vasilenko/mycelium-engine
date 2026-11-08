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
    /// Режим вида ландшафта `TerrainView` (0 — с освещением).
    pub view_mode: u32,
    /// Сторона карты, м: мировые XZ → координаты маски влажности.
    pub map_size_m: f32,
    pub _pad: [f32; 2],
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

/// Вершина ландшафта, 32 байта: `packed_float3` позиции и нормали,
/// `uchar4` весов (лес, торф, берег, тропа) и `uchar4` материала
/// (субстрат, русло, влажность, тип берега: 0 песок, 255 камень).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct TerrainVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub weights: [u8; 4],
    pub material: [u8; 4],
}

/// Волна Герстнера открытого моря, 32 байта. В MSL — `GerstnerWave`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct GerstnerWave {
    /// Единичное направление распространения в плоскости XZ.
    pub dir: [f32; 2],
    pub wavelength: f32,
    pub amplitude: f32,
    /// Доля горизонтального смещения Q (0 — синусоида, 1 — острый гребень).
    pub steepness: f32,
    pub phase: f32,
    /// Угловая частота, рад/с: ω = √(g·k).
    pub omega: f32,
    pub _pad: f32,
}

/// Число волн открытого моря в `SeaParams`.
pub const SEA_WAVES: usize = 8;
/// Точки проверки функции волн CPU ↔ GPU.
pub const WATER_PROBES: usize = 8;

/// Параметры моря и прибоя, 432 байта, выравнивание 16. В MSL — `SeaParams`.
#[repr(C, align(16))]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct SeaParams {
    pub sea_level: f32,
    pub wave_count: u32,
    /// Длина и амплитуда береговой волны на глубине, м.
    pub shore_wavelength: f32,
    pub shore_amplitude: f32,
    /// Угловая частота береговой волны (период наката), рад/с.
    pub shore_omega: f32,
    /// Высота заплеска на песке и на камне, м.
    pub swash_sand_m: f32,
    pub swash_rock_m: f32,
    /// Время высыхания песка после наката, с.
    pub dry_time_s: f32,
    /// Поле берега: точек на сторону и шаг, м.
    pub field_size: u32,
    pub field_step_m: f32,
    /// Шаг времени для маски мокрого песка, с.
    pub dt: f32,
    pub _pad: f32,
    pub waves: [GerstnerWave; SEA_WAVES],
    /// Точки проверки: (x, z, время, 0).
    pub probe: [[f32; 4]; WATER_PROBES],
}

/// Уровень кольцевой сетки воды, 32 байта. В MSL — `WaterLevel`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct WaterLevel {
    /// Мировые XZ угла уровня, м.
    pub origin: [f32; 2],
    /// Шаг сетки уровня, м.
    pub cell: f32,
    /// 1 — сшивать внешний край с более грубым уровнем.
    pub morph_edge: u32,
    /// Прямоугольник более мелкого уровня (min, max): эту часть рисует он.
    pub inner_min: [f32; 2],
    pub inner_max: [f32; 2],
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
        offset_of!(FrameUniforms, view_mode) as u32,
        offset_of!(FrameUniforms, map_size_m) as u32,
        size_of::<InstanceData>() as u32,
        offset_of!(InstanceData, tint) as u32,
        size_of::<Vertex>() as u32,
        offset_of!(Vertex, normal) as u32,
        offset_of!(Vertex, uv) as u32,
        size_of::<TerrainVertex>() as u32,
        offset_of!(TerrainVertex, normal) as u32,
        offset_of!(TerrainVertex, weights) as u32,
        offset_of!(TerrainVertex, material) as u32,
        size_of::<GerstnerWave>() as u32,
        size_of::<SeaParams>() as u32,
        offset_of!(SeaParams, waves) as u32,
        offset_of!(SeaParams, probe) as u32,
        size_of::<WaterLevel>() as u32,
    ]
}

pub const LAYOUT_PROBE_LEN: usize = 24;

pub const LAYOUT_PROBE_NAMES: [&str; LAYOUT_PROBE_LEN] = [
    "sizeof(FrameUniforms)",
    "alignof(FrameUniforms)",
    "FrameUniforms.camera_pos",
    "FrameUniforms.light_dir",
    "FrameUniforms.time",
    "FrameUniforms.exposure",
    "FrameUniforms.pattern_time",
    "FrameUniforms.frame_index",
    "FrameUniforms.view_mode",
    "FrameUniforms.map_size_m",
    "sizeof(InstanceData)",
    "InstanceData.tint",
    "sizeof(Vertex)",
    "Vertex.normal",
    "Vertex.uv",
    "sizeof(TerrainVertex)",
    "TerrainVertex.normal",
    "TerrainVertex.weights",
    "TerrainVertex.material",
    "sizeof(GerstnerWave)",
    "sizeof(SeaParams)",
    "SeaParams.waves",
    "SeaParams.probe",
    "sizeof(WaterLevel)",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_layout_matches_msl_rules() {
        // float4x4 = 64 байта, float4 выравнивается по 16.
        assert_eq!(
            expected_probe(),
            [
                128, 16, 64, 80, 96, 100, 104, 108, 112, 116, 80, 64, 32, 12, 24, 32, 12, 24, 28,
                32, 432, 48, 304, 32
            ]
        );
    }
}
