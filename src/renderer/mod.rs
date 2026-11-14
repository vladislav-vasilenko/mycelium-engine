//! Конкретный рендерер на Metal 4 — фасад над модулями бэкенда.
//!
//! Одна очередь `MTL4CommandQueue` для compute, render и copy. Два слота кадра,
//! у каждого свой `MTL4CommandAllocator`, командный буфер, таблицы аргументов
//! и upload-буфер в `StorageModeShared`. Завершение кадра отмечает `MTLSharedEvent`.
//! Все нативные типы и `unsafe` вызовы Metal остаются в этом модуле и его
//! подмодулях:
//!
//! - `device` — проверка устройства и GPU capture;
//! - `resources` — буферы, текстуры, загрузка, отложенное освобождение;
//! - `frame` — вход и итог кадра, слоты, состав проходов;
//! - `passes` — проходы как объекты за трейтом `FramePass`;
//! - `readback` — раскладка и разбор копий для самопроверки;
//! - `targets`, `terrain`, `water`, `hands` — цели рендера и ресурсы сцены.

mod device;
mod frame;
mod hands;
mod passes;
mod readback;
mod resources;
pub mod shaders;
mod targets;
mod terrain;
mod water;

use std::any::Any;
use std::path::{Path, PathBuf};
use std::ptr::NonNull;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_metal::*;
use objc2_quartz_core::{CAMetalDrawable, CAMetalLayer};

pub use device::{DeviceInfo, probe_device};
pub use frame::{FrameInput, FrameOutcome, Stats, WaterFrame};
pub use hands::{HandsFrame, MAX_JOINTS};
pub use readback::Readback;
pub use terrain::TerrainTextures;

use crate::frame_graph::{self, FramePlan};
use crate::layout::Vertex;
use crate::reference::PATTERN_SIZE;
use frame::{
    FRAME_SLOTS, FrameSlot, HANDS_OFFSET, INSTANCE_OFFSET, LEVELS_OFFSET, MAX_INSTANCES,
    PALETTE_OFFSET, SEA_OFFSET, SLOT_WAIT_TIMEOUT_MS, frame_passes,
};
use hands::HandsGpu;
use passes::{PassBarrier, PassCtx};
use readback::{PendingReadback, RB_BYTES};
use resources::{
    DeferredRelease, TextureUpload, as_allocation, buffer_with, label, private_texture,
    shared_buffer, to_mtl_stages, upload_bytes,
};
use shaders::{Obj, Pipelines, ShaderError, SourceOverride};
use targets::Targets;
use terrain::TerrainGpu;
use water::WaterGpu;

pub struct Renderer {
    pub info: DeviceInfo,
    device: Obj<dyn MTLDevice>,
    queue: Obj<dyn MTL4CommandQueue>,
    compiler: Obj<dyn MTL4Compiler>,
    layer: Retained<CAMetalLayer>,
    residency: Obj<dyn MTLResidencySet>,
    event: Obj<dyn MTLSharedEvent>,
    depth_state: Obj<dyn MTLDepthStencilState>,
    /// Глубина для прозрачной воды: проверка без записи.
    water_depth_state: Obj<dyn MTLDepthStencilState>,
    slots: Vec<FrameSlot>,
    pipelines: Pipelines,
    targets: Option<Targets>,
    pattern_tex: Obj<dyn MTLTexture>,
    vertex_buf: Obj<dyn MTLBuffer>,
    index_buf: Obj<dyn MTLBuffer>,
    index_count: usize,
    readback_buf: Obj<dyn MTLBuffer>,
    /// Планы по составу предыдущего и текущего кадра: `plans[prev][cur]`,
    /// индекс 1 — кадр с readback.
    plans: [[FramePlan; 2]; 2],
    /// Был ли readback в последнем отправленном кадре.
    prev_readback: bool,
    frame_index: u64,
    retired: DeferredRelease,
    pending_readback: Option<PendingReadback>,
    completed_readbacks: Vec<Readback>,
    shader_dir: PathBuf,
    terrain: Option<TerrainGpu>,
    water: Option<WaterGpu>,
    hands: Option<HandsGpu>,
    pub stats: Stats,
}

