//! Конкретный рендерер на Metal 4.
//!
//! Одна очередь `MTL4CommandQueue` для compute, render и copy. Два слота кадра,
//! у каждого свой `MTL4CommandAllocator`, командный буфер, таблицы аргументов
//! и upload-буфер в `StorageModeShared`. Завершение кадра отмечает `MTLSharedEvent`.
//! Все нативные типы и `unsafe` вызовы Metal остаются в этом модуле.

pub mod shaders;

use std::any::Any;
use std::path::{Path, PathBuf};
use std::ptr::NonNull;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2_foundation::{NSProcessInfo, NSString, NSURL};
use objc2_metal::*;
use objc2_quartz_core::{CAMetalDrawable, CAMetalLayer};

use crate::frame_graph::{self, FramePlan, PassDecl, PassKind, ResourceId, Stages};
use crate::layout::{FrameUniforms, InstanceData, LAYOUT_PROBE_LEN, Vertex};
use crate::reference::PATTERN_SIZE;
use shaders::{Obj, Pipelines, ShaderError, SourceOverride};

const FRAME_SLOTS: usize = 2;
const SLOT_WAIT_TIMEOUT_MS: u64 = 100;
const UPLOAD_BYTES: usize = 4096;
const INSTANCE_OFFSET: usize = 256;
const MAX_INSTANCES: usize = (UPLOAD_BYTES - INSTANCE_OFFSET) / size_of::<InstanceData>();

// Ресурсы для планировщика проходов.
const RES_PATTERN: ResourceId = ResourceId(1);
const RES_HDR: ResourceId = ResourceId(2);
const RES_DEPTH: ResourceId = ResourceId(3);
const RES_DRAWABLE: ResourceId = ResourceId(4);
const RES_READBACK: ResourceId = ResourceId(5);

// Раскладка буфера readback.
const RB_PATTERN_BYTES: usize = (PATTERN_SIZE * PATTERN_SIZE * 4) as usize;
const RB_DEPTH_OFFSET: usize = RB_PATTERN_BYTES;
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
const RB_PROBE_OFFSET: usize = RB_DEPTH_OFFSET + 256;
const RB_BYTES: usize = RB_PROBE_OFFSET + 256;

#[derive(Clone, Debug)]
pub struct DeviceInfo {
    pub name: String,
    pub os: String,
    pub metal4: bool,
    pub apple_family: Option<u32>,
    /// `None` — не запрашивалось (режим GPU capture).
    pub raytracing: Option<bool>,
    pub raytracing_from_render: Option<bool>,
}

fn flag(v: Option<bool>) -> &'static str {
    match v {
        Some(true) => "да",
        Some(false) => "нет",
        None => "не запрошено (capture)",
    }
}

impl std::fmt::Display for DeviceInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "GPU: {} | ОС: {} | Metal 4: {} | семейство Apple{} | raytracing: {} | raytracing from render: {}",
            self.name,
            self.os,
            if self.metal4 { "да" } else { "нет" },
            self.apple_family.map_or("?".to_string(), |v| v.to_string()),
            flag(self.raytracing),
            flag(self.raytracing_from_render)
        )
    }
}

fn capture_enabled() -> bool {
    std::env::var_os("MTL_CAPTURE_ENABLED").is_some_and(|v| v == "1")
}

pub fn probe_device() -> Result<(Obj<dyn MTLDevice>, DeviceInfo), String> {
    let device = MTLCreateSystemDefaultDevice().ok_or("Metal-устройство не найдено")?;
    let families = [
        (10u32, MTLGPUFamily::Apple10),
        (9, MTLGPUFamily::Apple9),
        (8, MTLGPUFamily::Apple8),
        (7, MTLGPUFamily::Apple7),
    ];
    let apple_family = families
        .iter()
        .find(|(_, f)| device.supportsFamily(*f))
        .map(|(v, _)| *v);
    let info = DeviceInfo {
        name: device.name().to_string(),
        os: NSProcessInfo::processInfo()
            .operatingSystemVersionString()
            .to_string(),
        metal4: device.supportsFamily(MTLGPUFamily::Metal4),
        apple_family,
        // При GPU capture устройство обёрнуто в CaptureMTLDevice, который только
        // пересылает эти селекторы; проверка objc2 в Debug считает это ошибкой.
        // В режиме capture признаки трассировки не запрашиваем.
        raytracing: (!capture_enabled()).then(|| device.supportsRaytracing()),
        raytracing_from_render: (!capture_enabled()).then(|| device.supportsRaytracingFromRender()),
    };
    Ok((device, info))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameOutcome {
    Presented,
    SkippedZeroSize,
    SkippedNoDrawable,
    SkippedBusySlot,
}

pub struct FrameInput<'a> {
    pub uniforms: FrameUniforms,
    pub instances: &'a [InstanceData],
    pub readback: bool,
    pub capture: Option<PathBuf>,
}

