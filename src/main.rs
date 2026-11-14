//! «Эхо мицелия»: окно, камера от первого лица, ландшафт маршрута (этап 02),
//! compute → raster, HDR и тонмаппинг, перезагрузка MSL по F5.
//!
//! Управление: клик — захват мыши, WASD — движение, Shift — бег/быстрее,
//! Space — прыжок (в полёте вверх), G — ходьба/полёт (над водой — на начало тропы),
//! R — на начало тропы, E/Q — вверх/вниз в полёте,
//! B — покачивание головы, V/Shift+V — отладочные виды ландшафта,
//! T — ландшафт/тестовые кубы,
//! Esc — отпустить мышь, ⌘M — свернуть, F5 — перезагрузить MSL,
//! F9 — readback и сверка с эталоном, F12 — GPU capture одного кадра.
//!
//! Режимы: `--selftest` — автоматическая проверка с кодом выхода,
//! `--overview` — старт в полёте над маршрутом,
//! `--world DIR` — собрать мир из слоёв PNG в каталоге (`world.json`),
//! `--export-world DIR` — записать слои сгенерированного мира и выйти,
//! `--camera x,y,z,tx,ty,tz` — старт в полёте с взглядом на точку,
//! `--view lit|height|slope|biome|substrate|coast|wetness|materials` — вид ландшафта,
//! `--capture-frame N` — записать кадр N в `captures/`.

mod app;
mod frame_graph;
mod hands;
mod layout;
mod platform;
mod reference;
mod renderer;
mod scene;
mod sim;
mod sky;
mod terrain_clusters;
mod terrain_materials;
mod terrain_mesh;
mod water;
mod world;

use winit::event_loop::{ControlFlow, EventLoop};

use app::{App, Args, load_world};

fn main() {
    let args = Args::parse();
    if let Some(dir) = &args.export_world {
        let world = load_world(&args);
        match world::layers::export(&world, dir) {
            Ok(m) => {
                eprintln!(
                    "слои записаны в {}: {}×{} точек, контрольная сумма мира {}",
                    dir.display(),
                    m.size_samples[0],
                    m.size_samples[1],
                    m.exported_checksum
                );
                std::process::exit(0);
            }
            Err(e) => {
                eprintln!("экспорт слоёв: {e}");
                std::process::exit(2);
            }
        }
    }
    if args.capture_frame.is_some() && std::env::var_os("MTL_CAPTURE_ENABLED").is_none() {
        eprintln!("[capture] для записи .gputrace запусти с MTL_CAPTURE_ENABLED=1");
    }
    let event_loop = EventLoop::new().expect("event loop");
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App::new(args);
    if let Err(e) = event_loop.run_app(&mut app) {
        eprintln!("event loop: {e}");
        std::process::exit(2);
    }
    std::process::exit(app.exit_code);
}
