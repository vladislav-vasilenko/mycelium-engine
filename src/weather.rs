//! Погода (04.3, сессия 1): пресеты «ясно», «облачно», «морось», «ливень»,
//! «гроза», плавный переход к выбранному и влажность земли.
//!
//! Время — игровое (с учётом ускорения хода суток): облака сходятся за
//! десятки игровых минут, земля мокнет за ~15 минут ливня и сохнет часами —
//! быстрее под солнцем. Шейдеры получают `uniforms()` в `FrameUniforms.weather`.

/// Пресет погоды.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    Clear,
    Cloudy,
    Drizzle,
    Rain,
    Storm,
}

impl Preset {
    pub const ALL: [Preset; 5] = [
        Preset::Clear,
        Preset::Cloudy,
        Preset::Drizzle,
        Preset::Rain,
        Preset::Storm,
    ];

    /// Имя для командной строки (`--weather`).
    pub fn id(self) -> &'static str {
        match self {
            Preset::Clear => "clear",
            Preset::Cloudy => "cloudy",
            Preset::Drizzle => "drizzle",
            Preset::Rain => "rain",
            Preset::Storm => "storm",
        }
    }

    /// Подпись для заголовка окна.
    pub fn label(self) -> &'static str {
        match self {
            Preset::Clear => "ясно",
            Preset::Cloudy => "облачно",
            Preset::Drizzle => "морось",
            Preset::Rain => "ливень",
            Preset::Storm => "гроза",
        }
    }

    pub fn from_id(s: &str) -> Option<Preset> {
        Preset::ALL.into_iter().find(|p| p.id() == s)
    }

    /// (облачность 0…1, сила дождя 0…1, ветер 0…1).
    fn target(self) -> [f32; 3] {
        match self {
            Preset::Clear => [0.05, 0.0, 0.2],
            Preset::Cloudy => [0.75, 0.0, 0.35],
            Preset::Drizzle => [0.9, 0.25, 0.3],
            Preset::Rain => [1.0, 0.75, 0.55],
            Preset::Storm => [1.0, 1.0, 1.0],
        }
    }

    pub fn next(self) -> Preset {
        let i = Preset::ALL.iter().position(|&p| p == self).unwrap_or(0);
        Preset::ALL[(i + 1) % Preset::ALL.len()]
    }
}

/// Постоянная времени перехода облачности, дождя и ветра, игровые секунды.
const BLEND_S: f32 = 900.0;
/// Ливень (сила 1) мочит сухую землю за ~столько игровых секунд.
const WET_S: f32 = 900.0;
/// Сохнет без дождя под пасмурным небом (~12 ч); под солнцем — втрое быстрее:
/// после ливня на Якусиме сыро до вечера.
const DRY_S: f32 = 12.0 * 3600.0;

#[derive(Clone, Debug)]
pub struct Weather {
    pub preset: Preset,
    pub cloud: f32,
    pub rain: f32,
    pub wind: f32,
    /// Влажность земли 0…1 (общая для округи; местные различия — в шейдере).
    pub ground_wet: f32,
}

impl Weather {
    /// Сразу в состоянии пресета (без перехода); земля мокрая, если идёт дождь.
    pub fn new(preset: Preset) -> Weather {
        let [cloud, rain, wind] = preset.target();
        Weather {
            preset,
            cloud,
            rain,
            wind,
            ground_wet: if rain > 0.0 {
                (rain * 1.2).min(1.0)
            } else {
                0.0
            },
        }
    }

    /// Новая цель; переход — в `update`.
    pub fn set(&mut self, preset: Preset) {
        self.preset = preset;
    }

    /// Шаг на `dt` игровых секунд.
    pub fn update(&mut self, dt: f32) {
        let dt = dt.max(0.0);
        let [tc, tr, tw] = self.preset.target();
        let k = 1.0 - (-dt / BLEND_S).exp();
        self.cloud += (tc - self.cloud) * k;
        self.rain += (tr - self.rain) * k;
        self.wind += (tw - self.wind) * k;
        if self.rain > 0.02 {
            self.ground_wet += dt * self.rain / WET_S * (1.0 - self.ground_wet);
        } else {
            let sun = 1.0 + 2.0 * (1.0 - self.cloud);
            self.ground_wet -= dt * sun / DRY_S;
        }
        self.ground_wet = self.ground_wet.clamp(0.0, 1.0);
    }

    /// `FrameUniforms.weather`: x — облачность, y — дождь, z — влажность земли, w — ветер.
    pub fn uniforms(&self) -> [f32; 4] {
        [self.cloud, self.rain, self.ground_wet, self.wind]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transition_is_smooth_and_converges() {
        let mut w = Weather::new(Preset::Clear);
        w.set(Preset::Rain);
        let mut last = w.cloud;
        for _ in 0..600 {
            w.update(10.0);
            assert!(w.cloud >= last - 1e-6, "облачность растёт монотонно");
            assert!(w.cloud - last < 0.05, "без скачков");
            last = w.cloud;
        }
        assert!((w.cloud - 1.0).abs() < 0.01 && (w.rain - 0.75).abs() < 0.01);
    }

    #[test]
    fn ground_wets_in_rain_and_dries_after() {
        let mut w = Weather::new(Preset::Clear);
        assert_eq!(w.ground_wet, 0.0);
        w.set(Preset::Storm);
        for _ in 0..360 {
            w.update(10.0); // час ливня
        }
        assert!(w.ground_wet > 0.8, "мокро: {}", w.ground_wet);
        w.set(Preset::Clear);
        let wet = w.ground_wet;
        // Дождь затихает ~час (переход), затем земля сохнет.
        for _ in 0..3 * 360 {
            w.update(10.0);
        }
        assert!(w.ground_wet < wet, "сохнет");
        assert!(w.ground_wet > 0.0, "за три часа не высыхает полностью");
        for p in Preset::ALL {
            assert_eq!(Preset::from_id(p.id()), Some(p));
        }
    }
}
