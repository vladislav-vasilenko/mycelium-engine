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
// палитра костей float4x4[MAX_JOINTS] @8448..12544, SkyInputs @12544..12704.
pub(super) const UPLOAD_BYTES: usize = 14336;
pub(super) const INSTANCE_OFFSET: usize = 256;
pub(super) const INSTANCE_END: usize = 4096;
pub(super) const MAX_INSTANCES: usize =
    (INSTANCE_END - INSTANCE_OFFSET) / size_of::<InstanceData>();
pub(super) const SEA_OFFSET: usize = 4096;
pub(super) const LEVELS_OFFSET: usize = 4608;
pub(super) const HANDS_OFFSET: usize = 8192;
pub(super) const PALETTE_OFFSET: usize = 8448;
/// `SkyInputs` (04.1) после палитры костей.
pub(super) const SKY_OFFSET: usize = 12544;
/// `ShadowUniforms` (04.2) после входов неба.
pub(super) const SHADOW_OFFSET: usize = 12800;
/// `CullView` × 5 (02.5) после теней.
pub(super) const CULL_OFFSET: usize = 13312;
/// `TaaUniforms` (04.5) после видов среза (13312 + 5 × 128).
pub(super) const TAA_OFFSET: usize = 13952;

// Ресурсы для планировщика проходов.
const RES_PATTERN: ResourceId = ResourceId(1);
const RES_HDR: ResourceId = ResourceId(2);
const RES_DEPTH: ResourceId = ResourceId(3);
const RES_DRAWABLE: ResourceId = ResourceId(4);
const RES_READBACK: ResourceId = ResourceId(5);
const RES_WETNESS: ResourceId = ResourceId(6);
/// LUT неба, свет неба и экспозиция (04.1).
const RES_SKY: ResourceId = ResourceId(7);
/// Состояние экспозиции между кадрами (04.1).
const RES_EXPOSURE: ResourceId = ResourceId(8);
/// Каскады карты теней (04.2).
const RES_SHADOW: ResourceId = ResourceId(9);
/// Списки видимых кластеров и indirect-аргументы (02.5).
const RES_CULL: ResourceId = ResourceId(10);
/// Списки растений и их indirect-аргументы (02.4).
const RES_FOREST: ResourceId = ResourceId(11);
/// История временного сглаживания (04.5).
const RES_TAA: ResourceId = ResourceId(12);
/// Цепочка bloom (04.5).
const RES_BLOOM: ResourceId = ResourceId(13);
/// Ускоряющие структуры для лучей (11).
const RES_RT: ResourceId = ResourceId(14);
/// Затенение лучами (11): пишет `rt_ao` после сцены, читает сцена следующего кадра.
pub(super) const RES_AO: ResourceId = ResourceId(15);
/// Объёмный свет (11, сессия 2).
const RES_VOL: ResourceId = ResourceId(16);
/// Пробы освещённости (11, сессия 3).
const RES_PROBES: ResourceId = ResourceId(17);

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
    /// Кластерный рельеф: видимые кластеры среза. `None` — тайлы уровня 0.
    pub clusters: Option<&'a [u32]>,
    /// Небо (04.1): солнце, луна, звёзды. `None` — без неба (тестовые кубы).
    pub sky: Option<crate::layout::SkyInputs>,
    /// Каскадные тени (04.2): матрицы и диапазоны списков кластеров каскадов.
    pub shadows: Option<super::shadows::ShadowFrame>,
    /// Кластеры каскадов подряд; ложатся в буфер слота после `clusters`.
    pub shadow_clusters: &'a [u32],
    /// Выбор кластеров на GPU (02.5): камеры видов. Тогда `clusters` и
    /// `shadow_clusters` не используются, рисование — по indirect-аргументам.
    pub cull: Option<super::cull::CullFrame>,
    /// Рисовать лес (02.4); нужны растения, кластеры и выбор на GPU.
    pub forest: bool,
    /// Грибы и предметы с CPU (05.1): экземпляры вида «гриб», `_pad` — номер вида гриба.
    pub props: &'a [crate::layout::PlantInstance],
    /// Интерфейс кадра (05.1): треугольники в пикселях окна.
    pub ui: &'a [crate::layout::UiVertex],
    /// Временное сглаживание (04.5): параметры перепроекции; `None` — выключено.
    pub taa: Option<crate::layout::TaaUniforms>,
    /// Bloom (04.5): цепочка считается и подмешивается в тонмаппинге.
    pub bloom: bool,
    /// Лучи затенения (11) разрешены (`--no-rt` выключает).
    pub rt: bool,
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
    Sky,
    SkyDraw,
    Exposure,
    Shadow,
    Cull,
    Forest,
    Plant,
    PlantShadow,
    Ui,
    Taa,
    Bloom,
    Rt,
    Volumetric,
    /// Капли дождя в проходе `scene` (04.3, сессия 3).
    Rain,
    /// Объёмные облака (04.3, сессия 4б) — после сцены, по её глубине.
    Clouds,
    /// Поверхность рек (03.2, сессия 3).
    River,
    /// Поверхность рек в карте укрытия от дождя (03.2).
    RiverRain,
    /// Предмет в руке (этап 10).
    HandProp,
}

const TABLE_NAMES: [&str; 28] = [
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
    "sky",
    "sky draw",
    "exposure",
    "shadow",
    "cull",
    "forest",
    "plant",
    "plant shadow",
    "ui",
    "taa",
    "bloom",
    "rt",
    "volumetric",
    "rain",
    "clouds",
    "river",
    "river rain",
    "hand prop",
];

