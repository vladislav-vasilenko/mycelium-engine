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

pub struct Pipelines {
    pub pattern: Obj<dyn MTLComputePipelineState>,
    pub layout_probe: Obj<dyn MTLComputePipelineState>,
    pub scene: Obj<dyn MTLRenderPipelineState>,
    pub tonemap: Obj<dyn MTLRenderPipelineState>,
    pub terrain: Obj<dyn MTLRenderPipelineState>,
    /// Кластерный рельеф (02.3): тот же фрагментный шейдер, вершины из поля высот.
    pub terrain_clusters: Obj<dyn MTLRenderPipelineState>,
    /// Вода: dual-source смешивание `dst × пропускание + цвет`.
    pub water: Obj<dyn MTLRenderPipelineState>,
    pub wet_sand: Obj<dyn MTLComputePipelineState>,
    pub water_probe: Obj<dyn MTLComputePipelineState>,
    /// Руки со скином (проход `viewmodel`) и сверка скиннинга с CPU.
    pub hands: Obj<dyn MTLRenderPipelineState>,
    pub skin_probe: Obj<dyn MTLComputePipelineState>,
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

fn render_pipeline(
    compiler: &ProtocolObject<dyn MTL4Compiler>,
    lib: &ProtocolObject<dyn MTLLibrary>,
    file: &str,
    label: &str,
    (vs, fs): (&str, &str),
    color_format: MTLPixelFormat,
    transmit_blend: bool,
) -> Result<Obj<dyn MTLRenderPipelineState>, ShaderError> {
    require_function(lib, file, vs)?;
    require_function(lib, file, fs)?;
    let desc = MTL4RenderPipelineDescriptor::new();
    desc.setLabel(Some(&NSString::from_str(label)));
    desc.setVertexFunctionDescriptor(Some(&function(lib, vs)));
    desc.setFragmentFunctionDescriptor(Some(&function(lib, fs)));
    let color = unsafe { desc.colorAttachments().objectAtIndexedSubscript(0) };
    color.setPixelFormat(color_format);
    if transmit_blend {
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
            false,
        )?,
        tonemap: render_pipeline(
            compiler,
            &tonemap_lib,
            "tonemap.metal",
            "tonemap",
            ("tonemap_vs", "tonemap_fs"),
            DRAWABLE_FORMAT,
            false,
        )?,
        terrain: render_pipeline(
            compiler,
            &terrain_lib,
            "terrain.metal",
            "terrain",
            ("terrain_vs", "terrain_fs"),
            HDR_FORMAT,
            false,
        )?,
        terrain_clusters: render_pipeline(
            compiler,
            &terrain_lib,
            "terrain.metal",
            "terrain clusters",
            ("terrain_cluster_vs", "terrain_fs"),
            HDR_FORMAT,
            false,
        )?,
        water: render_pipeline(
            compiler,
            &water_lib,
            "water.metal",
            "water",
            ("water_vs", "water_fs"),
            HDR_FORMAT,
            true,
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
            false,
        )?,
        skin_probe: compute_pipeline(compiler, &hands_lib, "hands.metal", "skin_probe")?,
        generation,
    })
}
