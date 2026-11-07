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
    let common = std::fs::read_to_string(dir.join("common.h")).map_err(|e| ShaderError {
        file: "common.h".into(),
        stage: "read",
        message: e.to_string(),
    })?;
    // Metal API не знает путей проекта, поэтому общий заголовок подставляем сами.
    Ok(text.replace("#include \"common.h\"", &common))
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
    vs: &str,
    fs: &str,
    color_format: MTLPixelFormat,
) -> Result<Obj<dyn MTLRenderPipelineState>, ShaderError> {
    require_function(lib, file, vs)?;
    require_function(lib, file, fs)?;
    let desc = MTL4RenderPipelineDescriptor::new();
    desc.setLabel(Some(&NSString::from_str(label)));
    desc.setVertexFunctionDescriptor(Some(&function(lib, vs)));
    desc.setFragmentFunctionDescriptor(Some(&function(lib, fs)));
    let color = unsafe { desc.colorAttachments().objectAtIndexedSubscript(0) };
    color.setPixelFormat(color_format);
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
    Ok(Pipelines {
        pattern: compute_pipeline(compiler, &pattern_lib, "pattern.metal", "pattern_main")?,
        layout_probe: compute_pipeline(compiler, &pattern_lib, "pattern.metal", "layout_probe")?,
        scene: render_pipeline(
            compiler,
            &scene_lib,
            "scene.metal",
            "scene",
            "scene_vs",
            "scene_fs",
            HDR_FORMAT,
        )?,
        tonemap: render_pipeline(
            compiler,
            &tonemap_lib,
            "tonemap.metal",
            "tonemap",
            "tonemap_vs",
            "tonemap_fs",
            DRAWABLE_FORMAT,
        )?,
        terrain: render_pipeline(
            compiler,
            &terrain_lib,
            "terrain.metal",
            "terrain",
            "terrain_vs",
            "terrain_fs",
            HDR_FORMAT,
        )?,
        generation,
    })
}
