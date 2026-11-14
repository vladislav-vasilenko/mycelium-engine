//! Небо в приложении (04.1): игровое время, светила, входы неба для кадра и
//! управление временем.

use glam::{Mat4, Vec3};

use super::{App, SceneMode};
use crate::layout::SkyInputs;
use crate::sky::celestial::{self, Site};
use crate::sky::time::TimeOfDay;
use crate::world::World;

/// Шаги скорости хода времени (клавиши `-` и `=`).
const SCALES: [f64; 6] = [0.0, 1.0, 10.0, 60.0, 600.0, 3600.0];

/// Место наблюдателя: привязка мира к Земле или экватор (−3°) для генератора.
pub(super) fn site_of(world: &World) -> Site {
    match &world.geo {
        Some(g) => Site {
            lat_deg: g.center_lat,
            lon_deg: g.center_lon,
        },
        None => Site {
            lat_deg: -3.0,
            lon_deg: 0.0,
        },
    }
}

/// Небо кадра.
pub(super) struct SkyFrameData {
    pub inputs: SkyInputs,
    /// К главному светилу (солнце или луна).
    pub light_to: Vec3,
}

impl App {
    /// Солнце, луна и звёзды на текущий момент; `view_proj_rel` — для
    /// направления пикселей в проходе неба.
    pub(super) fn sky_frame(&self, view_proj_rel: Mat4, dt: f32, height_px: u32) -> SkyFrameData {
        let jd = self.time.jd_utc;
        let sun = celestial::sun_position(self.site, jd);
        let sun_dir = sun.world_dir();
        let rot = celestial::equatorial_to_world(self.site, jd);
        let moon = celestial::moon_state(jd);
        let moon_dir = (rot * moon.equatorial).as_vec3().normalize();
        // Главное светило — солнце, пока оно выше −1°; ночью — луна над горизонтом.
        let light_to = if sun.alt_deg > -1.0 || moon_dir.y < 0.0 {
            sun_dir
        } else {
            moon_dir
        };
        let r = rot.transpose(); // строки поворота
        let row = |v: glam::DVec3| [v.x as f32, v.y as f32, v.z as f32, 0.0];
        SkyFrameData {
            inputs: SkyInputs {
                sun_dir: sun_dir.extend(0.0).to_array(),
                moon_dir: moon_dir.extend(moon.illuminated as f32).to_array(),
                star_rot: [row(r.x_axis), row(r.y_axis), row(r.z_axis)],
                params: [
                    self.args.exposure.unwrap_or(0.0),
                    dt,
                    (self.time.jd_utc.fract() * 86_400.0) as f32,
                    1.0,
                ],
                inv_view_proj_rel: view_proj_rel.inverse().to_cols_array_2d(),
                // Телесный угол пикселя у центра кадра: (2·tg(FOV/2) / высота)².
                view: [
                    (2.0 / view_proj_rel.y_axis.y / height_px as f32).powi(2),
                    0.0,
                    0.0,
                    0.0,
                ],
            },
            light_to,
        }
    }

    /// Ход игрового времени на `ticks` тиков симуляции.
    pub(super) fn advance_time(&mut self, ticks: u32) {
        if self.selftest.is_none() && self.scene == SceneMode::Terrain {
            self.time.advance(ticks as f64 * crate::sim::TICK_DT);
        }
    }

    /// Клавиши времени: `[` `]` — ±1 ч, `\` — пауза, `-` `=` — скорость хода.
    pub(super) fn time_key(&mut self, code: winit::keyboard::KeyCode) -> bool {
        use winit::keyboard::KeyCode as K;
        let t = &mut self.time;
        match code {
            K::BracketLeft => t.shift_hours(-1.0),
            K::BracketRight => t.shift_hours(1.0),
            K::Backslash => t.paused = !t.paused,
            K::Minus | K::Equal => {
                let i = SCALES.iter().position(|&s| s >= t.scale).unwrap_or(3);
                let j = if code == K::Minus {
                    i.saturating_sub(1).max(1)
                } else {
                    (i + 1).min(SCALES.len() - 1)
                };
                t.scale = SCALES[j];
            }
            _ => return false,
        }
        true
    }

    /// Подпись времени для заголовка: «2026-11-14 07:30 ×60».
    pub(super) fn time_label(&self) -> String {
        let t = &self.time;
        let speed = if t.paused {
            "пауза".to_string()
        } else {
            format!("×{}", t.scale)
        };
        let jd = t.jd_utc;
        let sun = celestial::sun_position(self.site, jd).alt_deg;
        let moon = celestial::moon_state(jd);
        format!(
            "{} {speed} · солнце {sun:+.0}° · луна {:.0} % ({:.0} сут.)",
            t.label(),
            moon.illuminated * 100.0,
            moon.age_days
        )
    }
}

/// Время запуска: `--date`, `--time` или 2026-11-14 10:00 местного времени.
pub(super) fn initial_time(args: &super::Args, site: Site) -> TimeOfDay {
    let (y, m, d) = args.date.unwrap_or((2026, 11, 14));
    let tz = (site.lon_deg / 15.0).round();
    let mut t = TimeOfDay::new(y, m, d, args.time_h.unwrap_or(10.0), tz);
    if let Some(s) = args.timescale {
        t.scale = s;
    }
    t
}
