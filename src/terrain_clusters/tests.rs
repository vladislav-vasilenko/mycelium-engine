use std::collections::HashMap;

use glam::{Mat4, Vec3};

use super::*;
use crate::world::heightfield::HeightField;

/// Холмистое поле 129 × 129 с шагом 1 м (32 768 треугольников).
fn hills() -> HeightField {
    let n = 129;
    let mut h = Vec::with_capacity(n * n);
    for j in 0..n {
        for i in 0..n {
            let (x, z) = (i as f32, j as f32);
            h.push(
                6.0 * (x * 0.05).sin() * (z * 0.07).cos()
                    + 2.0 * (x * 0.21 + z * 0.13).sin()
                    + 0.4 * (x * 0.9).sin() * (z * 1.1).sin(),
            );
        }
    }
    HeightField::new(n, n, 1.0, h)
}

fn view_from(hf: &HeightField, eye: Vec3, target: Vec3) -> View {
    let view = Mat4::look_at_rh(eye, target, Vec3::Y);
    let proj = Mat4::perspective_infinite_reverse_rh(70f32.to_radians(), 16.0 / 9.0, 0.05);
    let _ = hf;
    View::new(view, proj, 1080.0, 0.05, 1.0)
}

fn sorted(t: [u32; 3]) -> [u32; 3] {
    let mut s = t;
    s.sort_unstable();
    s
}

/// Уровень 0 покрывает каждый треугольник поля ровно один раз.
#[test]
fn level0_covers_heightfield_once() {
    let hf = hills();
    let set = build(&hf);
    let mut count: HashMap<[u32; 3], u32> = HashMap::new();
    for (i, c) in set.info.iter().enumerate() {
        if c.refined < 0 {
            for t in set.cluster_triangles(i) {
                *count.entry(sorted(t)).or_default() += 1;
            }
        }
    }
    let grid = hf.triangles();
    assert_eq!(count.len(), grid.len() / 3);
    for t in grid.as_chunks::<3>().0 {
        assert_eq!(count.get(&sorted([t[0], t[1], t[2]])), Some(&1));
    }
    assert!(set.levels >= 3, "уровней {}", set.levels);
    assert!(
        set.gpu
            .iter()
            .all(|c| c.triangle_count as usize <= MAX_TRIANGLES)
    );
}

/// Ошибка родителя не меньше ошибки ребёнка; повторная сборка совпадает.
#[test]
fn errors_are_monotonic_and_build_is_deterministic() {
    let hf = hills();
    let set = build(&hf);
    for c in &set.info {
        if c.refined >= 0 {
            let child = set.groups[c.refined as usize].error;
            let parent = set.groups[c.group as usize].error;
            assert!(parent >= child, "родитель {parent} < ребёнок {child}");
        }
    }
    let again = build(&hf);
    assert_eq!(set.vertices, again.vertices);
    assert_eq!(set.triangles, again.triangles);
}

/// Срез водонепроницаем: каждое ребро — у двух треугольников, кроме края карты.
#[test]
fn cut_is_watertight_from_many_cameras() {
    let hf = hills();
    let set = build(&hf);
    let n = hf.nx as u32;
    let on_border = |v: u32| {
        let (i, j) = (v % n, v / n);
        i == 0 || j == 0 || i == n - 1 || j == n - 1
    };
    let cams = [
        (Vec3::new(64.0, 3.0, 64.0), Vec3::new(120.0, 0.0, 100.0)),
        (Vec3::new(-50.0, 40.0, -50.0), Vec3::new(64.0, 0.0, 64.0)),
        (Vec3::new(10.0, 2.0, 120.0), Vec3::new(100.0, 0.0, 10.0)),
        (Vec3::new(64.0, 300.0, 64.0), Vec3::new(64.0, 0.0, 65.0)),
        (Vec3::new(200.0, 15.0, 64.0), Vec3::new(0.0, 0.0, 64.0)),
        (Vec3::new(30.0, 8.0, 30.0), Vec3::new(31.0, 8.0, 30.0)),
    ];
    let mut levels_seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for (eye, target) in cams {
        let v = view_from(&hf, eye, target);
        select(&set, &v, false, &mut out);
        let mut edges: HashMap<(u32, u32), u32> = HashMap::new();
        for &c in &out {
            levels_seen.insert(set.gpu[c as usize].depth);
            for t in set.cluster_triangles(c as usize) {
                for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                    *edges.entry((a.min(b), a.max(b))).or_default() += 1;
                }
            }
        }
        for (&(a, b), &k) in &edges {
            let border = on_border(a) && on_border(b);
            assert!(
                k == 2 || (k == 1 && border),
                "камера {eye}: ребро ({a}, {b}) у {k} треугольников"
            );
        }
    }
    assert!(
        levels_seen.len() >= 2,
        "срезы не смешивают уровни: {levels_seen:?}"
    );
}

