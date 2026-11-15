//! Лес на GPU (02.4): сетки и текстуры растений, списки экземпляров и
//! indirect-аргументы слотов. Списки: вид × LOD (0–8) и тени вида (9–11),
//! по `FOREST_CAP` экземпляров; раскладка совпадает с `forest.metal`.

use objc2_metal::*;

use super::frame::FRAME_SLOTS;
use super::resources::{as_allocation, buffer_with, shared_buffer};
use super::shaders::Obj;
use crate::layout::PlantInstance;
use crate::plants::PlantAssets;

pub const FOREST_LISTS: usize = 12;
pub const FOREST_CAP: usize = 16384;
pub const FOREST_GRID: usize = 160;
/// u32 в буфере счётчиков: 16 счётчиков (12 занято), затем аргументы по 5 u32.
pub const ARGS_OFFSET: usize = 16 * 4;
const COUNTERS_BYTES: usize = ARGS_OFFSET + FOREST_LISTS * 20;

/// Итог расстановки последнего завершённого кадра слота.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ForestStats {
    /// Видимых деревьев и валунов (все LOD), в тенях.
    pub visible: u32,
    pub shadow: u32,
    pub overflow: bool,
}

pub(super) struct ForestGpu {
    pub vertices: Obj<dyn MTLBuffer>,
    pub indices: Obj<dyn MTLBuffer>,
    /// (первый индекс, число индексов, базовая вершина, 0) вида × LOD.
    pub lods: Obj<dyn MTLBuffer>,
    pub albedo: Obj<dyn MTLTexture>,
    /// Сообщества (R8, номер) и параметры леса (256 × 2 float4); без карты — 1 × 1 нуль.
    pub vegetation: Obj<dyn MTLTexture>,
    pub vegetation_table: Obj<dyn MTLBuffer>,
    pub instances: Vec<Obj<dyn MTLBuffer>>,
    pub counters: Vec<Obj<dyn MTLBuffer>>,
    counted: [bool; FRAME_SLOTS],
    /// Стволы и крупные валуны завершённого кадра (x, z, радиус): деревья —
    /// из списков теней (все в 160 м от камеры), валуны — из LOD 0 (45 м).
    pub obstacles: Vec<[f32; 3]>,
}

impl ForestGpu {
    /// Списки и счётчики завершённого кадра слота; сброс для нового кадра.
    pub fn take_and_reset(&mut self, slot: usize) -> Option<ForestStats> {
        let c = &self.counters[slot];
        // SAFETY: shared-буфер слота; GPU закончил кадр слота.
        let words = unsafe {
            std::slice::from_raw_parts_mut(c.contents().as_ptr().cast::<u32>(), COUNTERS_BYTES / 4)
        };
        let stats = self.counted[slot].then(|| ForestStats {
            visible: words[..9].iter().map(|&n| n.min(FOREST_CAP as u32)).sum(),
            shadow: words[9..12].iter().map(|&n| n.min(FOREST_CAP as u32)).sum(),
            overflow: words[..12].iter().any(|&n| n as usize > FOREST_CAP),
        });
        if self.counted[slot] {
            let inst = &self.instances[slot];
            // SAFETY: shared-буфер слота; GPU закончил кадр слота.
            let all = unsafe {
                std::slice::from_raw_parts(
                    inst.contents().as_ptr().cast::<PlantInstance>(),
                    FOREST_LISTS * FOREST_CAP,
                )
            };
            self.obstacles.clear();
            // (список, радиус ствола у груди при масштабе 1, м).
            for (list, r) in [(9usize, 1.0f32), (10, 0.45), (6, 1.3)] {
                let n = (words[list] as usize).min(FOREST_CAP);
                for p in &all[list * FOREST_CAP..list * FOREST_CAP + n] {
                    if list == 6 && p.scale < 0.6 {
                        continue; // мелкий камень перешагивается
                    }
                    self.obstacles.push([p.position[0], p.position[2], r * p.scale]);
                }
            }
        }
        words[..16].fill(0);
        self.counted[slot] = true;
        stats
    }
}