pub struct Readback {
    pub frame: u64,
    pub pattern_time: f32,
    pub width: u32,
    pub height: u32,
    pub pattern: Vec<u8>,
    /// (x, y, глубина) в пикселях целевого буфера.
    pub depth: Vec<(u32, u32, f32)>,
    pub probe: [u32; LAYOUT_PROBE_LEN],
}

struct PendingReadback {
    signal_value: u64,
    frame: u64,
    pattern_time: f32,
    width: u32,
    height: u32,
    pixels: Vec<(u32, u32)>,
}

struct FrameSlot {
    allocator: Obj<dyn MTL4CommandAllocator>,
    cmd: Obj<dyn MTL4CommandBuffer>,
    tables: [Obj<dyn MTL4ArgumentTable>; 4],
    upload: Obj<dyn MTLBuffer>,
    signal_value: u64,
}

struct Targets {
    width: u32,
    height: u32,
    hdr: Obj<dyn MTLTexture>,
    depth: Obj<dyn MTLTexture>,
}

/// Ресурс, который нельзя освобождать до завершения кадров, где он использовался.
struct Retired {
    after_value: u64,
    textures: Vec<Obj<dyn MTLTexture>>,
    /// Держит объект (например, старые pipeline) живым до завершения кадров.
    _keep_alive: Option<Box<dyn Any>>,
}

#[derive(Default, Debug, Clone)]
pub struct Stats {
    pub presented: u64,
    pub skipped_busy: u64,
    pub skipped_no_drawable: u64,
    pub released_deferred: u64,
    pub cpu_encode_ms: f64,
}

pub struct Renderer {
    pub info: DeviceInfo,
    device: Obj<dyn MTLDevice>,
    queue: Obj<dyn MTL4CommandQueue>,
    compiler: Obj<dyn MTL4Compiler>,
    layer: Retained<CAMetalLayer>,
    residency: Obj<dyn MTLResidencySet>,
    event: Obj<dyn MTLSharedEvent>,
    depth_state: Obj<dyn MTLDepthStencilState>,
    slots: Vec<FrameSlot>,
    pipelines: Pipelines,
    targets: Option<Targets>,
    pattern_tex: Obj<dyn MTLTexture>,
    vertex_buf: Obj<dyn MTLBuffer>,
    index_buf: Obj<dyn MTLBuffer>,
    index_count: usize,
    readback_buf: Obj<dyn MTLBuffer>,
    plan: FramePlan,
    plan_readback: FramePlan,
    frame_index: u64,
    retired: Vec<Retired>,
    pending_readback: Option<PendingReadback>,
    completed_readbacks: Vec<Readback>,
    shader_dir: PathBuf,
    pub stats: Stats,
}

fn label(s: &str) -> Retained<NSString> {
    NSString::from_str(s)
}

fn to_mtl_stages(s: Stages) -> MTLStages {
    let mut out = MTLStages(0);
    if s.contains(Stages::VERTEX) {
        out |= MTLStages::Vertex;
    }
    if s.contains(Stages::FRAGMENT) {
        out |= MTLStages::Fragment;
    }
    if s.contains(Stages::DISPATCH) {
        out |= MTLStages::Dispatch;
    }
    if s.contains(Stages::BLIT) {
        out |= MTLStages::Blit;
    }
    out
}

fn as_allocation<T: ?Sized + MTLAllocation>(
    obj: &ProtocolObject<T>,
) -> &ProtocolObject<dyn MTLAllocation>
where
    ProtocolObject<T>: MTLAllocation,
{
    ProtocolObject::from_ref(obj)
}

fn shared_buffer(
    device: &ProtocolObject<dyn MTLDevice>,
    len: usize,
    name: &str,
) -> Obj<dyn MTLBuffer> {
    let buf = device
        .newBufferWithLength_options(len, MTLResourceOptions::StorageModeShared)
        .expect("не удалось создать буфер");
    buf.setLabel(Some(&label(name)));
    buf
}

