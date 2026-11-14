//! Сверки GPU с CPU-эталонами для самопроверки и отчёты по F9.

use glam::{Mat4, Vec3};

use super::SelfTest;
use crate::renderer::{self, Readback};
use crate::world::{Ground, World};
use crate::{layout, reference, scene, sim, water};

pub(in crate::app) fn verify_readback(t: &mut SelfTest, rb: &Readback, view: Mat4, proj: Mat4) {
    let (mismatches, max_diff) =
        reference::compare_pattern(&rb.pattern, reference::PATTERN_SIZE, rb.pattern_time, 2);
    t.check(
        mismatches == 0,
        format!(
            "compute-узор {}×{} совпадает с эталоном CPU: несовпадений {mismatches}, макс. отклонение {max_diff}/255 (кадр {})",
            reference::PATTERN_SIZE,
            reference::PATTERN_SIZE,
            rb.frame
        ),
    );
    let boxes = scene::boxes();
    let mut worst = 0.0f32;
    let mut hits = 0;
    let mut covered = [false; 2];
    let mut failures = 0;
    for &(x, y, got) in &rb.depth {
        let expected = reference::expected_depth(x, y, rb.width, rb.height, view, proj, &boxes);
        if expected > 0.0 {
            hits += 1;
            // Какой куб ближе: сравнить с глубиной только первого куба.
            let only_a =
                reference::expected_depth(x, y, rb.width, rb.height, view, proj, &boxes[..1]);
            covered[usize::from(only_a != expected)] = true;
        }
        // Допуск: растеризация берёт центр пикселя, эталон — тот же центр;
        // на гранях куба разница — интерполяция float.
        let err = (got - expected).abs();
        let tol = 1e-4 + expected.abs() * 1e-3;
        if err > tol {
            failures += 1;
            println!("       глубина ({x},{y}): GPU {got:.6}, эталон {expected:.6}");
        }
        worst = worst.max(err);
    }
    t.check(
        failures == 0 && hits >= 4 && covered == [true, true],
        format!(
            "reversed-Z глубина в {} точках {}×{} (попаданий в кубы {hits}, видны оба куба {}): макс. ошибка {worst:.2e}",
            rb.depth.len(),
            rb.width,
            rb.height,
            covered == [true, true]
        ),
    );
    let expected = layout::expected_probe();
    let mismatched: Vec<_> = (0..layout::LAYOUT_PROBE_LEN)
        .filter(|&i| rb.probe[i] != expected[i])
        .map(|i| {
            format!(
                "{} GPU {} ≠ CPU {}",
                layout::LAYOUT_PROBE_NAMES[i],
                rb.probe[i],
                expected[i]
            )
        })
        .collect();
    t.check(
        mismatched.is_empty(),
        format!(
            "раскладка структур CPU/GPU ({} полей) {}",
            expected.len(),
            mismatched.join(", ")
        ),
    );
}

/// Глубина ландшафта с GPU против трассировки луча по `SampleGround` на CPU.
/// Возвращает (точек на ландшафте, точек неба, расхождений, наибольшая
/// относительная ошибка глубины).
pub(in crate::app) fn compare_terrain_depth(
    rb: &Readback,
    view: Mat4,
    proj: Mat4,
    world: &World,
    log: bool,
) -> (u32, u32, u32, f32) {
    // Для reversed-Z с бесконечной дальней плоскостью глубина = near / z_view.
    let near = sim::NEAR_PLANE_M;
    let (mut hits, mut sky, mut bad, mut worst) = (0, 0, 0, 0.0f32);
    for &(x, y, got) in &rb.depth {
        let expected = reference::expected_terrain_view_depth(
            (x, y),
            (rb.width, rb.height),
            view,
            proj,
            world,
            3000.0,
        );
        let ok = match expected {
            Some(z) => {
                hits += 1;
                let gpu_z = if got > 0.0 { near / got } else { f32::INFINITY };
                let rel = (gpu_z - z).abs() / z;
                worst = worst.max(rel);
                if log {
                    println!("       ландшафт ({x},{y}): GPU {gpu_z:.3} м, CPU {z:.3} м");
                }
                rel < 2e-3
            }
            None => {
                sky += 1;
                got == 0.0
            }
        };
        if !ok {
            bad += 1;
            println!("       расхождение в ({x},{y}): GPU глубина {got:.7}, CPU {expected:?}");
        }
    }
    (hits, sky, bad, worst)
}