impl super::Renderer {
    /// Растения для леса: сетки трёх видов и массив текстур коры и листвы.
    pub fn set_forest(
        &mut self,
        a: &PlantAssets,
        vegetation: Option<&crate::world::VegetationLayer>,
    ) -> Result<(), String> {
        let buf = |bytes: &[u8], name: &str| buffer_with(&self.device, bytes, name);
        let mut lods = Vec::new();
        for s in &a.lods {
            for l in s {
                lods.extend_from_slice(&[l.index_start, l.index_count, l.base_vertex, 0]);
            }
        }
        let albedo = super::resources::sampled_texture(
            &self.device,
            MTLPixelFormat::RGBA8Unorm_sRGB,
            crate::plants::TEXTURE_SIZE as u32,
            crate::plants::TEXTURE_LAYERS,
            true,
            "plants: bark + foliage",
        )?;
        self.upload_textures(&[super::resources::TextureUpload {
            texture: &albedo,
            data: &a.albedo,
            pixel_bytes: 4,
            mips: true,
        }])?;
        let zero = [0u8];
        let (ids, size) = vegetation.map_or((&zero[..], 1), |v| (&v.ids[..], v.size));
        let veg_tex = super::resources::sampled_texture(
            &self.device,
            MTLPixelFormat::R8Unorm,
            size,
            1,
            false,
            "forest: vegetation communities",
        )?;
        self.upload_textures(&[super::resources::TextureUpload {
            texture: &veg_tex,
            data: ids,
            pixel_bytes: 1,
            mips: false,
        }])?;
        let table: Vec<[f32; 8]> = vegetation.map_or(vec![[0.0; 8]; 256], |v| v.forest.clone());
        let inst_bytes = FOREST_LISTS * FOREST_CAP * std::mem::size_of::<PlantInstance>();
        let g = ForestGpu {
            vertices: buf(bytemuck::cast_slice(&a.vertices), "plants: vertices"),
            indices: buf(bytemuck::cast_slice(&a.indices), "plants: indices"),
            lods: buf(bytemuck::cast_slice(&lods), "plants: LOD ranges"),
            albedo,
            vegetation: veg_tex,
            vegetation_table: buf(bytemuck::cast_slice(&table), "forest: community table"),
            instances: (0..FRAME_SLOTS)
                .map(|s| {
                    shared_buffer(
                        &self.device,
                        inst_bytes,
                        &format!("forest instances, slot {s}"),
                    )
                })
                .collect(),
            counters: (0..FRAME_SLOTS)
                .map(|s| {
                    shared_buffer(
                        &self.device,
                        COUNTERS_BYTES,
                        &format!("forest counters, slot {s}"),
                    )
                })
                .collect(),
            counted: [false; FRAME_SLOTS],
            obstacles: Vec::new(),
        };
        self.residency.addAllocation(as_allocation(&*g.vegetation));
        for b in [&g.vertices, &g.indices, &g.lods, &g.vegetation_table]
            .into_iter()
            .chain(&g.instances)
            .chain(&g.counters)
        {
            self.residency.addAllocation(as_allocation(&**b));
        }
        self.residency.addAllocation(as_allocation(&*g.albedo));
        self.residency.commit();
        if let Some(old) = self.forest.replace(g) {
            self.retire(
                vec![old.albedo, old.vegetation],
                Some(Box::new((
                    old.vertices,
                    old.indices,
                    old.lods,
                    old.instances,
                    old.counters,
                    old.vegetation_table,
                ))),
            );
        }
        Ok(())
    }

    /// Препятствия леса последнего завершённого кадра (x, z, радиус).
    pub fn forest_obstacles(&self) -> &[[f32; 3]] {
        self.forest.as_ref().map_or(&[], |f| &f.obstacles)
    }

    pub fn has_forest(&self) -> bool {
        self.forest.is_some()
    }

    pub fn forest_stats(&self) -> Option<ForestStats> {
        self.forest_stats
    }
}