fn upload_bytes(buf: &ProtocolObject<dyn MTLBuffer>, offset: usize, bytes: &[u8]) {
    assert!(
        offset + bytes.len() <= buf.length(),
        "запись за пределы буфера"
    );
    // SAFETY: буфер в shared-памяти, диапазон проверен выше. Слот кадра
    // переиспользуется только после сигнала завершения его кадра на GPU.
    unsafe {
        let dst = buf.contents().as_ptr().cast::<u8>().add(offset);
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), dst, bytes.len());
    }
}

fn private_texture(
    device: &ProtocolObject<dyn MTLDevice>,
    format: MTLPixelFormat,
    width: u32,
    height: u32,
    usage: MTLTextureUsage,
    name: &str,
) -> Obj<dyn MTLTexture> {
    let desc = unsafe {
        MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
            format,
            width as usize,
            height as usize,
            false,
        )
    };
    desc.setUsage(usage);
    desc.setStorageMode(MTLStorageMode::Private);
    let tex = device
        .newTextureWithDescriptor(&desc)
        .expect("не удалось создать текстуру");
    tex.setLabel(Some(&label(name)));
    tex
}

fn frame_passes(readback: bool) -> Vec<PassDecl> {
    let mut passes = vec![
        PassDecl {
            name: "pattern",
            kind: PassKind::Compute,
            reads: vec![],
            writes: vec![(RES_PATTERN, Stages::DISPATCH)],
        },
        PassDecl {
            name: "scene",
            kind: PassKind::Render,
            reads: vec![(RES_PATTERN, Stages::FRAGMENT)],
            writes: vec![(RES_HDR, Stages::FRAGMENT), (RES_DEPTH, Stages::FRAGMENT)],
        },
    ];
    if readback {
        passes.push(PassDecl {
            name: "readback",
            kind: PassKind::Compute,
            reads: vec![(RES_PATTERN, Stages::BLIT), (RES_DEPTH, Stages::BLIT)],
            writes: vec![(RES_READBACK, Stages::DISPATCH | Stages::BLIT)],
        });
    }
    passes.push(PassDecl {
        name: "tonemap",
        kind: PassKind::Render,
        reads: vec![(RES_HDR, Stages::FRAGMENT)],
        writes: vec![(RES_DRAWABLE, Stages::FRAGMENT)],
    });
    passes
}