fn depth_state(
    device: &ProtocolObject<dyn MTLDevice>,
    write: bool,
    name: &str,
) -> Result<Obj<dyn MTLDepthStencilState>, String> {
    let d = MTLDepthStencilDescriptor::new();
    d.setDepthCompareFunction(MTLCompareFunction::GreaterEqual);
    d.setDepthWriteEnabled(write);
    d.setLabel(Some(&label(name)));
    device
        .newDepthStencilStateWithDescriptor(&d)
        .ok_or_else(|| format!("depth state {name}"))
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

        let depth_state = depth_state(&device, true, "reversed-Z GreaterEqual")?;
        let water_depth_state = depth_state_no_write(&device, "reversed-Z test, no write (water)")?;

        let shader_dir = shaders::shader_dir();
        let pipelines = shaders::build_pipelines(&device, &compiler, &shader_dir, 1, None)
            .map_err(|e| e.to_string())?;

        let vertex_buf = buffer_with(&device, bytemuck::cast_slice(vertices), "cube vertices");
        let index_buf = buffer_with(&device, bytemuck::cast_slice(indices), "cube indices");
        let readback_buf = shared_buffer(&device, RB_BYTES, "readback");
        let pattern_tex = private_texture(
            &device,
            shaders::PATTERN_FORMAT,
            PATTERN_SIZE,
            PATTERN_SIZE,
            MTLTextureUsage::ShaderRead | MTLTextureUsage::ShaderWrite,
            "pattern (compute → fragment)",
        );

        let slots = (0..FRAME_SLOTS)
            .map(|i| FrameSlot::new(&device, &residency, i))
            .collect::<Result<Vec<_>, _>>()?;

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

        // Барьеры зависят и от состава предыдущего кадра: после кадра с readback
        // запись pattern и depth должна дождаться его копий в стадии blit.
        let build = |prev: bool, cur: bool| {
            frame_graph::plan_after(&frame_passes(prev), frame_passes(cur), &[])
                .map_err(|e| e.to_string())
        };
        let plans = [
            [build(false, false)?, build(false, true)?],
            [build(true, false)?, build(true, true)?],
        ];

        Ok(Renderer {
            info,
            device,
            queue,
            compiler,
            layer,
            residency,
            event,
            depth_state,
            water_depth_state,
            slots,
            pipelines,
            targets: None,
            pattern_tex,
            vertex_buf,
            index_buf,
            index_count: indices.len(),
            readback_buf,
            plans,
            prev_readback: false,
            frame_index: 0,
            retired: DeferredRelease::default(),
            pending_readback: None,
            completed_readbacks: Vec::new(),
            shader_dir,
            terrain: None,
            water: None,
            hands: None,
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
        for (title, prev, cur) in [
            ("кадр с readback после такого же", 1, 1),
            ("обычный кадр после кадра с readback", 1, 0),
        ] {
            s += &format!("  [{title}]\n");
            let plan = &self.plans[prev][cur];
            for (p, b) in plan.passes.iter().zip(&plan.barriers) {
                s += &format!(
                    "  {:<9} after {:?} before {:?} {:?}\n",
                    p.name, b.after, b.before, b.hazards
                );
            }
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
        let fresh = Targets::new(&self.device, width, height);
        self.residency.addAllocation(as_allocation(&*fresh.hdr));
        self.residency.addAllocation(as_allocation(&*fresh.depth));
        self.residency.commit();
        if let Some(old) = self.targets.replace(fresh) {
            self.retire(vec![old.hdr, old.depth], None);
        }
    }

    /// Освобождение после всех уже отправленных кадров.
    fn retire(&mut self, textures: Vec<Obj<dyn MTLTexture>>, other: Option<Box<dyn Any>>) {
        self.retired.push(self.frame_index, textures, other);
    }

    fn upload_textures(&self, uploads: &[TextureUpload]) -> Result<(), String> {
        resources::upload_textures(&self.device, &self.queue, &self.residency, uploads)
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
        self.stats.released_deferred += self.retired.collect(done, &self.residency);
        if let Some(p) = &self.pending_readback
            && p.signal_value <= done
        {
            let p = self.pending_readback.take().unwrap();
            let base = self.readback_buf.contents().as_ptr().cast::<u8>();
            // SAFETY: GPU закончил кадр с копированием (проверено по событию),
            // новые записи в буфер не ставятся, пока этот readback не прочитан.
            let bytes = unsafe { std::slice::from_raw_parts(base, RB_BYTES) };
            self.completed_readbacks.push(p.decode(bytes));
        }
    }

    pub fn take_readbacks(&mut self) -> Vec<Readback> {
        std::mem::take(&mut self.completed_readbacks)
    }

    /// Ждёт завершения всех отправленных кадров (выход, тесты).
    pub fn wait_idle(&self) {
        let _ = self
            .event
            .waitUntilSignaledValue_timeoutMS(self.frame_index, 2000);
    }

    /// Кадр: ждёт свободный слот и drawable, записывает upload, кодирует
    /// проходы плана с барьерами, отправляет и показывает.
    pub fn render(&mut self, input: &FrameInput) -> FrameOutcome {
        self.collect_completed();
        let Some((width, height)) = self.size() else {
            return FrameOutcome::SkippedZeroSize;
        };
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
            .is_some_and(|path| device::start_capture(&self.device, path));
        let encode_start = std::time::Instant::now();
        self.encode_frame(slot_index, &drawable, input, readback);

        // Схема Apple для Metal 4: ждать drawable → commit → сигнал drawable → present.
        let slot = &self.slots[slot_index];
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
        self.prev_readback = readback;
        self.stats.cpu_encode_ms = encode_start.elapsed().as_secs_f64() * 1000.0;
        self.stats.presented += 1;

        if readback {
            self.pending_readback = Some(PendingReadback {
                signal_value: self.frame_index,
                frame: self.frame_index - 1,
                pattern_time: input.uniforms.pattern_time,
                width,
                height,
                pixels: readback::sample_pixels(width, height),
            });
        }
        if capturing {
            device::stop_capture(self.frame_index - 1);
        }
        FrameOutcome::Presented
    }

    /// Записывает upload слота и кодирует все проходы плана в его командный буфер.
    fn encode_frame(
        &self,
        slot_index: usize,
        drawable: &ProtocolObject<dyn CAMetalDrawable>,
        input: &FrameInput,
        readback: bool,
    ) {
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
        if let Some(w) = &input.water {
            upload_bytes(&slot.upload, SEA_OFFSET, bytemuck::bytes_of(&w.params));
            upload_bytes(&slot.upload, LEVELS_OFFSET, bytemuck::cast_slice(&w.levels));
        }
        if let Some(h) = &input.hands {
            let n = h.palette.len().min(MAX_JOINTS);
            let mut u = h.uniforms;
            u.joint_count = n as u32;
            upload_bytes(&slot.upload, HANDS_OFFSET, bytemuck::bytes_of(&u));
            upload_bytes(
                &slot.upload,
                PALETTE_OFFSET,
                bytemuck::cast_slice(&h.palette[..n]),
            );
        }
        let drawable_tex = drawable.texture();
        let ctx = PassCtx {
            r: self,
            slot,
            upload_addr: slot.upload.gpuAddress(),
            targets: self.targets.as_ref().expect("цели есть: размер проверен"),
            drawable: &drawable_tex,
            input,
            instance_count: instances.len(),
            readback,
            water: match (&input.water, &self.water, &self.terrain) {
                (Some(_), Some(w), Some(t)) => Some((w, t)),
                _ => None,
            },
        };
        let plan = &self.plans[usize::from(self.prev_readback)][usize::from(readback)];
        for (pass, barrier) in plan.passes.iter().zip(&plan.barriers) {
            let barrier = PassBarrier {
                after: to_mtl_stages(barrier.after),
                before: to_mtl_stages(barrier.before),
                needed: !barrier.after.is_empty(),
            };
            passes::find(pass.name).encode(&ctx, &barrier);
        }
        slot.cmd.popDebugGroup();
        slot.cmd.endCommandBuffer();
    }
}

fn depth_state_no_write(
    device: &ProtocolObject<dyn MTLDevice>,
    name: &str,
) -> Result<Obj<dyn MTLDepthStencilState>, String> {
    depth_state(device, false, name)
}

impl Drop for Renderer {
    fn drop(&mut self) {
        self.wait_idle();
    }
}
