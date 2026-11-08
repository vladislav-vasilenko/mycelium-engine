//! Тесты слоёв карты: экспорт, импорт, замены, ошибки.

use super::*;

fn tmp(name: &str) -> std::path::PathBuf {
    let d = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target/test-layers")
        .join(name);
    let _ = std::fs::remove_dir_all(&d);
    d
}

#[test]
fn weights_encode_to_exact_255_and_round_trip() {
    for w in [
        BiomeWeights {
            forest: 1.0 / 3.0,
            peat: 1.0 / 3.0,
            shore: 1.0 / 3.0,
        },
        BiomeWeights {
            forest: 0.7,
            peat: 0.2,
            shore: 0.1,
        },
        BiomeWeights {
            forest: 0.0,
            peat: 0.0,
            shore: 1.0,
        },
    ] {
        let e = encode_weights(&w);
        assert_eq!(e.iter().map(|&v| v as u32).sum::<u32>(), 255);
        let d = decode_weights(e.map(|v| v as u16));
        assert_eq!(encode_weights(&d), e);
    }
}

#[test]
fn export_import_round_trip_is_stable_and_close() {
    let generated = World::generate(0xEC40, 3, 2);
    let a = tmp("round-trip-a");
    let b = tmp("round-trip-b");
    export(&generated, &a).unwrap();
    let imported = import(&a).unwrap();
    export(&imported, &b).unwrap();
    // Источники после импорта и повторного экспорта совпадают побайтно.
    for name in ["height.png", "masks.png", "climate.png", "coast_type.png"] {
        assert_eq!(
            std::fs::read(a.join(name)).unwrap(),
            std::fs::read(b.join(name)).unwrap(),
            "{name}"
        );
    }
    // Импорт детерминирован.
    assert_eq!(imported.checksum(), import(&a).unwrap().checksum());
    // Высоты отличаются не больше половины шага квантования.
    let step = (HEIGHT_RANGE_M[1] - HEIGHT_RANGE_M[0]) / 65535.0;
    let mut same_substrate = 0;
    let mut total = 0;
    for (tg, ti) in generated.tiles.iter().zip(&imported.tiles) {
        assert_eq!(tg.id, ti.id);
        for (sg, si) in tg.samples.iter().zip(&ti.samples) {
            assert!((sg.height - si.height).abs() <= 0.5 * step + 1e-5);
            same_substrate += (sg.substrate == si.substrate) as usize;
            total += 1;
        }
    }
    assert!(
        same_substrate as f32 / total as f32 > 0.99,
        "субстрат совпал в {same_substrate} из {total}"
    );
    assert_eq!(imported.markers.len(), generated.markers.len());
    // Стыки тайлов после импорта по-прежнему общие.
    let t0 = imported.tile(super::super::TileId { x: 0, z: 0 }).unwrap();
    let t1 = imported.tile(super::super::TileId { x: 1, z: 0 }).unwrap();
    for j in 0..TILE_SAMPLES {
        assert_eq!(t0.at(TILE_SAMPLES - 1, j), t1.at(0, j));
    }
}

#[test]
fn overrides_replace_biome_and_substrate() {
    let generated = World::generate(0xEC40, 2, 2);
    let dir = tmp("overrides");
    export(&generated, &dir).unwrap();
    let (nx, nz) = (129, 129);
    // Весь биом — торф, весь субстрат — камень.
    let biome: Vec<u8> = (0..nx * nz).flat_map(|_| [0u8, 255, 0]).collect();
    let substrate = vec![3 * SUBSTRATE_STEP; nx * nz];
    use png::{BitDepth::*, ColorType::*};
    write_png(&dir.join("biome.png"), (nx, nz), Rgb, Eight, &biome).unwrap();
    write_png(
        &dir.join("substrate.png"),
        (nx, nz),
        Grayscale,
        Eight,
        &substrate,
    )
    .unwrap();

    // Без overrides правленые слои не используются.
    let w = import(&dir).unwrap();
    assert!(w.tiles[0].samples.iter().any(|s| s.weights.peat < 0.5));

    let path = dir.join(MANIFEST);
    let mut m: Manifest = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    m.overrides = vec!["biome".into(), "substrate".into()];
    std::fs::write(&path, serde_json::to_string(&m).unwrap()).unwrap();
    let w = import(&dir).unwrap();
    assert!(
        w.tiles
            .iter()
            .flat_map(|t| &t.samples)
            .all(|s| s.weights.peat == 1.0 && s.substrate == Substrate::Rock)
    );
    let g = w.sample_ground(10.0, 10.0).unwrap();
    assert_eq!(g.substrate, Substrate::Rock);
}

#[test]
fn rejects_wrong_size_and_8_bit_height() {
    let generated = World::generate(0xEC40, 2, 2);
    let dir = tmp("reject");
    export(&generated, &dir).unwrap();
    use png::{BitDepth::*, ColorType::*};
    write_png(
        &dir.join("height.png"),
        (129, 129),
        Grayscale,
        Eight,
        &vec![128; 129 * 129],
    )
    .unwrap();
    let e = import(&dir).err().unwrap();
    assert!(e.contains("16 бит"), "{e}");
    write_png(
        &dir.join("height.png"),
        (100, 129),
        Grayscale,
        Sixteen,
        &vec![0; 200 * 129],
    )
    .unwrap();
    let e = import(&dir).err().unwrap();
    assert!(e.contains("размер"), "{e}");
}

/// Карта для глаз и слои первой карты: `cargo test export_first_map -- --ignored`.
#[test]
#[ignore]
fn export_first_map() {
    let w = World::generate(
        0xEC40,
        crate::world::FIRST_MAP_TILES,
        crate::world::FIRST_MAP_TILES,
    );
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/world-layers");
    let m = export(&w, &dir).unwrap();
    println!("{} → {}", m.exported_checksum, dir.display());
}