impl Renderer {
    pub fn new(
        device: Obj<dyn MTLDevice>,
        info: DeviceInfo,
        layer: Retained<CAMetalLayer>,
        vertices: &[Vertex],
        indices: &[u16],
    ) -> Result<Renderer, String> {
        if !info.metal4 {
            return Err(format!(
                "GPU «{}» не поддерживает MTLGPUFamilyMetal4. Нужен Apple silicon (M1 и новее) и macOS 26 или новее.",
                info.name
            ));
        }
        layer.setDevice(Some(&device));
        layer.setPixelFormat(shaders::DRAWABLE_FORMAT);
        layer.setFramebufferOnly(true);
        layer.setMaximumDrawableCount(3);

        let queue_desc = MTL4CommandQueueDescriptor::new();
        queue_desc.setLabel(Some(&label("main queue")));
        let queue = device
            .newMTL4CommandQueueWithDescriptor_error(&queue_desc)
            .map_err(|e| format!("MTL4CommandQueue: {}", e.localizedDescription()))?;
        let compiler = device
            .newCompilerWithDescriptor_error(&MTL4CompilerDescriptor::new())
            .map_err(|e| format!("MTL4Compiler: {}", e.localizedDescription()))?;
        let event = device
            .newSharedEvent()
            .ok_or("не удалось создать MTLSharedEvent")?;
        event.setLabel(Some(&label("frame completion")));

        let residency_desc = MTLResidencySetDescriptor::new();
        residency_desc.setLabel(Some(&label("engine resources")));
        let residency = device
            .newResidencySetWithDescriptor_error(&residency_desc)
            .map_err(|e| format!("MTLResidencySet: {}", e.localizedDescription()))?;

        let ds_desc = MTLDepthStencilDescriptor::new();
        ds_desc.setDepthCompareFunction(MTLCompareFunction::GreaterEqual);
        ds_desc.setDepthWriteEnabled(true);
        ds_desc.setLabel(Some(&label("reversed-Z GreaterEqual")));
        let depth_state = device
            .newDepthStencilStateWithDescriptor(&ds_desc)
            .ok_or("depth state")?;

        let shader_dir = shaders::shader_dir();
        let pipelines = shaders::build_pipelines(&device, &compiler, &shader_dir, 1, None)
            .map_err(|e| e.to_string())?;

        let vertex_bytes: &[u8] = bytemuck::cast_slice(vertices);
        let vertex_buf = shared_buffer(&device, vertex_bytes.len(), "cube vertices");
        upload_bytes(&vertex_buf, 0, vertex_bytes);
        let index_bytes: &[u8] = bytemuck::cast_slice(indices);
        let index_buf = shared_buffer(&device, index_bytes.len(), "cube indices");
        upload_bytes(&index_buf, 0, index_bytes);
        let readback_buf = shared_buffer(&device, RB_BYTES, "readback");
        let pattern_tex = private_texture(
            &device,
            shaders::PATTERN_FORMAT,
            PATTERN_SIZE,
            PATTERN_SIZE,
            MTLTextureUsage::ShaderRead | MTLTextureUsage::ShaderWrite,
            "pattern (compute → fragment)",
        );

        let mut slots = Vec::with_capacity(FRAME_SLOTS);
        for i in 0..FRAME_SLOTS {
            let allocator = device.newCommandAllocator().ok_or("command allocator")?;
            let cmd = device.newCommandBuffer().ok_or("command buffer")?;
            cmd.setLabel(Some(&label(&format!("frame slot {i}"))));
            let table = |name: &str| -> Result<Obj<dyn MTL4ArgumentTable>, String> {
                let d = MTL4ArgumentTableDescriptor::new();
                d.setMaxBufferBindCount(4);
                d.setMaxTextureBindCount(2);
                d.setLabel(Some(&label(&format!("{name} args, slot {i}"))));
                device
                    .newArgumentTableWithDescriptor_error(&d)
                    .map_err(|e| format!("argument table: {}", e.localizedDescription()))
            };
            let tables = [
                table("pattern")?,
                table("scene")?,
                table("readback")?,
                table("tonemap")?,
            ];
            let upload = shared_buffer(&device, UPLOAD_BYTES, &format!("upload slot {i}"));
            residency.addAllocation(as_allocation(&*upload));
            slots.push(FrameSlot {
                allocator,
                cmd,
                tables,
                upload,
                signal_value: 0,
            });
        }

        for a in [
            as_allocation(&*vertex_buf),
            as_allocation(&*index_buf),
            as_allocation(&*readback_buf),
        ] {
            residency.addAllocation(a);
        }
        residency.addAllocation(as_allocation(&*pattern_tex));
        residency.commit();
        queue.addResidencySet(&residency);
        queue.addResidencySet(&layer.residencySet());

        let plan = frame_graph::plan(frame_passes(false), &[]).map_err(|e| e.to_string())?;
        let plan_readback =
            frame_graph::plan(frame_passes(true), &[]).map_err(|e| e.to_string())?;

        Ok(Renderer {
            info,
            device,
            queue,
            compiler,
            layer,
            residency,
            event,
            depth_state,
            slots,
            pipelines,
            targets: None,
            pattern_tex,
            vertex_buf,
            index_buf,
            index_count: indices.len(),
            readback_buf,
            plan,
            plan_readback,
            frame_index: 0,
            retired: Vec::new(),
            pending_readback: None,
            completed_readbacks: Vec::new(),
            shader_dir,
            stats: Stats::default(),
        })
    }

    pub fn size(&self) -> Option<(u32, u32)> {
        self.targets.as_ref().map(|t| (t.width, t.height))
    }

    pub fn pipeline_generation(&self) -> u32 {
        self.pipelines.generation
    }

    pub fn frame_index(&self) -> u64 {
        self.frame_index
    }

    pub fn shader_dir(&self) -> &Path {
        &self.shader_dir
    }

    pub fn plan_summary(&self) -> String {
        let mut s = String::new();
        for (p, b) in self
            .plan_readback
            .passes
            .iter()
            .zip(&self.plan_readback.barriers)
        {
            s += &format!(
                "  {:<9} after {:?} before {:?} {:?}\n",
                p.name, b.after, b.before, b.hazards
            );
        }
        s
    }

