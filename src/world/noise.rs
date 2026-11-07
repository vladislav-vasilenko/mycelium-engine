//! Детерминированный шум для генерации мира.
//!
//! Только целочисленный хеш и арифметика f32 без трансцендентных функций,
//! поэтому результат не зависит от libm и совпадает на Mac и Windows.

/// Перемешивание 64 бит (splitmix64).
pub fn mix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// Хеш точки решётки в [0, 1).
pub fn hash01(seed: u64, ix: i32, iz: i32) -> f32 {
    let h = mix64(seed ^ mix64((ix as u32 as u64) | ((iz as u32 as u64) << 32)));
    (h >> 40) as f32 / (1u64 << 24) as f32
}

fn fade(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

/// Value noise в [−1, 1] с квинтической интерполяцией.
pub fn value_noise(seed: u64, x: f32, z: f32) -> f32 {
    let x0 = x.floor();
    let z0 = z.floor();
    let (ix, iz) = (x0 as i32, z0 as i32);
    let (fx, fz) = (fade(x - x0), fade(z - z0));
    let a = hash01(seed, ix, iz);
    let b = hash01(seed, ix + 1, iz);
    let c = hash01(seed, ix, iz + 1);
    let d = hash01(seed, ix + 1, iz + 1);
    let top = a + (b - a) * fx;
    let bottom = c + (d - c) * fx;
    (top + (bottom - top) * fz) * 2.0 - 1.0
}

/// Фрактальный шум: сумма октав, нормированная в [−1, 1].
pub fn fbm(seed: u64, x: f32, z: f32, octaves: u32) -> f32 {
    let mut sum = 0.0;
    let mut amp = 1.0;
    let mut norm = 0.0;
    let (mut fx, mut fz) = (x, z);
    for o in 0..octaves {
        sum += amp * value_noise(seed.wrapping_add(o as u64 * 0x1000_0001), fx, fz);
        norm += amp;
        amp *= 0.5;
        // Поворот октавы убирает выравнивание решёток по осям.
        let (rx, rz) = (fx * 1.6 - fz * 1.2, fx * 1.2 + fz * 1.6);
        fx = rx;
        fz = rz;
    }
    sum / norm
}

pub fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_is_deterministic_and_bounded() {
        for i in 0..1000 {
            let (x, z) = (i as f32 * 0.37 - 50.0, i as f32 * 0.91 + 3.0);
            let a = fbm(42, x, z, 5);
            assert_eq!(a, fbm(42, x, z, 5));
            assert!((-1.0..=1.0).contains(&a));
            assert!((0.0..1.0).contains(&hash01(7, i, -i)));
        }
        assert_ne!(fbm(1, 10.5, 3.25, 4), fbm(2, 10.5, 3.25, 4));
    }

    #[test]
    fn value_noise_is_continuous_across_cells() {
        let s = 99;
        for i in -5..5 {
            let edge = i as f32;
            let left = value_noise(s, edge - 1e-4, 0.3);
            let right = value_noise(s, edge + 1e-4, 0.3);
            assert!((left - right).abs() < 1e-3);
        }
    }
}
