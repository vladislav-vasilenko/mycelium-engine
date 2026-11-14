use super::celestial::*;
use super::time::TimeOfDay;

const TOKYO: Site = Site {
    lat_deg: 35.6895,
    lon_deg: 139.6917,
};
const YAKUSHIMA: Site = Site {
    lat_deg: 30.335,
    lon_deg: 130.525,
};

fn hhmm_jst(min_utc: f64) -> f64 {
    (min_utc + 9.0 * 60.0).rem_euclid(1440.0)
}

/// Восход и заход в Токио (таблицы Национальной обсерватории Японии):
/// 21 июня — 4:25 и 19:00, 21 декабря — 6:47 и 16:32 по JST; допуск 3 мин.
#[test]
fn sunrise_sunset_tokyo_solstices() {
    for (y, m, d, rise, set) in [
        (2026, 6, 21, 4 * 60 + 25, 19 * 60),
        (2026, 12, 21, 6 * 60 + 47, 16 * 60 + 32),
    ] {
        // Полночь JST = 15:00 UTC предыдущих суток.
        let jd = julian_day(y, m, d, -9.0);
        let (r, s) = sunrise_sunset_utc_min(TOKYO, jd - (-9.0 / 24.0)).unwrap();
        let (r, s) = (hhmm_jst(r), hhmm_jst(s));
        assert!(
            (r - rise as f64).abs() <= 3.0,
            "{y}-{m}-{d}: восход {r:.1} мин, ждали {rise}"
        );
        assert!(
            (s - set as f64).abs() <= 3.0,
            "{y}-{m}-{d}: заход {s:.1} мин, ждали {set}"
        );
    }
}

/// Полуденная высота: равноденствие — 90° − φ, летнее солнцестояние — 90° − φ + 23,44°.
#[test]
fn noon_altitude_on_equinox_and_solstice() {
    for (m, d, decl) in [(3, 20, 0.0), (6, 21, 23.44)] {
        let midnight = julian_day(2026, m, d, 0.0);
        let mut best = -90.0f64;
        for k in 0..1440 {
            best = best.max(sun_position(YAKUSHIMA, midnight + k as f64 / 1440.0).alt_deg);
        }
        let want = 90.0 - YAKUSHIMA.lat_deg + decl;
        assert!(
            (best - want).abs() < 0.5,
            "{m}-{d}: {best:.2}° вместо {want:.2}°"
        );
    }
}

/// Два независимых пути к солнцу совпадают: часовой угол NOAA и поворот
/// экваториальных координат на звёздное время.
#[test]
fn equatorial_rotation_matches_noaa() {
    for h in [0.0, 3.5, 7.25, 12.0, 18.0, 23.0] {
        let jd = julian_day(2026, 11, 14, h);
        let a = sun_position(YAKUSHIMA, jd).world_dir().as_dvec3();
        let b = equatorial_to_world(YAKUSHIMA, jd) * sun_equatorial(jd);
        let angle = a.dot(b).clamp(-1.0, 1.0).acos().to_degrees();
        assert!(angle < 0.1, "{h} UTC: расхождение {angle:.3}°");
    }
}

/// Новолуние и солнечное затмение 2024-04-08 18:18 UTC: луна перед солнцем;
/// полнолуние 2024-04-23 23:49 UTC: диск освещён.
#[test]
fn moon_phases_on_known_dates() {
    let eclipse = julian_day(2024, 4, 8, 18.3);
    let m = moon_state(eclipse);
    let sep = m
        .equatorial
        .dot(sun_equatorial(eclipse))
        .acos()
        .to_degrees();
    assert!(sep < 1.0, "в затмение луна в {sep:.2}° от солнца");
    assert!(m.illuminated < 0.01);
    let full = moon_state(julian_day(2024, 4, 23, 23.8));
    assert!(full.illuminated > 0.99, "полнолуние: {}", full.illuminated);
    assert!((full.age_days - SYNODIC_MONTH / 2.0).abs() < 1.0);
}

/// Полярная звезда — на высоте широты над севером; Сириус в кульминации — на
/// высоте 90° − (φ − δ) над югом.
#[test]
fn stars_rotate_with_sidereal_time() {
    let polaris = equatorial_dir(37.95, 89.264);
    for h in [0.0, 6.0, 12.0, 18.0] {
        let jd = julian_day(2026, 11, 14, h);
        let d = equatorial_to_world(YAKUSHIMA, jd) * polaris;
        let alt = d.y.asin().to_degrees();
        assert!(
            (alt - YAKUSHIMA.lat_deg).abs() < 1.0,
            "Полярная на {alt:.2}°"
        );
        assert!(d.z < 0.0, "Полярная не на севере");
    }
    let sirius = equatorial_dir(101.287, -16.716);
    let mut best = (-90.0f64, 0.0f64);
    for k in 0..1440 {
        let jd = julian_day(2026, 11, 14, 0.0) + k as f64 / 1440.0;
        let d = equatorial_to_world(YAKUSHIMA, jd) * sirius;
        let alt = d.y.asin().to_degrees();
        if alt > best.0 {
            best = (alt, d.z);
        }
    }
    let want = 90.0 - (YAKUSHIMA.lat_deg + 16.716);
    assert!(
        (best.0 - want).abs() < 0.2,
        "Сириус в кульминации {:.2}°, ждали {want:.2}°",
        best.0
    );
    assert!(best.1 > 0.0, "кульминация не на юге");
}

/// Время: ход со скоростью, пауза, местные часы и подпись.
#[test]
fn time_of_day_advances_and_labels() {
    let mut t = TimeOfDay::new(2026, 11, 14, 7.5, 9.0);
    assert_eq!(t.label(), "2026-11-14 07:30");
    t.advance(60.0); // минута при 60× — час
    assert!((t.local_hours() - 8.5).abs() < 1e-6);
    t.paused = true;
    t.advance(600.0);
    assert!((t.local_hours() - 8.5).abs() < 1e-6);
    t.set_local_hours(18.25);
    assert_eq!(t.label(), "2026-11-14 18:15");
}
