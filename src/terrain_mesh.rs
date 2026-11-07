//! Сетка ландшафта для GPU из запечённых тайлов мира.
//!
//! Все тайлы LOD 0 лежат в одном буфере вершин подряд: тайл `k` занимает
//! вершины `k * 4225 .. (k + 1) * 4225`. Индексы общие для всех тайлов
//! (65 × 65 точек, 64 × 64 ячейки × 2 треугольника), поэтому ландшафт рисуется
//! одним instanced indexed draw: номер экземпляра — номер тайла.

use crate::layout::TerrainVertex;
use crate::world::{TILE_SAMPLES, World};

pub const VERTICES_PER_TILE: usize = TILE_SAMPLES * TILE_SAMPLES;

/// Индексы одной сетки 65 × 65. Диагональ ячейки (i, j) → (i+1, j+1),
/// треугольники (a, c, b) и (a, d, c), обход CCW при взгляде сверху —
/// тот же разбор, что в `World::sample_ground`.
pub fn tile_indices() -> Vec<u16> {
    let n = TILE_SAMPLES as u16;
    let mut idx = Vec::with_capacity((TILE_SAMPLES - 1) * (TILE_SAMPLES - 1) * 6);
    for j in 0..n - 1 {
        for i in 0..n - 1 {
            let a = j * n + i;
            let b = a + 1;
            let d = a + n;
            let c = d + 1;
            idx.extend_from_slice(&[a, c, b, a, d, c]);
        }
    }
    idx
}

fn unorm8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// Вершины всех тайлов в порядке `world.tiles`.
pub fn vertices(world: &World) -> Vec<TerrainVertex> {
    let mut out = Vec::with_capacity(world.tiles.len() * VERTICES_PER_TILE);
    for tile in &world.tiles {
        let origin = tile.id.origin();
        for j in 0..TILE_SAMPLES {
            for i in 0..TILE_SAMPLES {
                let s = tile.at(i, j);
                out.push(TerrainVertex {
                    position: [origin.x + i as f32, s.height, origin.y + j as f32],
                    normal: s.normal.to_array(),
                    weights: [
                        unorm8(s.weights.forest),
                        unorm8(s.weights.peat),
                        unorm8(s.weights.shore),
                        unorm8(s.trail),
                    ],
                    material: [
                        s.substrate as u8,
                        unorm8(s.river),
                        unorm8(s.moisture),
                        unorm8(s.rocky),
                    ],
                });
            }
        }
    }
    out
}

/// Детальная текстура грунта: бесшовный шум, RGBA8, `size × size`.
/// R — мелкая зернистость, G — крупные пятна, B — трещины/камешки.
pub fn detail_texture(seed: u64, size: usize) -> Vec<u8> {
    use crate::world::noise::{hash01, smoothstep};
    // Периодический value noise: решётка по модулю `period`.
    let periodic = |salt: u64, x: f32, z: f32, period: i32| {
        let (x0, z0) = (x.floor(), z.floor());
        let (ix, iz) = (x0 as i32, z0 as i32);
        let f = |t: f32| t * t * (3.0 - 2.0 * t);
        let (fx, fz) = (f(x - x0), f(z - z0));
        let h = |a: i32, b: i32| hash01(seed ^ salt, a.rem_euclid(period), b.rem_euclid(period));
        let top = h(ix, iz) + (h(ix + 1, iz) - h(ix, iz)) * fx;
        let bot = h(ix, iz + 1) + (h(ix + 1, iz + 1) - h(ix, iz + 1)) * fx;
        top + (bot - top) * fz
    };
    let octaves = |salt: u64, u: f32, v: f32, base: i32, count: u32| {
        let (mut sum, mut amp, mut norm, mut period) = (0.0, 1.0, 0.0, base);
        for o in 0..count {
            let k = period as f32;
            sum += amp * periodic(salt + o as u64, u * k, v * k, period);
            norm += amp;
            amp *= 0.5;
            period *= 2;
        }
        sum / norm
    };
    let mut px = Vec::with_capacity(size * size * 4);
    for y in 0..size {
        for x in 0..size {
            let (u, v) = (x as f32 / size as f32, y as f32 / size as f32);
            let grain = octaves(1, u, v, 32, 4);
            let blotch = octaves(2, u, v, 4, 4);
            let pebble = smoothstep(0.62, 0.8, octaves(3, u, v, 24, 2));
            px.extend_from_slice(&[unorm8(grain), unorm8(blotch), unorm8(pebble), 255]);
        }
    }
    px
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    #[test]
    fn tile_triangles_face_up_and_cover_grid() {
        let idx = tile_indices();
        assert_eq!(idx.len(), 64 * 64 * 6);
        let pos = |k: u16| {
            let k = k as usize;
            Vec3::new((k % TILE_SAMPLES) as f32, 0.0, (k / TILE_SAMPLES) as f32)
        };
        for tri in idx.chunks(3) {
            let n = (pos(tri[1]) - pos(tri[0])).cross(pos(tri[2]) - pos(tri[0]));
            assert!(n.y > 0.0, "треугольник {tri:?} смотрит вниз");
        }
    }

    #[test]
    fn mesh_vertices_match_world_samples() {
        let world = World::generate(0xEC40, 2, 2);
        let v = vertices(&world);
        assert_eq!(v.len(), 4 * VERTICES_PER_TILE);
        let t = &world.tiles[3];
        let k = 3 * VERTICES_PER_TILE + 10 * TILE_SAMPLES + 7;
        assert_eq!(v[k].position[1], t.at(7, 10).height);
        assert_eq!(v[k].position[0], t.id.origin().x + 7.0);
    }

    #[test]
    fn detail_texture_tiles_seamlessly() {
        // Шов x = n−1 → 0 не должен давать перепада больше, чем соседние
        // пиксели внутри текстуры: тогда повтор незаметен.
        let n = 128;
        let px = detail_texture(5, n);
        let at = |x: usize, y: usize, c: usize| px[(y * n + x) * 4 + c] as i32;
        for c in 0..3 {
            let mut interior = 0;
            let mut seam = 0;
            for y in 0..n {
                for x in 0..n - 1 {
                    interior = interior.max((at(x, y, c) - at(x + 1, y, c)).abs());
                }
                seam = seam.max((at(n - 1, y, c) - at(0, y, c)).abs());
                seam = seam.max((at(y, n - 1, c) - at(y, 0, c)).abs());
            }
            assert!(seam <= interior, "канал {c}: шов {seam}, внутри {interior}");
        }
    }
}
