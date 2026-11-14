//! Игровое время и дата (04.1). Время идёт от тиков симуляции, поэтому один и
//! тот же запуск даёт одно и то же небо.

use super::celestial::julian_day;

/// Часы, минуты и дата острова; ход ускорен, можно остановить и задать.
#[derive(Clone, Copy, Debug)]
pub struct TimeOfDay {
    /// Момент в юлианских днях UTC.
    pub jd_utc: f64,
    /// Часовой пояс острова, часы (Якусима: UTC+9).
    pub tz_hours: f64,
    /// Скорость хода: 1 — реальное время, 60 — сутки за 24 минуты.
    pub scale: f64,
    pub paused: bool,
}

/// Скорость хода по умолчанию: сутки за 24 минуты.
pub const DEFAULT_SCALE: f64 = 60.0;

impl TimeOfDay {
    /// Местное время `hours_local` дня `year-month-day` в поясе `tz_hours`.
    pub fn new(year: i32, month: u32, day: u32, hours_local: f64, tz_hours: f64) -> TimeOfDay {
        TimeOfDay {
            jd_utc: julian_day(year, month, day, hours_local - tz_hours),
            tz_hours,
            scale: DEFAULT_SCALE,
            paused: false,
        }
    }

    /// Ход времени на `dt_s` реальных секунд с учётом скорости и паузы.
    pub fn advance(&mut self, dt_s: f64) {
        if !self.paused {
            self.jd_utc += dt_s * self.scale / 86_400.0;
        }
    }

    /// Сдвиг на `hours` игровых часов (клавиши `[` и `]`).
    pub fn shift_hours(&mut self, hours: f64) {
        self.jd_utc += hours / 24.0;
    }

    /// Местные часы 0…24.
    #[cfg(test)]
    pub fn local_hours(&self) -> f64 {
        ((self.jd_utc + 0.5) * 24.0 + self.tz_hours).rem_euclid(24.0)
    }

    /// Задать местное время того же местного дня.
    #[cfg(test)]
    pub fn set_local_hours(&mut self, hours: f64) {
        let delta = hours - self.local_hours();
        self.jd_utc += delta / 24.0;
    }

    /// Местные дата и время для заголовка: «2026-11-14 07:30».
    pub fn label(&self) -> String {
        let jd = self.jd_utc + self.tz_hours / 24.0 + 0.5;
        let z = jd.floor();
        let f = jd - z;
        // Обратное преобразование Мееуса (григорианский календарь).
        let alpha = ((z - 1_867_216.25) / 36_524.25).floor();
        let a = z + 1.0 + alpha - (alpha / 4.0).floor();
        let b = a + 1524.0;
        let c = ((b - 122.1) / 365.25).floor();
        let d = (365.25 * c).floor();
        let e = ((b - d) / 30.6001).floor();
        let day = b - d - (30.6001 * e).floor();
        let month = if e < 14.0 { e - 1.0 } else { e - 13.0 };
        let year = if month > 2.0 { c - 4716.0 } else { c - 4715.0 };
        // Полсекунды запаса: 18:14:59,9999 из-за округления — это 18:15.
        let minutes = ((f * 1440.0 + 1.0 / 120.0).floor() as u32).min(1439);
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}",
            year as i32,
            month as u32,
            day as u32,
            minutes / 60,
            minutes % 60
        )
    }
}
