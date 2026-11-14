//! Положение солнца, луны и звёздного неба (04.1).
//!
//! Солнце — алгоритм NOAA (склонение, уравнение времени, часовой угол;
//! точность ~0,01° в 1800–2100 гг.). Луна — упрощённая орбита по П. Шлютеру
//! («How to compute planetary positions»): наклон, прецессия узлов, главные
//! возмущения (~0,3°). Звёзды — поворот экваториальных координат на местное
//! звёздное время. Мировые оси: x — восток, y — вверх, z — юг.

use glam::{DMat3, DVec3, Vec3};

/// Юлианская дата по календарной дате UTC (григорианский календарь).
pub fn julian_day(year: i32, month: u32, day: u32, hours_utc: f64) -> f64 {
    let (y, m) = if month <= 2 {
        (year - 1, month + 12)
    } else {
        (year, month)
    };
    let a = (y as f64 / 100.0).floor();
    let b = 2.0 - a + (a / 4.0).floor();
    (365.25 * (y as f64 + 4716.0)).floor() + (30.6001 * (m as f64 + 1.0)).floor() + day as f64 + b
        - 1524.5
        + hours_utc / 24.0
}

/// Место наблюдателя.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Site {
    pub lat_deg: f64,
    /// Восточная долгота, градусы.
    pub lon_deg: f64,
}

/// Высота и азимут (от севера по часовой стрелке), градусы.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Horizontal {
    pub alt_deg: f64,
    pub az_deg: f64,
}

impl Horizontal {
    /// Единичный вектор в мировых осях (x — восток, y — вверх, z — юг).
    pub fn world_dir(&self) -> Vec3 {
        let (h, a) = (self.alt_deg.to_radians(), self.az_deg.to_radians());
        Vec3::new(
            (a.sin() * h.cos()) as f32,
            h.sin() as f32,
            (-a.cos() * h.cos()) as f32,
        )
    }
}

fn rev(x: f64) -> f64 {
    x.rem_euclid(360.0)
}

fn sind(x: f64) -> f64 {
    x.to_radians().sin()
}

fn cosd(x: f64) -> f64 {
    x.to_radians().cos()
}

/// Данные NOAA для солнца на момент `jd` (UTC).
struct SunNoaa {
    decl_deg: f64,
    /// Уравнение времени, минуты.
    eot_min: f64,
    /// Видимая эклиптическая долгота, градусы.
    app_long_deg: f64,
    obliq_deg: f64,
}

fn sun_noaa(jd: f64) -> SunNoaa {
    let jc = (jd - 2_451_545.0) / 36_525.0;
    let l0 = rev(280.46646 + jc * (36_000.769_83 + jc * 0.000_303_2));
    let m = 357.52911 + jc * (35_999.050_29 - 0.000_153_7 * jc);
    let e = 0.016_708_634 - jc * (0.000_042_037 + 0.000_000_126_7 * jc);
    let c = sind(m) * (1.914602 - jc * (0.004817 + 0.000014 * jc))
        + sind(2.0 * m) * (0.019993 - 0.000101 * jc)
        + sind(3.0 * m) * 0.000289;
    let true_long = l0 + c;
    let omega = 125.04 - 1934.136 * jc;
    let app_long = true_long - 0.00569 - 0.00478 * sind(omega);
    let obliq0 =
        23.0 + (26.0 + (21.448 - jc * (46.815 + jc * (0.00059 - jc * 0.001813))) / 60.0) / 60.0;
    let obliq = obliq0 + 0.00256 * cosd(omega);
    let decl = (sind(obliq) * sind(app_long)).asin().to_degrees();
    let y = (obliq.to_radians() / 2.0).tan().powi(2);
    let (l0r, mr) = (l0.to_radians(), m.to_radians());
    let eot = 4.0
        * (y * (2.0 * l0r).sin() - 2.0 * e * mr.sin() + 4.0 * e * y * mr.sin() * (2.0 * l0r).cos()
            - 0.5 * y * y * (4.0 * l0r).sin()
            - 1.25 * e * e * (2.0 * mr).sin())
        .to_degrees();
    SunNoaa {
        decl_deg: decl,
        eot_min: eot,
        app_long_deg: app_long,
        obliq_deg: obliq,
    }
}

