//! Кадр: входные данные, итог, статистика, слоты кадра и состав проходов.

use std::path::PathBuf;

use objc2_metal::*;

use super::resources::{as_allocation, label, shared_buffer};
use super::shaders::Obj;
use crate::frame_graph::{PassDecl, PassKind, ResourceId, Stages};
use crate::layout::{FrameUniforms, InstanceData, SeaParams, WaterLevel};
use crate::water::WATER_LEVELS;

pub(super) const FRAME_SLOTS: usize = 2;
pub(super) const SLOT_WAIT_TIMEOUT_MS: u64 = 100;
// Раскладка буфера upload слота: FrameUniforms @0, InstanceData[] @256..4096,
// SeaParams @4096, WaterLevel[] @4608, HandsUniforms @8192,
// палитра костей float4x4[MAX_JOINTS] @8448..12544.
pub(super) const UPLOAD_BYTES: usize = 12544;
pub(super) const INSTANCE_OFFSET: usize = 256;
pub(super) const INSTANCE_END: usize = 4096;
pub(super) const MAX_INSTANCES: usize =
    (INSTANCE_END - INSTANCE_OFFSET) / size_of::<InstanceData>();
pub(super) const SEA_OFFSET: usize = 4096;
pub(super) const LEVELS_OFFSET: usize = 4608;
pub(super) const HANDS_OFFSET: usize = 8192;
pub(super) const PALETTE_OFFSET: usize = 8448;

// Ресурсы для планировщика проходов.
const RES_PATTERN: ResourceId = ResourceId(1);
const RES_HDR: ResourceId = ResourceId(2);
const RES_DEPTH: ResourceId = ResourceId(3);
const RES_DRAWABLE: ResourceId = ResourceId(4);
const RES_READBACK: ResourceId = ResourceId(5);
const RES_WETNESS: ResourceId = ResourceId(6);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameOutcome {
    Presented,
    SkippedZeroSize,
    SkippedNoDrawable,
    SkippedBusySlot,
}

pub struct FrameInput<'a> {
    pub uniforms: FrameUniforms,
    /// Экземпляры куба: тестовая сцена или столбики маркеров.
    pub instances: &'a [InstanceData],
    /// Рисовать ландшафт, если он загружен `set_terrain`.
    pub draw_terrain: bool,
    /// Цвет очистки HDR-цели (линейный).
    pub clear_color: [f32; 3],
    pub readback: bool,
    pub capture: Option<PathBuf>,
    /// Море: параметры и уровни сетки вокруг камеры. `None` — без воды.
    pub water: Option<WaterFrame>,
    /// Руки: параметры и палитра. `None` — без рук.
    pub hands: Option<super::hands::HandsFrame>,
}

/// Вода в кадре (03.1).
pub struct WaterFrame {
    pub params: SeaParams,
    pub levels: [WaterLevel; WATER_LEVELS],
}

#[derive(Default, Debug, Clone)]
pub struct Stats {
    pub presented: u64,
    pub skipped_busy: u64,
    pub skipped_no_drawable: u64,
    pub released_deferred: u64,
    pub cpu_encode_ms: f64,
}

/// Таблица аргументов слота: по одной на проход или отрисовку.
#[derive(Clone, Copy, Debug)]
pub(super) enum Table {
    Pattern = 0,
    Scene,
    Readback,
    Tonemap,
    Terrain,
    WetSand,
    Water,
    WaterProbe,
    Hands,
    SkinProbe,
}

const TABLE_NAMES: [&str; 10] = [
    "pattern",
    "scene",
    "readback",
    "tonemap",
    "terrain",
    "wet sand",
    "water",
    "water probe",
    "hands",
    "skin probe",
];

