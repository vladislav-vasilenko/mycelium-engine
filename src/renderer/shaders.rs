//! Загрузка MSL из `shaders/metal/` и сборка pipeline через `MTL4Compiler`.
//!
//! В Debug исходники компилируются в рантайме. Ошибка компиляции не трогает
//! рабочие pipeline: вызывающий код подменяет их только после полного успеха.

use std::path::{Path, PathBuf};

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_foundation::NSString;
use objc2_metal::*;

pub type Obj<T> = Retained<ProtocolObject<T>>;

pub const HDR_FORMAT: MTLPixelFormat = MTLPixelFormat::RGBA16Float;
pub const DEPTH_FORMAT: MTLPixelFormat = MTLPixelFormat::Depth32Float;
pub const DRAWABLE_FORMAT: MTLPixelFormat = MTLPixelFormat::BGRA8Unorm_sRGB;
pub const PATTERN_FORMAT: MTLPixelFormat = MTLPixelFormat::RGBA8Unorm;

/// Смешивание цели рендера.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Blend {
    Opaque,
    /// Вода: `цвет + dst × пропускание` (dual-source).
    Transmit,
    /// Звёзды: `dst + цвет`.
    Additive,
    /// Интерфейс: предумноженная альфа, `цвет + dst × (1 − α)`.
    Alpha,
}