/// У глаз игрока рисуется исходная сетка, вдали — грубее.
#[test]
fn near_camera_uses_level0() {
    let hf = hills();
    let set = build(&hf);
    let eye = Vec3::new(64.0, hf.sample(64.0, 64.0).unwrap().0 + 1.7, 64.0);
    let v = view_from(&hf, eye, eye + Vec3::new(1.0, -0.2, 0.3));
    let mut out = Vec::new();
    select(&set, &v, false, &mut out);
    for &c in &out {
        let b = set.info[c as usize].bounds;
        if (b.center - eye).length() - b.radius < 8.0 {
            assert_eq!(set.gpu[c as usize].depth, 0, "кластер у глаз не уровня 0");
        }
    }
    let far = view_from(
        &hf,
        Vec3::new(64.0, 2000.0, 64.0),
        Vec3::new(64.0, 0.0, 64.1),
    );
    let mut out_far = Vec::new();
    let s_far = select(&set, &far, false, &mut out_far);
    assert!(
        (s_far.triangles as usize) < hf.triangles().len() / 3 / 8,
        "с 2 км видно {} треугольников",
        s_far.triangles
    );
}

/// Пирамида видимости отбрасывает кластеры за спиной камеры.
#[test]
fn frustum_culls_behind_camera() {
    let hf = hills();
    let set = build(&hf);
    let v = view_from(
        &hf,
        Vec3::new(64.0, 10.0, 64.0),
        Vec3::new(128.0, 5.0, 64.0),
    );
    let mut all = Vec::new();
    let mut seen = Vec::new();
    select(&set, &v, false, &mut all);
    let s = select(&set, &v, true, &mut seen);
    assert!(s.culled > 0 && seen.len() < all.len());
    for &c in &seen {
        let b = set.info[c as usize].bounds;
        assert!(
            b.center.x + b.radius > 64.0 - 1.0,
            "кластер за спиной: {b:?}"
        );
    }
}

/// Обход деревьев над группами даёт тот же срез, что перебор всех кластеров.
#[test]
fn hierarchy_traversal_matches_brute_force() {
    let hf = hills();
    let set = build(&hf);
    assert!(set.nodes.len() > set.levels as usize);
    for (eye, target, threshold) in [
        (
            Vec3::new(64.0, 3.0, 64.0),
            Vec3::new(120.0, 0.0, 100.0),
            1.0,
        ),
        (
            Vec3::new(-50.0, 40.0, -50.0),
            Vec3::new(64.0, 0.0, 64.0),
            1.0,
        ),
        (
            Vec3::new(64.0, 300.0, 64.0),
            Vec3::new(64.0, 0.0, 65.0),
            0.5,
        ),
        (
            Vec3::new(10.0, 2.0, 120.0),
            Vec3::new(100.0, 0.0, 10.0),
            4.0,
        ),
        (Vec3::new(30.0, 8.0, 30.0), Vec3::new(31.0, 8.0, 30.0), -1.0),
    ] {
        let mut v = view_from(&hf, eye, target);
        v.threshold_px = threshold;
        let (mut a, mut b) = (Vec::new(), Vec::new());
        let sa = select(&set, &v, true, &mut a);
        let sb = select::select_all(&set, &v, true, &mut b);
        a.sort_unstable();
        b.sort_unstable();
        assert_eq!(a, b, "камера {eye}, порог {threshold}");
        assert_eq!(sa, sb);
    }
}