/// Слот кадра: свой аллокатор, командный буфер, таблицы аргументов и upload.
/// Слот переиспользуется, когда GPU сигнализирует `signal_value`.
pub(super) struct FrameSlot {
    pub allocator: Obj<dyn MTL4CommandAllocator>,
    pub cmd: Obj<dyn MTL4CommandBuffer>,
    tables: [Obj<dyn MTL4ArgumentTable>; 10],
    pub upload: Obj<dyn MTLBuffer>,
    pub signal_value: u64,
}

impl FrameSlot {
    pub fn new(
        device: &objc2::runtime::ProtocolObject<dyn MTLDevice>,
        residency: &objc2::runtime::ProtocolObject<dyn MTLResidencySet>,
        index: usize,
    ) -> Result<FrameSlot, String> {
        let allocator = device.newCommandAllocator().ok_or("command allocator")?;
        let cmd = device.newCommandBuffer().ok_or("command buffer")?;
        cmd.setLabel(Some(&label(&format!("frame slot {index}"))));
        let table = |name: &str| -> Result<Obj<dyn MTL4ArgumentTable>, String> {
            let d = MTL4ArgumentTableDescriptor::new();
            d.setMaxBufferBindCount(4);
            d.setMaxTextureBindCount(4);
            d.setLabel(Some(&label(&format!("{name} args, slot {index}"))));
            device
                .newArgumentTableWithDescriptor_error(&d)
                .map_err(|e| format!("argument table: {}", e.localizedDescription()))
        };
        let mut tables = Vec::with_capacity(TABLE_NAMES.len());
        for name in TABLE_NAMES {
            tables.push(table(name)?);
        }
        let upload = shared_buffer(device, UPLOAD_BYTES, &format!("upload slot {index}"));
        residency.addAllocation(as_allocation(&*upload));
        Ok(FrameSlot {
            allocator,
            cmd,
            tables: tables
                .try_into()
                .unwrap_or_else(|_| unreachable!("10 таблиц")),
            upload,
            signal_value: 0,
        })
    }

    pub fn table(&self, t: Table) -> &Obj<dyn MTL4ArgumentTable> {
        &self.tables[t as usize]
    }
}

/// Состав кадра для планировщика барьеров.
pub(super) fn frame_passes(readback: bool) -> Vec<PassDecl> {
    let mut passes = vec![
        PassDecl {
            name: "pattern",
            kind: PassKind::Compute,
            reads: vec![],
            writes: vec![(RES_PATTERN, Stages::DISPATCH)],
        },
        PassDecl {
            name: "wet_sand",
            kind: PassKind::Compute,
            reads: vec![],
            writes: vec![(RES_WETNESS, Stages::DISPATCH)],
        },
        PassDecl {
            name: "scene",
            kind: PassKind::Render,
            reads: vec![
                (RES_PATTERN, Stages::FRAGMENT),
                (RES_WETNESS, Stages::FRAGMENT),
            ],
            writes: vec![(RES_HDR, Stages::FRAGMENT), (RES_DEPTH, Stages::FRAGMENT)],
        },
    ];
    if readback {
        passes.push(PassDecl {
            name: "readback",
            kind: PassKind::Compute,
            reads: vec![
                (RES_PATTERN, Stages::BLIT),
                (RES_DEPTH, Stages::BLIT),
                (RES_HDR, Stages::BLIT),
            ],
            writes: vec![(RES_READBACK, Stages::DISPATCH | Stages::BLIT)],
        });
    }
    // Руки поверх сцены: пишут HDR и очищают глубину после копий readback.
    passes.push(PassDecl {
        name: "viewmodel",
        kind: PassKind::Render,
        reads: vec![],
        writes: vec![(RES_HDR, Stages::FRAGMENT), (RES_DEPTH, Stages::FRAGMENT)],
    });
    passes.push(PassDecl {
        name: "tonemap",
        kind: PassKind::Render,
        reads: vec![(RES_HDR, Stages::FRAGMENT)],
        writes: vec![(RES_DRAWABLE, Stages::FRAGMENT)],
    });
    passes
}
