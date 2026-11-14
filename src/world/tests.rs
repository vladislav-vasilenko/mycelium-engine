//! Тесты мира: стыки тайлов, детерминизм, маршрут, остров, тип берега.

use super::*;
use std::sync::OnceLock;

pub fn test_world() -> &'static World {
    static W: OnceLock<World> = OnceLock::new();
    W.get_or_init(|| World::generate(0xEC40, FIRST_MAP_TILES, FIRST_MAP_TILES))
}

#[test]
fn neighbour_tiles_share_border_heights_and_normals() {
    let w = test_world();
    for tz in 0..w.tiles_z {
        for tx in 0..w.tiles_x - 1 {
            let a = w.tile(TileId { x: tx, z: tz }).unwrap();
            let b = w.tile(TileId { x: tx + 1, z: tz }).unwrap();
            for j in 0..TILE_SAMPLES {
                assert_eq!(a.at(TILE_SAMPLES - 1, j), b.at(0, j));
            }
        }
    }
    for tz in 0..w.tiles_z - 1 {
        for tx in 0..w.tiles_x {
            let a = w.tile(TileId { x: tx, z: tz }).unwrap();
            let b = w.tile(TileId { x: tx, z: tz + 1 }).unwrap();
            for i in 0..TILE_SAMPLES {
                assert_eq!(a.at(i, TILE_SAMPLES - 1), b.at(i, 0));
            }
        }
    }
}

#[test]
fn same_world_coordinates_give_same_ground_from_both_tiles() {
    let w = test_world();
    // Точки ровно на границе тайлов: ответ не зависит от того, какой тайл выбран.
    for k in 0..40 {
        let x = 64.0 * (1 + k % 10) as f32;
        let z = 13.37 + 17.0 * k as f32;
        let s = w.sample_ground(x, z).unwrap();
        let left_tile = w
            .tile(TileId {
                x: x as i32 / 64 - 1,
                z: (z / 64.0) as i32,
            })
            .unwrap();
        let j = ((z - w.tile_origin(left_tile.id).y) / SAMPLE_STEP_M) as usize;
        let fz = (z - w.tile_origin(left_tile.id).y) / SAMPLE_STEP_M - j as f32;
        let h0 = left_tile.at(64, j).height;
        let h1 = left_tile.at(64, j + 1).height;
        assert!(
            (s.height - (h0 + (h1 - h0) * fz)).abs() < 1e-4,
            "x={x} z={z}"
        );
    }
}

#[test]
fn sample_ground_matches_lod0_vertices() {
    let w = test_world();
    let t = w.tile(TileId { x: 3, z: 5 }).unwrap();
    for (i, j) in [(0, 0), (10, 20), (63, 1), (64, 64), (31, 47)] {
        let p = w.tile_origin(t.id) + Vec2::new(i as f32, j as f32);
        let s = w.sample_ground(p.x, p.y).unwrap();
        assert!((s.height - t.at(i, j).height).abs() < 1e-4);
    }
}

#[test]
fn generation_is_deterministic() {
    let a = World::generate(0xEC40, 3, 3);
    let b = World::generate(0xEC40, 3, 3);
    assert_eq!(a.checksum(), b.checksum());
    let c = World::generate(0xEC41, 3, 3);
    assert_ne!(a.checksum(), c.checksum());
    assert!(
        a.tiles
            .iter()
            .all(|t| t.world_seed == 0xEC40 && t.generator_version == GENERATOR_VERSION)
    );
}

#[test]
fn biome_weights_are_normalized() {
    let w = test_world();
    for t in &w.tiles {
        for s in t.samples.iter().step_by(97) {
            let sum = s.weights.forest + s.weights.peat + s.weights.shore;
            assert!((sum - 1.0).abs() < 1e-4, "{sum}");
        }
    }
}

#[test]
fn route_has_trail_river_and_sea() {
    let w = test_world();
    let g = &w.generator;
    let start = w.marker(MarkerKind::TrailStart).unwrap().position;
    let on_trail = w.sample_ground(start.x, start.z).unwrap();
    assert!(
        on_trail.on_trail && !on_trail.wet,
        "начало тропы сухое и на тропе"
    );
    let src = g.river.point_at(0.3);
    assert!(
        w.sample_ground(src.x, src.y).unwrap().wet,
        "середина русла мокрая"
    );
    let sea = w.sample_ground(512.0, 1000.0).unwrap();
    assert!(sea.height < 0.0 && sea.wet, "юг карты — море");
    let peat = w.sample_ground(g.basin_center.x, g.basin_center.y).unwrap();
    assert_eq!(peat.substrate, Substrate::Peat);
    assert_eq!(w.markers.len(), 6);
}