pub struct Pipelines {
    pub pattern: Obj<dyn MTLComputePipelineState>,
    pub layout_probe: Obj<dyn MTLComputePipelineState>,
    pub scene: Obj<dyn MTLRenderPipelineState>,
    pub tonemap: Obj<dyn MTLRenderPipelineState>,
    /// Интерфейс поверх кадра (05.1).
    pub ui: Obj<dyn MTLRenderPipelineState>,
    pub terrain: Obj<dyn MTLRenderPipelineState>,
    /// Кластерный рельеф (02.3): тот же фрагментный шейдер, вершины из поля высот.
    pub terrain_clusters: Obj<dyn MTLRenderPipelineState>,
    /// Детальная заплатка у камеры (02.10): `terrain_patch_vs` + `terrain_fs`.
    pub terrain_patch: Obj<dyn MTLRenderPipelineState>,
    /// Вода: dual-source смешивание `dst × пропускание + цвет`.
    pub water: Obj<dyn MTLRenderPipelineState>,
    pub wet_sand: Obj<dyn MTLComputePipelineState>,
    pub water_probe: Obj<dyn MTLComputePipelineState>,
    /// Руки со скином (проход `viewmodel`) и сверка скиннинга с CPU.
    pub hands: Obj<dyn MTLRenderPipelineState>,
    pub skin_probe: Obj<dyn MTLComputePipelineState>,
    /// Небо (04.1): LUT, свет неба, проход неба.
    pub sky_transmittance: Obj<dyn MTLComputePipelineState>,
    pub sky_multiscatter: Obj<dyn MTLComputePipelineState>,
    pub sky_view: Obj<dyn MTLComputePipelineState>,
    pub sky_view_moon: Obj<dyn MTLComputePipelineState>,
    /// Лес (02.4): растения, их тени, расстановка и indirect-аргументы.
    pub plant_shadow: Obj<dyn MTLRenderPipelineState>,
    /// Предпроход глубины растений и затенение после него (02.13).
    pub plant_depth: Obj<dyn MTLRenderPipelineState>,
    pub plant_prepassed: Obj<dyn MTLRenderPipelineState>,
    pub forest_place: Obj<dyn MTLComputePipelineState>,
    pub forest_finish: Obj<dyn MTLComputePipelineState>,
    /// Вода на земле (04.3, сессия 6).
    pub ground_water: Obj<dyn MTLComputePipelineState>,
    /// Выбор кластеров на GPU (02.5): глубина дерева и indirect-аргументы.
    pub cull_clusters: Obj<dyn MTLComputePipelineState>,
    pub cull_finish: Obj<dyn MTLComputePipelineState>,
    /// Временное сглаживание (04.5).
    pub taa_resolve: Obj<dyn MTLComputePipelineState>,
    /// Подготовка апскейла MetalFX (04.6): объёмный свет и векторы движения.
    pub upscale_prep: Obj<dyn MTLComputePipelineState>,
    /// Капли дождя (04.3, сессия 3).
    pub rain: Obj<dyn MTLRenderPipelineState>,
    /// Брызги от ударов капель (04.3, сессия 3).
    pub rain_splash: Obj<dyn MTLRenderPipelineState>,
    /// Поверхность рек (03.2, сессия 3).
    pub river: Obj<dyn MTLRenderPipelineState>,
    /// Рыба над рекой (03.2).
    pub fish: Obj<dyn MTLRenderPipelineState>,
    /// Поверхность реки в карте укрытия от дождя (03.2).
    pub river_rain: Obj<dyn MTLRenderPipelineState>,
    /// Водяная пыль водопадов (03.5).
    pub mist: Obj<dyn MTLRenderPipelineState>,
    /// Bloom (04.5): цепочки вниз и вверх.
    pub bloom_down: Obj<dyn MTLComputePipelineState>,
    pub bloom_up: Obj<dyn MTLComputePipelineState>,
    /// Сцена для лучей (11).
    pub rt_terrain_grid: Obj<dyn MTLComputePipelineState>,
    pub rt_instances: Obj<dyn MTLComputePipelineState>,
    pub rt_count_finish: Obj<dyn MTLComputePipelineState>,
    pub rt_ao: Obj<dyn MTLComputePipelineState>,
    /// Пробы освещённости (11, сессия 3).
    pub probe_update: Obj<dyn MTLComputePipelineState>,
    /// Объёмный свет (11, сессия 2).
    pub volumetric_light: Obj<dyn MTLComputePipelineState>,
    /// LUT воздушной перспективы (04.2).
    pub sky_aerial: Obj<dyn MTLComputePipelineState>,
    pub sky_ambient: Obj<dyn MTLComputePipelineState>,
    /// Объёмные облака (04.3, сессия 4б).
    pub clouds_march: Obj<dyn MTLComputePipelineState>,
    pub sky: Obj<dyn MTLRenderPipelineState>,
    pub exposure_meter: Obj<dyn MTLComputePipelineState>,
    /// Глубина кластеров рельефа в каскады теней (04.2), без фрагментного шейдера.
    pub terrain_shadow: Obj<dyn MTLRenderPipelineState>,
    /// Звёзды точками с аддитивным смешиванием (04.1).
    pub stars: Obj<dyn MTLRenderPipelineState>,
    /// Счётчик поколений для диагностики после перезагрузки.
    pub generation: u32,
}

#[derive(Debug, Clone)]
pub struct ShaderError {
    pub file: String,
    pub stage: &'static str,
    pub message: String,
}

impl std::fmt::Display for ShaderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} [{}]: {}", self.file, self.stage, self.message)
    }
}

/// Исходники можно подменить в памяти (самопроверка ошибочной правки).
pub type SourceOverride<'a> = Option<(&'a str, &'a str)>;

/// Каталог MSL. Порядок поиска:
/// 1. `ECHO_SHADER_DIR` — явное переопределение;
/// 2. `Echo.app/Contents/Resources/shaders/metal` — запуск из бандла;
/// 3. `shaders/metal` рядом с `Cargo.toml` — запуск через cargo.
pub fn shader_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("ECHO_SHADER_DIR") {
        return PathBuf::from(dir);
    }
    if let Some(dir) = bundle_shader_dir() {
        return dir;
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("shaders/metal")
}

/// `<exe>/../../Resources/shaders/metal`, если исполняемый файл лежит
/// в `Contents/MacOS` бандла и каталог существует.
fn bundle_shader_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let macos = exe.parent()?;
    if macos.file_name()? != "MacOS" {
        return None;
    }
    let dir = macos.parent()?.join("Resources/shaders/metal");
    dir.is_dir().then_some(dir)
}

