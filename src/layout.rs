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
    /// Поле высот кластерного рельефа (02.3): узлов по x и шаг, м.
    pub grid_nx: u32,
    pub grid_step_m: f32,
    /// Глубина глаз под поверхностью воды, м; 0 — над водой (подводный вид).
    pub underwater_m: f32,
    /// Доля леса в поясах у глаз 0…1: руки под пологом в тени (02.7).
    pub canopy_eye: f32,
    /// Предэкспозиция (04.1): освещённые шейдеры пишут яркость × это число,
    /// тонмаппинг делит обратно — ночные яркости ~10⁻⁶ не теряются в half.
    pub pre_exposure: f32,
    pub _pad: f32,
    /// Проекция × поворот вида без сдвига: позиции идут относительно камеры
    /// (AD-49), иначе сдвиг на десятки км съедает точность f32.
    pub view_proj_rel: [[f32; 4]; 4],
    /// Камера = `cam_hi + cam_lo`: крупная часть кратна 256 м (разность с ней
    /// точна), мелкая — остаток. `cam_hi` — и начало узоров текстур.
    pub cam_hi: [f32; 4],
    pub cam_lo: [f32; 4],
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

/// Вершина рук со скином (этап 10), 52 байта: `packed_float3` позиции и
/// нормали, `packed_float2` UV, `uchar4` индексов костей, `packed_float4` весов.
/// Всё упаковано: выравнивание 4, без хвостов.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct SkinnedVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    pub joints: [u8; 4],
    pub weights: [f32; 4],
}

/// Параметры прохода `viewmodel`, 144 байта. Палитра костей лежит отдельно,
/// сразу после них в upload (`float4x4[joint_count]`).
#[repr(C, align(16))]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct HandsUniforms {
    /// Проекция рук (свой угол обзора и ближняя плоскость), reversed-Z.
    pub proj: [[f32; 4]; 4],
    /// Поворот пространства глаза в мир: нормали для освещения.
    pub view_to_world: [[f32; 4]; 4],
    pub vertex_count: u32,
    pub joint_count: u32,
    pub _pad: [u32; 2],
}

/// Отрезок объекта карты (тропа, река, дорога), 32 байта (02.7).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct FeatureSegment {
    pub a: [f32; 2],
    pub b: [f32; 2],
    /// Полуширина полосы, м.
    pub half_width: f32,
    /// 0 — тропа, 1 — река, 2 — дорога.
    pub kind: u32,
    pub _pad: [f32; 2],
}

/// Кластер рельефа для вершинного шейдера (02.3), 16 байт. Вершины кластера —
/// номера узлов поля высот в `cluster_vertices[vertex_offset..]`, треугольники —
/// по три локальных индекса u8 в `cluster_triangles[triangle_offset..]` (u32 на треугольник).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct TerrainCluster {
    pub vertex_offset: u32,
    pub triangle_offset: u32,
    pub triangle_count: u32,
    /// Уровень иерархии (0 — исходная сетка).
    pub depth: u32,
}

/// Игровая зона рельефа для вершинного шейдера кластеров (02.7, шаг 5), 32 байта.
/// Узел кластера ≥ `far_count` — узел зоны с плоским номером `узел − far_count`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct TerrainZone {
    /// Узлов поля острова; `u32::MAX` — зоны нет.
    pub far_count: u32,
    /// Узлов зоны по стороне.
    pub n: u32,
    pub step_m: f32,
    /// Угол зоны в мире, м. Отдельные `f32`, а не `[f32; 2]`: `float2` в MSL
    /// выравнивается по 8 и сдвинул бы поле.
    pub origin_x: f32,
    pub origin_z: f32,
    pub _pad: [f32; 3],
}

/// Входы неба от CPU (04.1), 176 байт; на GPU — `SkyInputs` в `common.h`.
#[repr(C, align(16))]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct SkyInputs {
    /// К солнцу в мировых осях (x восток, y вверх, z юг).
    pub sun_dir: [f32; 4],
    /// К луне; w — освещённая доля.
    pub moon_dir: [f32; 4],
    /// Строки поворота экваториальных осей в мировые (звёзды).
    pub star_rot: [[f32; 4]; 3],
    /// x — фиксированная экспозиция (0 — авто), y — dt кадра, с, z — время, с,
    /// w — освещённость солнца на верхней границе атмосферы.
    pub params: [f32; 4],
    /// Обратная к `view_proj_rel`: направление взгляда пикселя в проходе неба.
    pub inv_view_proj_rel: [[f32; 4]; 4],
    /// x — телесный угол пикселя, ср (яркость звёзд), y — звёзд в буфере.
    pub view: [f32; 4],
}

/// Сколько вершин рук проверяет ядро `skin_probe`: вершины `i × (n / SKIN_PROBES)`.
pub const SKIN_PROBES: usize = 8;

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
        offset_of!(FrameUniforms, underwater_m) as u32,
        offset_of!(FrameUniforms, view_proj_rel) as u32,
        offset_of!(FrameUniforms, cam_lo) as u32,
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
        size_of::<SkinnedVertex>() as u32,
        offset_of!(SkinnedVertex, normal) as u32,
        offset_of!(SkinnedVertex, uv) as u32,
        offset_of!(SkinnedVertex, joints) as u32,
        offset_of!(SkinnedVertex, weights) as u32,
        size_of::<HandsUniforms>() as u32,
        offset_of!(HandsUniforms, vertex_count) as u32,
        size_of::<FeatureSegment>() as u32,
        offset_of!(FeatureSegment, half_width) as u32,
        offset_of!(FeatureSegment, kind) as u32,
        size_of::<TerrainCluster>() as u32,
        offset_of!(TerrainCluster, depth) as u32,
        size_of::<SkyInputs>() as u32,
        offset_of!(SkyInputs, params) as u32,
        offset_of!(SkyInputs, inv_view_proj_rel) as u32,
        size_of::<TerrainZone>() as u32,
    ]
}

pub const LAYOUT_PROBE_LEN: usize = 43;

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
    "FrameUniforms.underwater_m",
    "FrameUniforms.view_proj_rel",
    "FrameUniforms.cam_lo",
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
    "sizeof(SkinnedVertex)",
    "SkinnedVertex.normal",
    "SkinnedVertex.uv",
    "SkinnedVertex.joints",
    "SkinnedVertex.weights",
    "sizeof(HandsUniforms)",
    "HandsUniforms.vertex_count",
    "sizeof(FeatureSegment)",
    "FeatureSegment.half_width",
    "FeatureSegment.kind",
    "sizeof(TerrainCluster)",
    "TerrainCluster.depth",
    "sizeof(SkyInputs)",
    "SkyInputs.params",
    "SkyInputs.inv_view_proj_rel",
    "sizeof(TerrainZone)",
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
                240, 16, 64, 80, 96, 100, 104, 108, 112, 116, 128, 144, 224, 80, 64, 32, 12, 24,
                32, 12, 24, 28, 32, 432, 48, 304, 32, 52, 12, 24, 32, 36, 144, 128, 32, 16, 20, 16,
                12, 176, 80, 96, 32
            ]
        );
    }
}