/// Слот кадра: свой аллокатор, командный буфер, таблицы аргументов и upload.
/// Слот переиспользуется, когда GPU сигнализирует `signal_value`.
pub(super) struct FrameSlot {
    pub allocator: Obj<dyn MTL4CommandAllocator>,
    pub cmd: Obj<dyn MTL4CommandBuffer>,
    tables: [Obj<dyn MTL4ArgumentTable>; 28],
    pub upload: Obj<dyn MTLBuffer>,
    /// Видимые кластеры рельефа кадра (u32), `MAX_VISIBLE_CLUSTERS`.
    pub visible: Obj<dyn MTLBuffer>,
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
            d.setMaxBufferBindCount(16);
            d.setMaxTextureBindCount(16);
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
        let visible = shared_buffer(
            device,
            super::cull::VISIBLE_TOTAL * 4,
            &format!("visible clusters slot {index}"),
        );
        residency.addAllocation(as_allocation(&*visible));
        Ok(FrameSlot {
            allocator,
            cmd,
            tables: tables
                .try_into()
                .unwrap_or_else(|_| unreachable!("28 таблиц")),
            upload,
            visible,
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
            name: "sky_lut",
            kind: PassKind::Compute,
            reads: vec![],
            writes: vec![(RES_SKY, Stages::DISPATCH)],
        },
        PassDecl {
            name: "cull",
            kind: PassKind::Compute,
            reads: vec![],
            writes: vec![(RES_CULL, Stages::DISPATCH)],
        },
        PassDecl {
            name: "forest",
            kind: PassKind::Compute,
            reads: vec![],
            writes: vec![(RES_FOREST, Stages::DISPATCH)],
        },
        PassDecl {
            name: "rt",
            kind: PassKind::Compute,
            reads: vec![(RES_FOREST, Stages::DISPATCH)],
            writes: vec![(RES_RT, Stages::DISPATCH | Stages::ACCEL)],
        },
        PassDecl {
            name: "probes",
            kind: PassKind::Compute,
            reads: vec![(RES_RT, Stages::DISPATCH)],
            writes: vec![(RES_PROBES, Stages::DISPATCH)],
        },
        PassDecl {
            name: "shadows",
            kind: PassKind::Render,
            reads: vec![(RES_CULL, Stages::VERTEX), (RES_FOREST, Stages::VERTEX)],
            writes: vec![(RES_SHADOW, Stages::FRAGMENT)],
        },
        PassDecl {
            name: "scene",
            kind: PassKind::Render,
            reads: vec![
                (RES_PATTERN, Stages::FRAGMENT),
                (RES_WETNESS, Stages::FRAGMENT),
                (RES_SKY, Stages::FRAGMENT),
                (RES_SHADOW, Stages::FRAGMENT),
                (RES_CULL, Stages::VERTEX),
                (RES_FOREST, Stages::VERTEX | Stages::FRAGMENT),
                (RES_RT, Stages::FRAGMENT),
                (RES_AO, Stages::FRAGMENT),
                (RES_PROBES, Stages::FRAGMENT),
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
    // Затенение лучами (11) по глубине сцены — для освещения следующего кадра.
    passes.push(PassDecl {
        name: "rt_ao",
        kind: PassKind::Compute,
        reads: vec![(RES_DEPTH, Stages::DISPATCH), (RES_RT, Stages::DISPATCH)],
        writes: vec![(RES_AO, Stages::DISPATCH)],
    });
    // Объёмный свет (11): по глубине и карте теней — добавляет TAA.
    passes.push(PassDecl {
        name: "volumetric",
        kind: PassKind::Compute,
        reads: vec![
            (RES_DEPTH, Stages::DISPATCH),
            (RES_SHADOW, Stages::DISPATCH),
        ],
        writes: vec![(RES_VOL, Stages::DISPATCH)],
    });
    // Временное сглаживание (04.5): читает HDR и глубину сцены, пишет историю
    // и копирует её обратно в HDR — до рук (руки без сдвига и без шлейфа).
    passes.push(PassDecl {
        name: "taa",
        kind: PassKind::Compute,
        reads: vec![(RES_HDR, Stages::DISPATCH), (RES_DEPTH, Stages::DISPATCH)],
        writes: vec![
            (RES_HDR, Stages::BLIT),
            (RES_TAA, Stages::DISPATCH | Stages::BLIT),
        ],
    });
    // Руки поверх сцены: пишут HDR и очищают глубину после копий readback.
    passes.push(PassDecl {
        name: "viewmodel",
        kind: PassKind::Render,
        reads: vec![(RES_SKY, Stages::FRAGMENT)],
        writes: vec![(RES_HDR, Stages::FRAGMENT), (RES_DEPTH, Stages::FRAGMENT)],
    });
    // Bloom (04.5) по готовому кадру с руками.
    passes.push(PassDecl {
        name: "bloom",
        kind: PassKind::Compute,
        reads: vec![(RES_HDR, Stages::DISPATCH)],
        writes: vec![(RES_BLOOM, Stages::DISPATCH)],
    });
    // Замер экспозиции по готовому кадру (с руками) перед тонмаппингом.
    passes.push(PassDecl {
        name: "exposure",
        kind: PassKind::Compute,
        reads: vec![(RES_HDR, Stages::DISPATCH)],
        writes: vec![(RES_EXPOSURE, Stages::DISPATCH)],
    });
    passes.push(PassDecl {
        name: "tonemap",
        kind: PassKind::Render,
        reads: vec![
            (RES_HDR, Stages::FRAGMENT),
            (RES_EXPOSURE, Stages::FRAGMENT),
            (RES_BLOOM, Stages::FRAGMENT),
        ],
        writes: vec![(RES_DRAWABLE, Stages::FRAGMENT)],
    });
    passes.push(PassDecl {
        name: "ui",
        kind: PassKind::Render,
        reads: vec![],
        writes: vec![(RES_DRAWABLE, Stages::FRAGMENT)],
    });
    passes
}