fn read_source(dir: &Path, file: &str, over: SourceOverride) -> Result<String, ShaderError> {
    let err = |stage, message: String| ShaderError {
        file: file.to_string(),
        stage,
        message,
    };
    let text = match over {
        Some((name, text)) if name == file => text.to_string(),
        _ => std::fs::read_to_string(dir.join(file)).map_err(|e| err("read", e.to_string()))?,
    };
    expand_includes(dir, &text, 0)
}

/// Metal API не знает путей проекта, поэтому `#include "файл.h"` из каталога
/// шейдеров подставляем сами, рекурсивно и не больше одного раза на файл.
fn expand_includes(dir: &Path, text: &str, depth: u32) -> Result<String, ShaderError> {
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        let name = line
            .trim()
            .strip_prefix("#include \"")
            .and_then(|r| r.strip_suffix('"'));
        match name {
            Some(name) if depth < 4 => {
                let inc = std::fs::read_to_string(dir.join(name)).map_err(|e| ShaderError {
                    file: name.to_string(),
                    stage: "read",
                    message: e.to_string(),
                })?;
                out.push_str(&expand_includes(dir, &inc, depth + 1)?);
            }
            _ => out.push_str(line),
        }
        out.push('\n');
    }
    Ok(out)
}

fn compile_library(
    device: &ProtocolObject<dyn MTLDevice>,
    dir: &Path,
    file: &str,
    over: SourceOverride,
) -> Result<Obj<dyn MTLLibrary>, ShaderError> {
    let source = read_source(dir, file, over)?;
    let options = MTLCompileOptions::new();
    // Инвариантная позиция растений (предпроход глубины, 02.13) — бит в бит.
    if file == "forest.metal" {
        options.setPreserveInvariance(true);
    }
    device
        .newLibraryWithSource_options_error(&NSString::from_str(&source), Some(&options))
        .map_err(|e| ShaderError {
            file: file.to_string(),
            stage: "compile MSL",
            message: e.localizedDescription().to_string(),
        })
}

fn function(
    lib: &ProtocolObject<dyn MTLLibrary>,
    name: &str,
) -> Retained<MTL4LibraryFunctionDescriptor> {
    let f = MTL4LibraryFunctionDescriptor::new();
    f.setName(Some(&NSString::from_str(name)));
    f.setLibrary(Some(lib));
    f
}

fn require_function(
    lib: &ProtocolObject<dyn MTLLibrary>,
    file: &str,
    name: &str,
) -> Result<(), ShaderError> {
    let names = lib.functionNames();
    if names.iter().any(|n| n.to_string() == name) {
        Ok(())
    } else {
        Err(ShaderError {
            file: file.to_string(),
            stage: "entry point",
            message: format!("функция `{name}` не найдена"),
        })
    }
}

fn compute_pipeline(
    compiler: &ProtocolObject<dyn MTL4Compiler>,
    lib: &ProtocolObject<dyn MTLLibrary>,
    file: &str,
    name: &str,
) -> Result<Obj<dyn MTLComputePipelineState>, ShaderError> {
    require_function(lib, file, name)?;
    let desc = MTL4ComputePipelineDescriptor::new();
    desc.setLabel(Some(&NSString::from_str(name)));
    desc.setComputeFunctionDescriptor(Some(&function(lib, name)));
    compiler
        .newComputePipelineStateWithDescriptor_compilerTaskOptions_error(&desc, None)
        .map_err(|e| ShaderError {
            file: file.to_string(),
            stage: "compute pipeline",
            message: e.localizedDescription().to_string(),
        })
}

