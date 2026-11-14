//! Кэш собранной иерархии на диске: сборка острова занимает десятки секунд,
//! загрузка — доли секунды. Ключ — хеш поля высот и версии формата; файл в
//! `target/terrain-cache/` вне git. Ошибка чтения или записи не мешает игре:
//! иерархия просто собирается заново.

use std::io::{Read, Write};
use std::path::PathBuf;

use glam::Vec3;

#[cfg(test)]
use super::build;
use super::{Bounds, ClusterInfo, ClusterSet, Node, build_world};
use crate::layout::TerrainCluster;
use crate::world::heightfield::HeightField;

/// Версия формата: меняется при изменении сборки или раскладки файла.
const VERSION: u32 = 2;
const MAGIC: &[u8; 4] = b"ECLU";

/// Хеш поля высот (размер, шаг, все высоты побитно) и версии формата.
pub fn key(hf: &HeightField) -> u64 {
    use crate::world::noise::mix64;
    let mut h = mix64(VERSION as u64 ^ (hf.nx as u64) << 32 ^ hf.nz as u64);
    h = mix64(h ^ hf.step_m.to_bits() as u64);
    for v in &hf.heights {
        h = mix64(h ^ v.to_bits() as u64);
    }
    h
}

/// Ключ мира: поле острова и, если есть, зона (положение, деление, высоты).
pub fn world_key(world: &crate::world::World) -> u64 {
    use crate::world::noise::mix64;
    let mut h = key(&world.heightfield);
    if let Some(z) = &world.zone {
        h = mix64(h ^ (z.i0 as u64) << 40 ^ (z.j0 as u64) << 20 ^ (z.cells * 64 + z.ratio) as u64);
        h ^= key(&z.grid).rotate_left(17);
    }
    h
}

fn path(key: u64) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target/terrain-cache")
        .join(format!("clusters-{key:016x}.bin"))
}

/// Иерархия мира из кэша или новая сборка с записью в кэш. Второй элемент —
/// взята ли из кэша.
pub fn load_or_build(world: &crate::world::World) -> (ClusterSet, bool) {
    let k = world_key(world);
    let p = path(k);
    if let Ok(bytes) = std::fs::read(&p)
        && let Some(set) = decode(&bytes, k)
    {
        return (set, true);
    }
    let set = build_world(world);
    if let Some(dir) = p.parent()
        && std::fs::create_dir_all(dir).is_ok()
        && let Ok(mut f) = std::fs::File::create(&p)
    {
        let _ = f.write_all(&encode(&set, k));
    }
    (set, false)
}

struct W(Vec<u8>);

impl W {
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn f32(&mut self, v: f32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn bounds(&mut self, b: &Bounds) {
        for v in [b.center.x, b.center.y, b.center.z, b.radius, b.error] {
            self.f32(v);
        }
    }
    fn words(&mut self, v: &[u32]) {
        self.u32(v.len() as u32);
        self.0.extend_from_slice(bytemuck::cast_slice(v));
    }
}

fn encode(set: &ClusterSet, key: u64) -> Vec<u8> {
    let mut w = W(Vec::new());
    w.0.extend_from_slice(MAGIC);
    w.u32(VERSION);
    w.0.extend_from_slice(&key.to_le_bytes());
    w.u32(set.levels);
    w.u32(set.groups.len() as u32);
    for (b, gc) in set.groups.iter().zip(&set.group_clusters) {
        w.bounds(b);
        w.u32(gc[0]);
        w.u32(gc[1]);
    }
    w.u32(set.info.len() as u32);
    for (c, g) in set.info.iter().zip(&set.gpu) {
        w.u32(c.group);
        w.u32(c.refined as u32);
        w.bounds(&c.bounds);
        w.0.extend_from_slice(bytemuck::bytes_of(g));
    }
    w.words(&set.vertices);
    w.words(&set.triangles);
    w.u32(set.nodes.len() as u32);
    for n in &set.nodes {
        w.bounds(&n.bounds);
        w.u32(n.group as u32);
        w.u32(n.child_offset);
        w.u32(n.child_count);
    }
    w.0
}

struct R<'a>(&'a [u8]);

impl R<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        if self.0.len() < n {
            return None;
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Some(a)
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn f32(&mut self) -> Option<f32> {
        Some(f32::from_bits(self.u32()?))
    }
    fn bounds(&mut self) -> Option<Bounds> {
        Some(Bounds {
            center: Vec3::new(self.f32()?, self.f32()?, self.f32()?),
            radius: self.f32()?,
            error: self.f32()?,
        })
    }
    fn words(&mut self) -> Option<Vec<u32>> {
        let n = self.u32()? as usize;
        let mut v = vec![0u32; n];
        self.take(n * 4)?
            .read_exact(bytemuck::cast_slice_mut(&mut v))
            .ok()?;
        Some(v)
    }
}

fn decode(bytes: &[u8], key: u64) -> Option<ClusterSet> {
    let mut r = R(bytes);
    if r.take(4)? != MAGIC || r.u32()? != VERSION {
        return None;
    }
    if u64::from_le_bytes(r.take(8)?.try_into().ok()?) != key {
        return None;
    }
    let mut set = ClusterSet {
        levels: r.u32()?,
        ..Default::default()
    };
    for _ in 0..r.u32()? {
        set.groups.push(r.bounds()?);
        set.group_clusters.push([r.u32()?, r.u32()?]);
    }
    for _ in 0..r.u32()? {
        set.info.push(ClusterInfo {
            group: r.u32()?,
            refined: r.u32()? as i32,
            bounds: r.bounds()?,
        });
        set.gpu
            .push(*bytemuck::from_bytes::<TerrainCluster>(r.take(16)?));
    }
    set.vertices = r.words()?;
    set.triangles = r.words()?;
    for _ in 0..r.u32()? {
        set.nodes.push(Node {
            bounds: r.bounds()?,
            group: r.u32()? as i32,
            child_offset: r.u32()?,
            child_count: r.u32()?,
        });
    }
    r.0.is_empty().then_some(set)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Запись и чтение дают ту же иерархию; другой ключ не читается.
    #[test]
    fn round_trip() {
        let n = 33;
        let hf = HeightField::new(
            n,
            n,
            1.0,
            (0..n * n).map(|k| ((k % 7) as f32 * 0.3).sin()).collect(),
        );
        let set = build(&hf);
        let k = key(&hf);
        let back = decode(&encode(&set, k), k).expect("чтение");
        assert_eq!(back.vertices, set.vertices);
        assert_eq!(back.triangles, set.triangles);
        assert_eq!(back.groups, set.groups);
        assert_eq!(back.nodes.len(), set.nodes.len());
        assert!(decode(&encode(&set, k), k ^ 1).is_none());
    }
}