fn horizontal_from_hour_angle(site: Site, hour_angle_deg: f64, decl_deg: f64) -> Horizontal {
    let (phi, d, h) = (site.lat_deg, decl_deg, hour_angle_deg);
    let cos_z = (sind(phi) * sind(d) + cosd(phi) * cosd(d) * cosd(h)).clamp(-1.0, 1.0);
    let alt = 90.0 - cos_z.acos().to_degrees();
    // Азимут от севера: atan2(−sin H, tan δ · cos φ − sin φ · cos H).
    let az = rev((-sind(h))
        .atan2(d.to_radians().tan() * cosd(phi) - sind(phi) * cosd(h))
        .to_degrees());
    Horizontal {
        alt_deg: alt,
        az_deg: az,
    }
}

/// Солнце: геометрическая высота (без рефракции) и азимут.
pub fn sun_position(site: Site, jd: f64) -> Horizontal {
    let s = sun_noaa(jd);
    let minutes_utc = (jd + 0.5).fract() * 1440.0;
    let true_solar = (minutes_utc + s.eot_min + 4.0 * site.lon_deg).rem_euclid(1440.0);
    let hour_angle = true_solar / 4.0 - 180.0;
    horizontal_from_hour_angle(site, hour_angle, s.decl_deg)
}

/// Восход и заход солнца (верхний край с рефракцией, зенит 90,833°) в минутах
/// (сверка с таблицами в тестах; понадобится расписанию дня в 05).
#[cfg_attr(not(test), allow(dead_code))]
/// UTC от полуночи даты `jd_midnight`; `None` — полярный день или ночь.
pub fn sunrise_sunset_utc_min(site: Site, jd_midnight: f64) -> Option<(f64, f64)> {
    // Две итерации: склонение и уравнение времени на момент события.
    let mut noon = 720.0 - 4.0 * site.lon_deg;
    let mut out = (0.0, 0.0);
    for _ in 0..2 {
        let s = sun_noaa(jd_midnight + noon / 1440.0);
        noon = 720.0 - 4.0 * site.lon_deg - s.eot_min;
        let cos_ha = cosd(90.833) / (cosd(site.lat_deg) * cosd(s.decl_deg))
            - site.lat_deg.to_radians().tan() * s.decl_deg.to_radians().tan();
        if cos_ha.abs() > 1.0 {
            return None;
        }
        let ha = cos_ha.acos().to_degrees();
        out = (noon - 4.0 * ha, noon + 4.0 * ha);
    }
    Some(out)
}

/// Среднее звёздное время Гринвича, градусы.
pub fn gmst_deg(jd: f64) -> f64 {
    let t = (jd - 2_451_545.0) / 36_525.0;
    rev(280.460_618_37 + 360.985_647_366_29 * (jd - 2_451_545.0) + 0.000_387_933 * t * t)
}

/// Поворот экваториальных координат (x — точка весны, z — северный полюс мира)
/// в мировые оси наблюдателя на момент `jd`.
///
/// 1. Часовая система: x — точка экватора на меридиане, y — запад, z — полюс;
///    часовой угол H = LST − α, поэтому (x, y) = (cos L·eₓ + sin L·e_y,
///    sin L·eₓ − cos L·e_y) — поворот на −LST и отражение y.
/// 2. Горизонт (север, запад, зенит): полюс на высоте φ над севером, точка
///    экватора на меридиане — на высоте 90° − φ над югом.
/// 3. Мир: x = восток = −запад, y = зенит, z = юг = −север.
pub fn equatorial_to_world(site: Site, jd: f64) -> DMat3 {
    let lst = (gmst_deg(jd) + site.lon_deg).to_radians();
    let phi = site.lat_deg.to_radians();
    let flip_y = DMat3::from_diagonal(DVec3::new(1.0, -1.0, 1.0));
    let to_hour = flip_y * DMat3::from_rotation_z(-lst);
    let hour_to_nwz = DMat3::from_cols(
        DVec3::new(-phi.sin(), 0.0, phi.cos()),
        DVec3::new(0.0, 1.0, 0.0),
        DVec3::new(phi.cos(), 0.0, phi.sin()),
    );
    let nwz_to_world = DMat3::from_cols(
        DVec3::new(0.0, 0.0, -1.0),
        DVec3::new(-1.0, 0.0, 0.0),
        DVec3::new(0.0, 1.0, 0.0),
    );
    nwz_to_world * hour_to_nwz * to_hour
}

/// Направление из прямого восхождения и склонения (градусы) в экваториальные оси
/// (звёзды на GPU приходят уже в этих осях из `stars.bin`).
#[cfg(test)]
pub fn equatorial_dir(ra_deg: f64, dec_deg: f64) -> DVec3 {
    DVec3::new(
        cosd(dec_deg) * cosd(ra_deg),
        cosd(dec_deg) * sind(ra_deg),
        sind(dec_deg),
    )
}