    /// Пересоздаёт цели по пиксельному размеру. Старые освобождаются отложенно.
    pub fn resize(&mut self, width: u32, height: u32, scale: f64) {
        self.layer.setContentsScale(scale);
        if width == 0 || height == 0 {
            // Свёрнутое окно: цели оставляем, вывод пропускается.
            if let Some(t) = self.targets.take() {
                self.retire(vec![t.hdr, t.depth], None);
            }
            return;
        }
        if self.size() == Some((width, height)) {
            return;
        }
        self.layer
            .setDrawableSize(objc2_foundation::NSSize::new(width as f64, height as f64));
        let hdr = private_texture(
            &self.device,
            shaders::HDR_FORMAT,
            width,
            height,
            MTLTextureUsage::RenderTarget | MTLTextureUsage::ShaderRead,
            "HDR scene",
        );
        let depth = private_texture(
            &self.device,
            shaders::DEPTH_FORMAT,
            width,
            height,
            MTLTextureUsage::RenderTarget,
            "depth (reversed-Z)",
        );
        self.residency.addAllocation(as_allocation(&*hdr));
        self.residency.addAllocation(as_allocation(&*depth));
        self.residency.commit();
        if let Some(old) = self.targets.replace(Targets {
            width,
            height,
            hdr,
            depth,
        }) {
            self.retire(vec![old.hdr, old.depth], None);
        }
    }

    fn retire(&mut self, textures: Vec<Obj<dyn MTLTexture>>, other: Option<Box<dyn Any>>) {
        // Все уже отправленные кадры могут использовать ресурс.
        self.retired.push(Retired {
            after_value: self.frame_index,
            textures,
            _keep_alive: other,
        });
    }

    /// Компилирует шейдеры заново и подменяет pipeline на границе кадра.
    /// При ошибке текущие pipeline продолжают работать.
    pub fn reload_shaders(&mut self, over: SourceOverride) -> Result<u32, ShaderError> {
        let generation = self.pipelines.generation + 1;
        let fresh = shaders::build_pipelines(
            &self.device,
            &self.compiler,
            &self.shader_dir,
            generation,
            over,
        )?;
        let old = std::mem::replace(&mut self.pipelines, fresh);
        self.retire(Vec::new(), Some(Box::new(old)));
        Ok(generation)
    }

    /// Освобождает ресурсы завершённых кадров и забирает готовый readback.
    pub fn collect_completed(&mut self) {
        let done = self.event.signaledValue();
        let mut keep = Vec::new();
        for r in self.retired.drain(..) {
            if r.after_value <= done {
                for t in &r.textures {
                    self.residency.removeAllocation(as_allocation(&**t));
                }
                if !r.textures.is_empty() {
                    self.residency.commit();
                }
                self.stats.released_deferred += 1;
                drop(r);
            } else {
                keep.push(r);
            }
        }
        self.retired = keep;

        if let Some(p) = &self.pending_readback
            && p.signal_value <= done
        {
            let p = self.pending_readback.take().unwrap();
            self.completed_readbacks.push(self.read_back(p));
        }
    }

    pub fn take_readbacks(&mut self) -> Vec<Readback> {
        std::mem::take(&mut self.completed_readbacks)
    }

    fn read_back(&self, p: PendingReadback) -> Readback {
        let base = self.readback_buf.contents().as_ptr().cast::<u8>();
        // SAFETY: GPU закончил кадр с копированием (проверено по событию),
        // новые записи в буфер не ставятся, пока этот readback не прочитан.
        let bytes = unsafe { std::slice::from_raw_parts(base, RB_BYTES) };
        let pattern = bytes[..RB_PATTERN_BYTES].to_vec();
        let depth = p
            .pixels
            .iter()
            .enumerate()
            .map(|(i, &(x, y))| {
                let o = RB_DEPTH_OFFSET + i * 4;
                (
                    x,
                    y,
                    f32::from_le_bytes(bytes[o..o + 4].try_into().unwrap()),
                )
            })
            .collect();
        let mut probe = [0u32; LAYOUT_PROBE_LEN];
        for (i, v) in probe.iter_mut().enumerate() {
            let o = RB_PROBE_OFFSET + i * 4;
            *v = u32::from_le_bytes(bytes[o..o + 4].try_into().unwrap());
        }
        Readback {
            frame: p.frame,
            pattern_time: p.pattern_time,
            width: p.width,
            height: p.height,
            pattern,
            depth,
            probe,
        }
    }