/// Вид Raw: HDR = (высота, уклон в градусах, доминирующий биом, тип берега).
/// Сверка с CPU в точках попадания луча: высота ±5 см (half), уклон в диапазоне
/// уклонов граней под квадратом 2 × 2 пикселя ±1°,
/// доминирующий биом ближайшего узла — допускается одно расхождение на границе.
pub(in crate::app) fn compare_terrain_raw(
    rb: &Readback,
    view: Mat4,
    proj: Mat4,
    world: &World,
) -> (u32, u32, f32, f32, u32) {
    let (mut checked, mut bad, mut dh, mut ds, mut biome_miss) = (0, 0, 0.0f32, 0.0f32, 0);
    // Вид Raw пишет высоту относительно камеры.
    let camera_y = view.inverse().w_axis.y;
    for (&(x, y, _), got) in rb.depth.iter().zip(&rb.hdr) {
        let Some(hit) =
            reference::terrain_hit((x, y), (rb.width, rb.height), view, proj, world, 3000.0)
        else {
            continue;
        };
        let g = world
            .sample_ground(hit.x, hit.z)
            .expect("попадание на карте");
        let slope = g.normal.y.clamp(-1.0, 1.0).acos().to_degrees();
        let e_h = (got[0] + camera_y - g.height).abs();
        // GPU берёт нормаль грани по производным в квадрате 2 × 2 пикселя: у ребра
        // треугольника производные приходят с двух граней, и уклон получается
        // промежуточным. Допустимо значение в диапазоне уклонов граней в пределах
        // ±2 пикселей вокруг точки попадания.
        let dist = (hit - view.inverse().w_axis.truncate()).length();
        let px = 2.0 * dist / (proj.y_axis.y * rb.height as f32);
        let (lo, hi) = [(0.0, 0.0), (px, 0.0), (-px, 0.0), (0.0, px), (0.0, -px)]
            .iter()
            .filter_map(|&(dx, dz)| world.sample_ground(hit.x + dx, hit.z + dz))
            .map(|s| s.normal.y.clamp(-1.0, 1.0).acos().to_degrees())
            .fold((f32::MAX, f32::MIN), |(a, b), v| (a.min(v), b.max(v)));
        let e_s = (lo - got[1]).max(got[1] - hi).max(0.0);
        dh = dh.max(e_h);
        ds = ds.max(e_s);
        let tile = world.tile(g.tile).expect("тайл");
        let local =
            ((glam::Vec2::new(hit.x, hit.z) - world.tile_origin(g.tile)) / world.step_m).round();
        let s = tile.at(local.x as usize, local.y as usize);
        let w = [s.weights.forest, s.weights.peat, s.weights.shore];
        let dominant = (0..3).max_by(|&a, &b| w[a].total_cmp(&w[b])).unwrap() as f32;
        biome_miss += (got[2] != dominant) as u32;
        checked += 1;
        if e_h > 0.05 || e_s > 1.0 {
            bad += 1;
            println!(
                "       вид Raw ({x},{y}): GPU h {:.3} уклон {:.2}°, CPU h {:.3} уклон {slope:.2}°",
                got[0], got[1], g.height
            );
        }
    }
    (checked, bad, dh, ds, biome_miss)
}

pub(in crate::app) fn verify_terrain_readback(
    t: &mut SelfTest,
    rb: &Readback,
    view: Mat4,
    proj: Mat4,
    world: &World,
) {
    let (checked, bad_raw, dh, ds, biome_miss) = compare_terrain_raw(rb, view, proj, world);
    t.check(
        checked >= 4 && bad_raw == 0 && biome_miss <= 1,
        format!(
            "отладочный вид ландшафта: высота и уклон GPU совпадают с SampleGround в {checked} точках (макс. {dh:.3} м, {ds:.2}°), биом не совпал в {biome_miss}"
        ),
    );
    let (hits, sky, bad, worst) = compare_terrain_depth(rb, view, proj, world, false);
    t.check(
        bad == 0 && hits >= 4,
        format!(
            "ландшафт: глубина GPU совпадает с лучом по SampleGround в {hits} точках (небо {sky}), макс. отн. ошибка {worst:.1e}"
        ),
    );
    let expected = layout::expected_probe();
    t.check(
        rb.probe == expected,
        format!(
            "раскладка TerrainVertex и общих структур ({} полей)",
            expected.len()
        ),
    );
}