/// Конвейер только глубины: вершинный шейдер, без цвета и фрагментного шейдера.
fn depth_only_pipeline(
    compiler: &ProtocolObject<dyn MTL4Compiler>,
    lib: &ProtocolObject<dyn MTLLibrary>,
    file: &str,
    label: &str,
    vs: &str,
    fs: Option<&str>,
) -> Result<Obj<dyn MTLRenderPipelineState>, ShaderError> {
    require_function(lib, file, vs)?;
    let desc = MTL4RenderPipelineDescriptor::new();
    desc.setLabel(Some(&NSString::from_str(label)));
    desc.setVertexFunctionDescriptor(Some(&function(lib, vs)));
    // Фрагментный шейдер без цвета — только отсечение по альфе (листва в тенях).
    if let Some(fs) = fs {
        require_function(lib, file, fs)?;
        desc.setFragmentFunctionDescriptor(Some(&function(lib, fs)));
    }
    compiler
        .newRenderPipelineStateWithDescriptor_compilerTaskOptions_error(&desc, None)
        .map_err(|e| ShaderError {
            file: file.to_string(),
            stage: "render pipeline",
            message: e.localizedDescription().to_string(),
        })
}

#[allow(clippy::too_many_arguments)]
fn render_pipeline(
    compiler: &ProtocolObject<dyn MTL4Compiler>,
    lib: &ProtocolObject<dyn MTLLibrary>,
    file: &str,
    label: &str,
    (vs, fs): (&str, &str),
    color_format: MTLPixelFormat,
    blend: Blend,
    motion: Motion,
) -> Result<Obj<dyn MTLRenderPipelineState>, ShaderError> {
    require_function(lib, file, vs)?;
    require_function(lib, file, fs)?;
    let desc = MTL4RenderPipelineDescriptor::new();
    desc.setLabel(Some(&NSString::from_str(label)));
    desc.setVertexFunctionDescriptor(Some(&function(lib, vs)));
    desc.setFragmentFunctionDescriptor(Some(&function(lib, fs)));
    let color = unsafe { desc.colorAttachments().objectAtIndexedSubscript(0) };
    color.setPixelFormat(color_format);
    set_motion_attachment(&desc, motion);
    if blend == Blend::Additive {
        // Звёзды: свет добавляется к небу.
        color.setBlendingState(MTL4BlendState::Enabled);
        color.setSourceRGBBlendFactor(MTLBlendFactor::One);
        color.setDestinationRGBBlendFactor(MTLBlendFactor::One);
        color.setRgbBlendOperation(MTLBlendOperation::Add);
        color.setSourceAlphaBlendFactor(MTLBlendFactor::Zero);
        color.setDestinationAlphaBlendFactor(MTLBlendFactor::One);
        color.setAlphaBlendOperation(MTLBlendOperation::Add);
    }
    if blend == Blend::Alpha {
        color.setBlendingState(MTL4BlendState::Enabled);
        color.setSourceRGBBlendFactor(MTLBlendFactor::One);
        color.setDestinationRGBBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
        color.setRgbBlendOperation(MTLBlendOperation::Add);
        color.setSourceAlphaBlendFactor(MTLBlendFactor::One);
        color.setDestinationAlphaBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
        color.setAlphaBlendOperation(MTLBlendOperation::Add);
    }
    if blend == Blend::Transmit {
        // Прозрачная вода: результат = цвет (index 0) + dst × пропускание (index 1).
        color.setBlendingState(MTL4BlendState::Enabled);
        color.setSourceRGBBlendFactor(MTLBlendFactor::One);
        color.setDestinationRGBBlendFactor(MTLBlendFactor::Source1Color);
        color.setRgbBlendOperation(MTLBlendOperation::Add);
        color.setSourceAlphaBlendFactor(MTLBlendFactor::Zero);
        color.setDestinationAlphaBlendFactor(MTLBlendFactor::One);
        color.setAlphaBlendOperation(MTLBlendOperation::Add);
    }
    compiler
        .newRenderPipelineStateWithDescriptor_compilerTaskOptions_error(&desc, None)
        .map_err(|e| ShaderError {
            file: file.to_string(),
            stage: "render pipeline",
            message: e.localizedDescription().to_string(),
        })
}