/// Луна: геоцентрическое направление в экваториальных осях и освещённая доля.
pub struct MoonState {
    pub equatorial: DVec3,
    /// Освещённая доля диска 0…1.
    pub illuminated: f64,
    /// Возраст от новолуния, сутки (0…29,53).
    pub age_days: f64,
}

/// Синодический месяц, сутки.
pub const SYNODIC_MONTH: f64 = 29.530_588_853;

pub fn moon_state(jd: f64) -> MoonState {
    let d = jd - 2_451_543.5;
    // Элементы орбиты (Шлютер).
    let n = rev(125.1228 - 0.052_953_808_3 * d);
    let i = 5.1454;
    let w = rev(318.0634 + 0.164_357_322_3 * d);
    let e: f64 = 0.054900;
    let m = rev(115.3654 + 13.064_992_950_9 * d);
    // Уравнение Кеплера.
    let mut ea = m + e.to_degrees() * sind(m) * (1.0 + e * cosd(m));
    for _ in 0..5 {
        ea -= (ea - e.to_degrees() * sind(ea) - m) / (1.0 - e * cosd(ea));
    }
    let xv = cosd(ea) - e;
    let yv = (1.0 - e * e).sqrt() * sind(ea);
    let v = yv.atan2(xv).to_degrees();
    let r = (xv * xv + yv * yv).sqrt();
    let x = r * (cosd(n) * cosd(v + w) - sind(n) * sind(v + w) * cosd(i));
    let y = r * (sind(n) * cosd(v + w) + cosd(n) * sind(v + w) * cosd(i));
    let mut lon = rev(y.atan2(x).to_degrees());
    let z = r * sind(v + w) * sind(i);
    let mut lat = z.atan2((r * r - z * z).max(0.0).sqrt()).to_degrees();
    // Главные возмущения.
    let ms = rev(356.0470 + 0.985_600_258_5 * d);
    let ws = 282.9404 + 0.000_047_093_5 * d;
    let ls = ms + ws;
    let lm = m + w + n;
    let dd = lm - ls;
    let f = lm - n;
    lon += -1.274 * sind(m - 2.0 * dd) + 0.658 * sind(2.0 * dd)
        - 0.186 * sind(ms)
        - 0.059 * sind(2.0 * m - 2.0 * dd)
        - 0.057 * sind(m - 2.0 * dd + ms)
        + 0.053 * sind(m + 2.0 * dd)
        + 0.046 * sind(2.0 * dd - ms)
        + 0.041 * sind(m - ms)
        - 0.035 * sind(dd)
        - 0.031 * sind(m + ms)
        - 0.015 * sind(2.0 * f - 2.0 * dd)
        + 0.011 * sind(m - 4.0 * dd);
    lat += -0.173 * sind(f - 2.0 * dd)
        - 0.055 * sind(m - f - 2.0 * dd)
        - 0.046 * sind(m + f - 2.0 * dd)
        + 0.033 * sind(f + 2.0 * dd)
        + 0.017 * sind(2.0 * m + f);
    // Эклиптика → экватор.
    let s = sun_noaa(jd);
    let ecl = DVec3::new(cosd(lat) * cosd(lon), cosd(lat) * sind(lon), sind(lat));
    let eps = s.obliq_deg.to_radians();
    let equatorial = DMat3::from_rotation_x(eps) * ecl;
    // Фаза: элонгация от солнца по эклиптической долготе и широте.
    let sun_ecl = DVec3::new(cosd(s.app_long_deg), sind(s.app_long_deg), 0.0);
    let elong = ecl.dot(sun_ecl).clamp(-1.0, 1.0).acos();
    let illuminated = (1.0 - elong.cos()) / 2.0;
    let age = rev(lon - s.app_long_deg) / 360.0 * SYNODIC_MONTH;
    MoonState {
        equatorial,
        illuminated,
        age_days: age,
    }
}

/// Солнце в экваториальных осях (сверка двух путей расчёта в тестах).
#[cfg(test)]
pub fn sun_equatorial(jd: f64) -> DVec3 {
    let s = sun_noaa(jd);
    let ecl = DVec3::new(cosd(s.app_long_deg), sind(s.app_long_deg), 0.0);
    DMat3::from_rotation_x(s.obliq_deg.to_radians()) * ecl
}