/// Остров: весь край карты — открытое море, тропа и река до устья на суше
/// не ближе 15 м к кромке, суша занимает разумную долю карты.
#[test]
fn island_is_surrounded_by_sea() {
    let w = test_world();
    let g = &w.generator;
    let size = w.size_m();
    for k in 0..=64 {
        let t = k as f32 / 64.0;
        for (x, z) in [
            (t * size.x, 0.0),
            (t * size.x, size.y),
            (0.0, t * size.y),
            (size.x, t * size.y),
        ] {
            let s = w.sample_ground(x, z).unwrap();
            assert!(
                s.height < -10.0,
                "край карты ({x}, {z}) на глубине {}",
                s.height
            );
        }
    }
    for k in 0..=200 {
        let p = g.trail.point_at(k as f32 / 200.0);
        assert!(g.coast_distance(p) > 15.0, "тропа у кромки в {p}");
    }
    for k in 0..=150 {
        let p = g.river.point_at(k as f32 / 200.0);
        assert!(
            g.coast_distance(p) > 15.0,
            "русло выходит к морю раньше устья в {p}"
        );
    }
    let mut land = 0;
    let n = 128;
    for j in 0..n {
        for i in 0..n {
            let (x, z) = ((i as f32 + 0.5) * 8.0, (j as f32 + 0.5) * 8.0);
            land += (w.sample_ground(x, z).unwrap().height > 0.0) as usize;
        }
    }
    let share = land as f32 / (n * n) as f32;
    assert!((0.45..0.75).contains(&share), "доля суши {share}");
}

/// Вдоль береговой линии чередуются песчаные и каменистые участки: не меньше
/// трёх каждого вида длиннее 40 м; мыс в конце тропы каменистый, устье песчаное.
#[test]
fn coast_alternates_sand_and_rock() {
    let g = &test_world().generator;
    let n = 1440;
    let mut coast = Vec::with_capacity(n);
    for k in 0..n {
        let a = k as f32 / n as f32 * std::f32::consts::TAU;
        let dir = Vec2::new(a.cos(), a.sin()) * g.island_radii;
        // Первая точка моря на луче от центра.
        let mut t = 0.6;
        while g.coast_distance(g.island_center + dir * t) > 0.0 {
            t += 0.002;
        }
        coast.push(g.island_center + dir * t);
    }
    let rocky: Vec<bool> = coast.iter().map(|p| g.rocky(*p) > 0.5).collect();
    let start = rocky
        .iter()
        .position(|&r| r != rocky[0])
        .expect("берег одного типа");
    let (mut sand, mut rock) = (0, 0);
    let mut run = 0.0;
    for i in 0..n {
        let (a, b) = ((start + i) % n, (start + i + 1) % n);
        run += coast[a].distance(coast[b]);
        if rocky[a] != rocky[b] {
            if run > 40.0 {
                if rocky[a] { rock += 1 } else { sand += 1 }
            }
            run = 0.0;
        }
    }
    assert!(
        sand >= 3 && rock >= 3,
        "песчаных участков {sand}, каменистых {rock}"
    );
    assert!(g.rocky(g.cape_center) > 0.9, "мыс в конце тропы каменистый");
    assert!(g.rocky(g.mouth) < 0.1, "пляж у устья песчаный");
}

/// Мир Якусимы из слоёв: объекты OSM загружены, начало тропы — на дороге к
/// ущелью Сиратани на 600–700 м, устье Анбо у реки, Дзёмон-суги в лесу на ~1300 м.
#[test]
fn yakushima_layers_have_trails_and_rivers() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/world/yakushima/far");
    let w = super::layers::import(&dir).expect("мир Якусимы");
    let f = w.features.as_ref().expect("features.json");
    assert!(f.segments.len() > 10_000);
    let start = w
        .marker(MarkerKind::TrailStart)
        .expect("начало тропы")
        .position;
    let g = w.sample_ground(start.x, start.z).unwrap();
    assert!(g.on_trail, "начало тропы не на дороге или тропе");
    assert!(
        (600.0..720.0).contains(&g.height),
        "высота начала {}",
        g.height
    );
    let mouth = w.marker(MarkerKind::RiverMouth).unwrap().position;
    // Расстояние известно только рядом с полосами: ищем воду реки в 40 м.
    let near_river = (-8..=8).any(|i| {
        (-8..=8).any(|j| {
            f.sample(mouth.x + i as f32 * 5.0, mouth.z + j as f32 * 5.0)
                .river_m
                < 0.0
        })
    });
    assert!(near_river, "устье дальше 40 м от реки");
    let jomon = w
        .markers
        .iter()
        .find(|m| m.name == "Дзёмон-суги")
        .unwrap()
        .position;
    let h = w.sample_ground(jomon.x, jomon.z).unwrap().height;
    assert!((1250.0..1380.0).contains(&h), "Дзёмон-суги на {h} м");
}