/// Второе цветовое вложение прохода `scene` (04.6, сессия 2): сдвиг на
/// экране от ветра (`Targets.wind`, RG16Float). Конвейеры прохода объявляют
/// его все; пишут только растения.
#[derive(Clone, Copy, PartialEq)]
pub enum Motion {
    /// Конвейер не в проходе `scene`.
    None,
    /// В проходе `scene`, не пишет.
    Masked,
    /// Пишет сдвиг от ветра (растения).
    Write,
}

fn set_motion_attachment(desc: &MTL4RenderPipelineDescriptor, motion: Motion) {
    if motion == Motion::None {
        return;
    }
    let a = unsafe { desc.colorAttachments().objectAtIndexedSubscript(1) };
    a.setPixelFormat(MTLPixelFormat::RG16Float);
    if motion == Motion::Masked {
        a.setWriteMask(MTLColorWriteMask::None);
    }
}

/// Только глубина в проходе с цветом: формат цвета как у сцены, запись цвета
/// выключена; фрагментный шейдер — отсечение по альфе.
fn color_masked_pipeline(
    compiler: &ProtocolObject<dyn MTL4Compiler>,
    lib: &ProtocolObject<dyn MTLLibrary>,
    file: &str,
    label: &str,
    (vs, fs): (&str, &str),
) -> Result<Obj<dyn MTLRenderPipelineState>, ShaderError> {
    require_function(lib, file, vs)?;
    require_function(lib, file, fs)?;
    let desc = MTL4RenderPipelineDescriptor::new();
    desc.setLabel(Some(&NSString::from_str(label)));
    desc.setVertexFunctionDescriptor(Some(&function(lib, vs)));
    desc.setFragmentFunctionDescriptor(Some(&function(lib, fs)));
    let color = unsafe { desc.colorAttachments().objectAtIndexedSubscript(0) };
    color.setPixelFormat(HDR_FORMAT);
    color.setWriteMask(MTLColorWriteMask::None);
    set_motion_attachment(&desc, Motion::Masked);
    compiler
        .newRenderPipelineStateWithDescriptor_compilerTaskOptions_error(&desc, None)
        .map_err(|e| ShaderError {
            file: file.to_string(),
            stage: "render pipeline",
            message: e.localizedDescription().to_string(),
        })
}