/// Функция волн на GPU (ядро `water_probe`) совпадает с `water::wave_at` на CPU:
/// смещённая точка ±2 мм, пена ±0,02.
pub(in crate::app) fn verify_water_probe(
    t: &mut SelfTest,
    rb: &Readback,
    frame: Option<renderer::WaterFrame>,
    shore: Option<&water::ShoreField>,
) {
    let (Some(frame), Some(shore)) = (frame, shore) else {
        t.check(false, "вода не загружена".to_string());
        return;
    };
    let (mut worst_pos, mut worst_foam) = (0.0f32, 0.0f32);
    for (p, got) in frame.params.probe.iter().zip(&rb.water_probe) {
        let w = water::wave_at(shore, &frame.params, glam::Vec2::new(p[0], p[1]), p[2], 0.0);
        let d = (w.pos - Vec3::new(got[0], got[1], got[2]))
            .abs()
            .max_element();
        worst_pos = worst_pos.max(d);
        worst_foam = worst_foam.max((w.foam - got[3]).abs());
    }
    t.check(
        worst_pos < 2e-3 && worst_foam < 0.02,
        format!(
            "волны моря: GPU совпадает с CPU в {} точках (макс. {:.2} мм, пена {worst_foam:.3})",
            frame.params.probe.len(),
            worst_pos * 1000.0
        ),
    );
}

pub(in crate::app) fn report_terrain_readback(
    rb: &Readback,
    view: Mat4,
    proj: Mat4,
    world: &World,
) {
    let (hits, sky, bad, worst) = compare_terrain_depth(rb, view, proj, world, true);
    eprintln!(
        "[readback] ландшафт: точек {hits}, небо {sky}, расхождений {bad}, макс. отн. ошибка {worst:.1e}"
    );
}

pub(in crate::app) fn report_readback(rb: &Readback, view: Mat4, proj: Mat4) {
    let (mismatches, max_diff) =
        reference::compare_pattern(&rb.pattern, reference::PATTERN_SIZE, rb.pattern_time, 2);
    eprintln!(
        "[readback] кадр {}: узор — несовпадений {mismatches}, макс. {max_diff}/255",
        rb.frame
    );
    let boxes = scene::boxes();
    for &(x, y, got) in &rb.depth {
        let expected = reference::expected_depth(x, y, rb.width, rb.height, view, proj, &boxes);
        eprintln!("[readback] глубина ({x},{y}) GPU {got:.6} эталон {expected:.6}");
    }
}

/// Скиннинг на GPU (ядро `skin_probe`) совпадает с `hands::skin_position` в
/// контрольной позе: ±0,01 мм.
pub(in crate::app) fn verify_skin_probe(
    t: &mut SelfTest,
    rb: &Readback,
    hands: Option<&crate::hands::Hands>,
) {
    let Some(h) = hands else {
        t.check(false, "руки не загружены".to_string());
        return;
    };
    let palette = h.control_palette();
    let n = h.asset.vertices.len();
    let mut worst = 0.0f32;
    let mut bad_index = 0;
    for (i, got) in rb.skin_probe.iter().enumerate() {
        let idx = i * (n / crate::layout::SKIN_PROBES);
        bad_index += (got[3] as usize != idx) as u32;
        let want = crate::hands::skin_position(&h.asset.vertices[idx], &palette);
        worst = worst.max(
            (want - Vec3::new(got[0], got[1], got[2]))
                .abs()
                .max_element(),
        );
    }
    t.check(
        worst < 1e-5 && bad_index == 0,
        format!(
            "руки: скиннинг GPU совпадает с CPU в {} вершинах (макс. {:.4} мм)",
            rb.skin_probe.len(),
            worst * 1000.0
        ),
    );
}