    /// Ждёт завершения всех отправленных кадров (выход, тесты).
    pub fn wait_idle(&self) {
        let _ = self
            .event
            .waitUntilSignaledValue_timeoutMS(self.frame_index, 2000);
    }

    pub fn render(&mut self, input: &FrameInput) -> FrameOutcome {
        self.collect_completed();
        let Some(targets) = self.targets.as_ref() else {
            return FrameOutcome::SkippedZeroSize;
        };
        let (width, height) = (targets.width, targets.height);
        let slot_index = (self.frame_index % FRAME_SLOTS as u64) as usize;

        // Слот свободен, когда GPU закончил кадр, который его использовал.
        let wait_value = self.slots[slot_index].signal_value;
        if wait_value > self.event.signaledValue()
            && !self
                .event
                .waitUntilSignaledValue_timeoutMS(wait_value, SLOT_WAIT_TIMEOUT_MS)
        {
            self.stats.skipped_busy += 1;
            eprintln!(
                "[renderer] слот {slot_index} занят дольше {SLOT_WAIT_TIMEOUT_MS} мс (ждём {wait_value}, готово {}), кадр пропущен",
                self.event.signaledValue()
            );
            return FrameOutcome::SkippedBusySlot;
        }

        let Some(drawable) = self.layer.nextDrawable() else {
            self.stats.skipped_no_drawable += 1;
            return FrameOutcome::SkippedNoDrawable;
        };

        let readback = input.readback && self.pending_readback.is_none();
        let capturing = input
            .capture
            .as_ref()
            .map(|path| self.start_capture(path))
            .unwrap_or(false);
        let encode_start = std::time::Instant::now();

        let slot = &self.slots[slot_index];
        slot.allocator.reset();
        slot.cmd.beginCommandBufferWithAllocator(&slot.allocator);
        slot.cmd
            .pushDebugGroup(&label(&format!("frame {}", self.frame_index)));

        let mut uniforms = input.uniforms;
        uniforms.frame_index = self.frame_index as u32;
        upload_bytes(&slot.upload, 0, bytemuck::bytes_of(&uniforms));
        let instances = &input.instances[..input.instances.len().min(MAX_INSTANCES)];
        upload_bytes(
            &slot.upload,
            INSTANCE_OFFSET,
            bytemuck::cast_slice(instances),
        );
        let upload_addr = slot.upload.gpuAddress();

        let plan = if readback {
            &self.plan_readback
        } else {
            &self.plan
        };
        let mut pixels = Vec::new();
        for (pass, barrier) in plan.passes.iter().zip(&plan.barriers) {
            let after = to_mtl_stages(barrier.after);
            let before = to_mtl_stages(barrier.before);
            match pass.name {
                "pattern" => {
                    let enc = slot.cmd.computeCommandEncoder().expect("compute encoder");
                    enc.setLabel(Some(&label("pattern compute")));
                    if !barrier.after.is_empty() {
                        enc.barrierAfterQueueStages_beforeStages_visibilityOptions(
                            after,
                            before,
                            MTL4VisibilityOptions::Device,
                        );
                    }
                    let table = &slot.tables[0];
                    unsafe {
                        table.setTexture_atIndex(self.pattern_tex.gpuResourceID(), 0);
                        table.setAddress_atIndex(upload_addr, 0);
                    }
                    enc.setComputePipelineState(&self.pipelines.pattern);
                    enc.setArgumentTable(Some(table));
                    enc.dispatchThreads_threadsPerThreadgroup(
                        MTLSize {
                            width: PATTERN_SIZE as usize,
                            height: PATTERN_SIZE as usize,
                            depth: 1,
                        },
                        MTLSize {
                            width: 16,
                            height: 16,
                            depth: 1,
                        },
                    );
                    enc.endEncoding();
                }
                "scene" => {
                    let rp = MTL4RenderPassDescriptor::new();
                    let color = unsafe { rp.colorAttachments().objectAtIndexedSubscript(0) };
                    color.setTexture(Some(&targets.hdr));
                    color.setLoadAction(MTLLoadAction::Clear);
                    color.setStoreAction(MTLStoreAction::Store);
                    color.setClearColor(MTLClearColor {
                        red: 0.035,
                        green: 0.045,
                        blue: 0.06,
                        alpha: 1.0,
                    });
                    let depth = rp.depthAttachment();
                    depth.setTexture(Some(&targets.depth));
                    depth.setLoadAction(MTLLoadAction::Clear);
                    depth.setClearDepth(0.0);
                    depth.setStoreAction(if readback {
                        MTLStoreAction::Store
                    } else {
                        MTLStoreAction::DontCare
                    });
                    let enc = slot
                        .cmd
                        .renderCommandEncoderWithDescriptor(&rp)
                        .expect("render encoder");
                    enc.setLabel(Some(&label("scene raster")));
                    if !barrier.after.is_empty() {
                        enc.barrierAfterQueueStages_beforeStages_visibilityOptions(
                            after,
                            before,
                            MTL4VisibilityOptions::Device,
                        );
                    }
                    let table = &slot.tables[1];
                    unsafe {
                        table.setAddress_atIndex(self.vertex_buf.gpuAddress(), 0);
                        table.setAddress_atIndex(upload_addr, 1);
                        table.setAddress_atIndex(upload_addr + INSTANCE_OFFSET as u64, 2);
                        table.setTexture_atIndex(self.pattern_tex.gpuResourceID(), 0);
                    }
                    enc.setRenderPipelineState(&self.pipelines.scene);
                    enc.setDepthStencilState(Some(&self.depth_state));
                    enc.setFrontFacingWinding(MTLWinding::CounterClockwise);
                    enc.setCullMode(MTLCullMode::Back);
                    enc.setViewport(MTLViewport {
                        originX: 0.0,
                        originY: 0.0,
                        width: width as f64,
                        height: height as f64,
                        znear: 0.0,
                        zfar: 1.0,
                    });
                    enc.setArgumentTable_atStages(
                        table,
                        MTLRenderStages::Vertex | MTLRenderStages::Fragment,
                    );
                    if !instances.is_empty() {
                        unsafe {
                            enc.drawIndexedPrimitives_indexCount_indexType_indexBuffer_indexBufferLength_instanceCount(
                                MTLPrimitiveType::Triangle,
                                self.index_count,
                                MTLIndexType::UInt16,
                                self.index_buf.gpuAddress(),
                                self.index_buf.length(),
                                instances.len(),
                            );
                        }
                    }
                    enc.endEncoding();
                }
                "readback" => {
                    let enc = slot.cmd.computeCommandEncoder().expect("compute encoder");
                    enc.setLabel(Some(&label("readback copy")));
                    enc.barrierAfterQueueStages_beforeStages_visibilityOptions(
                        after,
                        before,
                        MTL4VisibilityOptions::Device,
                    );
                    let table = &slot.tables[2];
                    unsafe {
                        table.setAddress_atIndex(
                            self.readback_buf.gpuAddress() + RB_PROBE_OFFSET as u64,
                            0,
                        )
                    };
                    enc.setComputePipelineState(&self.pipelines.layout_probe);
                    enc.setArgumentTable(Some(table));
                    enc.dispatchThreads_threadsPerThreadgroup(
                        MTLSize {
                            width: 1,
                            height: 1,
                            depth: 1,
                        },
                        MTLSize {
                            width: 1,
                            height: 1,
                            depth: 1,
                        },
                    );
                    unsafe {
                        enc.copyFromTexture_sourceSlice_sourceLevel_sourceOrigin_sourceSize_toBuffer_destinationOffset_destinationBytesPerRow_destinationBytesPerImage(
                            &self.pattern_tex,
                            0,
                            0,
                            MTLOrigin { x: 0, y: 0, z: 0 },
                            MTLSize { width: PATTERN_SIZE as usize, height: PATTERN_SIZE as usize, depth: 1 },
                            &self.readback_buf,
                            0,
                            (PATTERN_SIZE * 4) as usize,
                            RB_PATTERN_BYTES,
                        );
                    }
                    for (i, (fx, fy)) in DEPTH_SAMPLES.iter().enumerate() {
                        let x = ((width as f32 * fx) as u32).min(width - 1);
                        let y = ((height as f32 * fy) as u32).min(height - 1);
                        pixels.push((x, y));
                        unsafe {
                            enc.copyFromTexture_sourceSlice_sourceLevel_sourceOrigin_sourceSize_toBuffer_destinationOffset_destinationBytesPerRow_destinationBytesPerImage(
                                &targets.depth,
                                0,
                                0,
                                MTLOrigin { x: x as usize, y: y as usize, z: 0 },
                                MTLSize { width: 1, height: 1, depth: 1 },
                                &self.readback_buf,
                                RB_DEPTH_OFFSET + i * 4,
                                4,
                                4,
                            );
                        }
                    }
                    enc.endEncoding();
                }
                "tonemap" => {
                    let rp = MTL4RenderPassDescriptor::new();
                    let color = unsafe { rp.colorAttachments().objectAtIndexedSubscript(0) };
                    color.setTexture(Some(&drawable.texture()));
                    color.setLoadAction(MTLLoadAction::DontCare);
                    color.setStoreAction(MTLStoreAction::Store);
                    let enc = slot
                        .cmd
                        .renderCommandEncoderWithDescriptor(&rp)
                        .expect("render encoder");
                    enc.setLabel(Some(&label("tone map")));
                    if !barrier.after.is_empty() {
                        enc.barrierAfterQueueStages_beforeStages_visibilityOptions(
                            after,
                            before,
                            MTL4VisibilityOptions::Device,
                        );
                    }
                    let table = &slot.tables[3];
                    unsafe {
                        table.setAddress_atIndex(upload_addr, 1);
                        table.setTexture_atIndex(targets.hdr.gpuResourceID(), 0);
                    }
                    enc.setRenderPipelineState(&self.pipelines.tonemap);
                    enc.setArgumentTable_atStages(
                        table,
                        MTLRenderStages::Vertex | MTLRenderStages::Fragment,
                    );
                    unsafe {
                        enc.drawPrimitives_vertexStart_vertexCount(MTLPrimitiveType::Triangle, 0, 3)
                    };
                    enc.endEncoding();
                }
                other => unreachable!("проход {other} без кодировщика"),
            }
        }

        slot.cmd.popDebugGroup();
        slot.cmd.endCommandBuffer();

        // Схема Apple для Metal 4: ждать drawable → commit → сигнал drawable → present.
        let mtl_drawable: &ProtocolObject<dyn MTLDrawable> = ProtocolObject::from_ref(&*drawable);
        self.queue.waitForDrawable(mtl_drawable);
        let mut cmd_ptr = NonNull::from(&*slot.cmd);
        unsafe { self.queue.commit_count(NonNull::from(&mut cmd_ptr), 1) };
        self.queue.signalDrawable(mtl_drawable);
        mtl_drawable.present();

        self.frame_index += 1;
        let event: &ProtocolObject<dyn MTLEvent> = ProtocolObject::from_ref(&*self.event);
        self.queue.signalEvent_value(event, self.frame_index);
        self.slots[slot_index].signal_value = self.frame_index;
        self.stats.cpu_encode_ms = encode_start.elapsed().as_secs_f64() * 1000.0;
        self.stats.presented += 1;

        if readback {
            self.pending_readback = Some(PendingReadback {
                signal_value: self.frame_index,
                frame: self.frame_index - 1,
                pattern_time: input.uniforms.pattern_time,
                width,
                height,
                pixels,
            });
        }
        if capturing {
            unsafe { MTLCaptureManager::sharedCaptureManager() }.stopCapture();
            eprintln!("[capture] кадр {} записан", self.frame_index - 1);
        }
        FrameOutcome::Presented
    }

    fn start_capture(&self, path: &Path) -> bool {
        let manager = unsafe { MTLCaptureManager::sharedCaptureManager() };
        if !manager.supportsDestination(MTLCaptureDestination::GPUTraceDocument) {
            eprintln!("[capture] запись в .gputrace недоступна: запусти с MTL_CAPTURE_ENABLED=1");
            return false;
        }
        let desc = MTLCaptureDescriptor::new();
        let device: &AnyObject = self.device.as_ref();
        unsafe { desc.setCaptureObject(Some(device)) };
        desc.setDestination(MTLCaptureDestination::GPUTraceDocument);
        let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
        desc.setOutputURL(Some(&url));
        match manager.startCaptureWithDescriptor_error(&desc) {
            Ok(()) => true,
            Err(e) => {
                eprintln!("[capture] ошибка: {}", e.localizedDescription());
                false
            }
        }
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        self.wait_idle();
    }
}