/// Собирает все pipeline кадра. При любой ошибке возвращает её целиком,
/// частичный набор не публикуется.
pub fn build_pipelines(
    device: &ProtocolObject<dyn MTLDevice>,
    compiler: &ProtocolObject<dyn MTL4Compiler>,
    dir: &Path,
    generation: u32,
    over: SourceOverride,
) -> Result<Pipelines, ShaderError> {
    let pattern_lib = compile_library(device, dir, "pattern.metal", over)?;
    let scene_lib = compile_library(device, dir, "scene.metal", over)?;
    let tonemap_lib = compile_library(device, dir, "tonemap.metal", over)?;
    let terrain_lib = compile_library(device, dir, "terrain.metal", over)?;
    let water_lib = compile_library(device, dir, "water.metal", over)?;
    let hands_lib = compile_library(device, dir, "hands.metal", over)?;
    let sky_lib = compile_library(device, dir, "sky.metal", over)?;
    let cull_lib = compile_library(device, dir, "cull.metal", over)?;
    let forest_lib = compile_library(device, dir, "forest.metal", over)?;
    let taa_lib = compile_library(device, dir, "taa.metal", over)?;
    let rain_lib = compile_library(device, dir, "rain.metal", over)?;
    let river_lib = compile_library(device, dir, "river.metal", over)?;
    let bloom_lib = compile_library(device, dir, "bloom.metal", over)?;
    let rt_lib = compile_library(device, dir, "rt.metal", over)?;
    let vol_lib = compile_library(device, dir, "volumetric.metal", over)?;
    let ui_lib = compile_library(device, dir, "ui.metal", over)?;
    Ok(Pipelines {
        pattern: compute_pipeline(compiler, &pattern_lib, "pattern.metal", "pattern_main")?,
        layout_probe: compute_pipeline(compiler, &pattern_lib, "pattern.metal", "layout_probe")?,
        scene: render_pipeline(
            compiler,
            &scene_lib,
            "scene.metal",
            "scene",
            ("scene_vs", "scene_fs"),
            HDR_FORMAT,
            Blend::Opaque,
            Motion::Masked,
        )?,
        tonemap: render_pipeline(
            compiler,
            &tonemap_lib,
            "tonemap.metal",
            "tonemap",
            ("tonemap_vs", "tonemap_fs"),
            DRAWABLE_FORMAT,
            Blend::Opaque,
            Motion::None,
        )?,
        terrain: render_pipeline(
            compiler,
            &terrain_lib,
            "terrain.metal",
            "terrain",
            ("terrain_vs", "terrain_fs"),
            HDR_FORMAT,
            Blend::Opaque,
            Motion::Masked,
        )?,
        terrain_clusters: render_pipeline(
            compiler,
            &terrain_lib,
            "terrain.metal",
            "terrain clusters",
            ("terrain_cluster_vs", "terrain_fs"),
            HDR_FORMAT,
            Blend::Opaque,
            Motion::Masked,
        )?,
        terrain_patch: render_pipeline(
            compiler,
            &terrain_lib,
            "terrain.metal",
            "terrain patch",
            ("terrain_patch_vs", "terrain_fs"),
            HDR_FORMAT,
            Blend::Opaque,
            Motion::Masked,
        )?,
        water: render_pipeline(
            compiler,
            &water_lib,
            "water.metal",
            "water",
            ("water_vs", "water_fs"),
            HDR_FORMAT,
            Blend::Transmit,
            Motion::None,
        )?,
        wet_sand: compute_pipeline(compiler, &water_lib, "water.metal", "wet_sand_update")?,
        water_probe: compute_pipeline(compiler, &water_lib, "water.metal", "water_probe")?,
        hands: render_pipeline(
            compiler,
            &hands_lib,
            "hands.metal",
            "hands",
            ("hands_vs", "hands_fs"),
            HDR_FORMAT,
            Blend::Opaque,
            Motion::None,
        )?,
        skin_probe: compute_pipeline(compiler, &hands_lib, "hands.metal", "skin_probe")?,
        sky_transmittance: compute_pipeline(
            compiler,
            &sky_lib,
            "sky.metal",
            "sky_transmittance_lut",
        )?,
        sky_multiscatter: compute_pipeline(
            compiler,
            &sky_lib,
            "sky.metal",
            "sky_multiscatter_lut",
        )?,
        sky_view: compute_pipeline(compiler, &sky_lib, "sky.metal", "sky_view_lut")?,
        sky_view_moon: compute_pipeline(compiler, &sky_lib, "sky.metal", "sky_view_lut_moon")?,
        sky_aerial: compute_pipeline(compiler, &sky_lib, "sky.metal", "sky_aerial_lut")?,
        cull_clusters: compute_pipeline(compiler, &cull_lib, "cull.metal", "cull_clusters")?,
        cull_finish: compute_pipeline(compiler, &cull_lib, "cull.metal", "cull_finish")?,
        taa_resolve: compute_pipeline(compiler, &taa_lib, "taa.metal", "taa_resolve")?,
        upscale_prep: compute_pipeline(compiler, &taa_lib, "taa.metal", "upscale_prep")?,
        rain: render_pipeline(
            compiler,
            &rain_lib,
            "rain.metal",
            "rain",
            ("rain_vs", "rain_fs"),
            HDR_FORMAT,
            Blend::Alpha,
            Motion::Masked,
        )?,
        river: render_pipeline(
            compiler,
            &river_lib,
            "river.metal",
            "river",
            ("river_vs", "river_fs"),
            HDR_FORMAT,
            Blend::Alpha,
            Motion::Masked,
        )?,
        mist: render_pipeline(
            compiler,
            &river_lib,
            "river.metal",
            "waterfall mist",
            ("mist_vs", "mist_fs"),
            HDR_FORMAT,
            Blend::Alpha,
            Motion::Masked,
        )?,
        river_rain: depth_only_pipeline(
            compiler,
            &river_lib,
            "river.metal",
            "river rain occluder",
            "river_rain_vs",
            None,
        )?,
        fish: render_pipeline(
            compiler,
            &river_lib,
            "river.metal",
            "fish",
            ("fish_vs", "fish_fs"),
            HDR_FORMAT,
            Blend::Opaque,
            Motion::Masked,
        )?,
        rain_splash: render_pipeline(
            compiler,
            &rain_lib,
            "rain.metal",
            "rain splash",
            ("rain_splash_vs", "rain_fs"),
            HDR_FORMAT,
            Blend::Alpha,
            Motion::Masked,
        )?,
        bloom_down: compute_pipeline(compiler, &bloom_lib, "bloom.metal", "bloom_down")?,
        bloom_up: compute_pipeline(compiler, &bloom_lib, "bloom.metal", "bloom_up")?,
        rt_terrain_grid: compute_pipeline(compiler, &rt_lib, "rt.metal", "rt_terrain_grid")?,
        rt_instances: compute_pipeline(compiler, &rt_lib, "rt.metal", "rt_instances")?,
        rt_count_finish: compute_pipeline(compiler, &rt_lib, "rt.metal", "rt_count_finish")?,
        rt_ao: compute_pipeline(compiler, &rt_lib, "rt.metal", "rt_ao")?,
        probe_update: compute_pipeline(compiler, &rt_lib, "rt.metal", "probe_update")?,
        volumetric_light: compute_pipeline(
            compiler,
            &vol_lib,
            "volumetric.metal",
            "volumetric_light",
        )?,
        sky_ambient: compute_pipeline(compiler, &sky_lib, "sky.metal", "sky_ambient")?,
        clouds_march: compute_pipeline(compiler, &sky_lib, "sky.metal", "clouds_march")?,
        exposure_meter: compute_pipeline(compiler, &sky_lib, "sky.metal", "exposure_meter")?,
        terrain_shadow: depth_only_pipeline(
            compiler,
            &terrain_lib,
            "terrain.metal",
            "terrain shadow",
            "terrain_shadow_vs",
            None,
        )?,
        plant_depth: color_masked_pipeline(
            compiler,
            &forest_lib,
            "forest.metal",
            "plants depth",
            ("plant_vs", "plant_depth_fs"),
        )?,
        plant_prepassed: render_pipeline(
            compiler,
            &forest_lib,
            "forest.metal",
            "plants (prepassed)",
            ("plant_vs", "plant_fs_prepassed"),
            HDR_FORMAT,
            Blend::Opaque,
            Motion::Write,
        )?,
        plant_shadow: depth_only_pipeline(
            compiler,
            &forest_lib,
            "forest.metal",
            "plant shadow",
            "plant_shadow_vs",
            Some("plant_shadow_fs"),
        )?,
        ui: render_pipeline(
            compiler,
            &ui_lib,
            "ui.metal",
            "ui overlay",
            ("ui_vs", "ui_fs"),
            DRAWABLE_FORMAT,
            Blend::Alpha,
            Motion::None,
        )?,
        forest_place: compute_pipeline(compiler, &forest_lib, "forest.metal", "forest_place")?,
        forest_finish: compute_pipeline(compiler, &forest_lib, "forest.metal", "forest_finish")?,
        ground_water: compute_pipeline(compiler, &forest_lib, "forest.metal", "ground_water")?,
        stars: render_pipeline(
            compiler,
            &sky_lib,
            "sky.metal",
            "stars",
            ("stars_vs", "stars_fs"),
            HDR_FORMAT,
            Blend::Additive,
            Motion::Masked,
        )?,
        sky: render_pipeline(
            compiler,
            &sky_lib,
            "sky.metal",
            "sky",
            ("sky_vs", "sky_fs"),
            HDR_FORMAT,
            Blend::Opaque,
            Motion::Masked,
        )?,
        generation,
    })
}
