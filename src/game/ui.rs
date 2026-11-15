//! Интерфейс среза: прицел и подсказки, сообщения, субтитры, затемнение
//! чувства мицелия, карточка макро-осмотра, полевой журнал.

use super::sense::TRACE_KINDS;
use super::{Focus, Game, SPECIES};
use crate::ui::UiBuilder;

const PANEL: [u8; 4] = [10, 14, 10, 140];

/// Подпись по центру с подложкой.
fn label(b: &mut UiBuilder, cx: f32, y: f32, px: f32, c: [u8; 4], bg: [u8; 4], text: &str) {
    let tw = b.font.width(text, px);
    b.rect(cx - tw * 0.5 - 12.0, y - 4.0, tw + 24.0, px + 14.0, bg);
    b.text(cx - tw * 0.5, y, px, c, text);
}

impl Game {
    pub fn ui(&self, b: &mut UiBuilder, w: f32, h: f32) {
        let (cx, cy) = (w * 0.5, h * 0.5);
        let close = self.closeup.weight();
        // Чувство мицелия: мир темнеет, светятся только нити.
        let l = self.sense.level;
        if l > 0.0 {
            b.rect(0.0, 0.0, w, h, [4, 10, 8, (l * 90.0) as u8]);
            label(
                b,
                cx,
                96.0,
                18.0,
                [190, 240, 210, (l * 230.0) as u8],
                [0, 0, 0, 0],
                "слушаю грибницу…",
            );
        }
        if !self.closeup.active() {
            let dot = if self.focus.is_some() { 6.0 } else { 3.0 };
            b.rect(
                cx - dot * 0.5,
                cy - dot * 0.5,
                dot,
                dot,
                [255, 255, 255, 170],
            );
        }
        match self.focus {
            Some(Focus::Mushroom(i)) if !self.closeup.active() => {
                let m = &self.mushrooms[i];
                let sp = &SPECIES[m.species as usize];
                let what = if self.found[m.species as usize] > 0 {
                    sp.name
                } else {
                    "неизвестный гриб"
                };
                let text = if m.inspected {
                    format!("F — рассмотреть ещё раз: {what}")
                } else {
                    format!("F — присесть и рассмотреть: {what}")
                };
                label(b, cx, cy + 30.0, 18.0, [235, 240, 225, 255], PANEL, &text);
            }
            Some(Focus::Trace(i)) => {
                let t = &self.traces[i];
                let name = TRACE_KINDS[t.kind as usize].name;
                let text = if t.found {
                    format!("{name} — уже в журнале")
                } else {
                    format!("F — осмотреть: {name}")
                };
                label(b, cx, cy + 30.0, 18.0, [245, 225, 190, 255], PANEL, &text);
            }
            _ => {}
        }
        // Карточка макро-осмотра.
        if let Some(i) = self.closeup.target.filter(|_| close > 0.85) {
            let a = ((close - 0.85) / 0.15 * 255.0) as u8;
            let m = &self.mushrooms[i];
            let sp = &SPECIES[m.species as usize];
            let pw = 380.0f32.min(w * 0.4);
            let (px, py) = (w - pw - 28.0, h * 0.5 - 110.0);
            let lines = wrap(b, sp.note, 16.0, pw - 36.0);
            let ph = 96.0 + lines.len() as f32 * b.line(16.0);
            b.rect(px, py, pw, ph, [16, 20, 14, (a as f32 * 0.85) as u8]);
            b.text(px + 18.0, py + 14.0, 22.0, [240, 238, 215, a], sp.name);
            b.text(px + 18.0, py + 44.0, 16.0, [190, 205, 170, a], sp.latin);
            let mut y = py + 74.0;
            for line in lines {
                b.text(px + 18.0, y, 16.0, [205, 210, 195, a], &line);
                y += b.line(16.0);
            }
            label(
                b,
                cx,
                h - 70.0,
                16.0,
                [220, 225, 210, a],
                PANEL,
                "F — встать · мышь — разглядеть",
            );
        }
        if let Some((t, left)) = &self.toast {
            let a = (left.min(1.0) * 255.0) as u8;
            label(
                b,
                cx,
                45.0,
                20.0,
                [210, 245, 200, a],
                [10, 20, 12, (a as f32 * 0.6) as u8],
                t,
            );
        }
        // Субтитры: голос грибницы — бледно-голубой, мысли героя — белые.
        if let Some(s) = self.subtitles.front() {
            let a = (s.seconds.min(0.6) / 0.6 * 255.0) as u8;
            let voice = s.speaker != "Я";
            let c = if voice {
                [170, 215, 255, a]
            } else {
                [240, 240, 232, a]
            };
            let line = format!("{}: {}", s.speaker, s.text);
            let lines = wrap(b, &line, 20.0, (w - 120.0).min(900.0));
            let mut y = h - 130.0 - (lines.len() as f32 - 1.0) * b.line(20.0);
            for l in lines {
                label(b, cx, y, 20.0, c, [0, 0, 0, (a as f32 * 0.55) as u8], &l);
                y += b.line(20.0) + 6.0;
            }
        }
        let total: u32 = self.found.iter().sum();
        let known = self.found.iter().filter(|&&n| n > 0).count();
        b.text(
            20.0,
            h - 34.0,
            16.0,
            [220, 225, 210, 200],
            &format!(
                "C (удерживать) — чувство мицелия · J — журнал · 1–4 — корзина, совок, фонарь, рация · видов {known}/{} · находок {total}",
                SPECIES.len()
            ),
        );
        if self.journal_open {
            self.journal(b, w, h);
        }
    }

    fn journal(&self, b: &mut UiBuilder, w: f32, h: f32) {
        let (pw, ph) = (680.0f32.min(w - 40.0), 640.0f32.min(h - 60.0));
        let (px, py) = ((w - pw) * 0.5, (h - ph) * 0.5);
        b.rect(px, py, pw, ph, [18, 22, 16, 225]);
        b.text(
            px + 24.0,
            py + 18.0,
            28.0,
            [240, 235, 210, 255],
            "Полевой журнал — Сиратани",
        );
        let mut y = py + 66.0;
        for (s, sp) in SPECIES.iter().enumerate() {
            if self.found[s] == 0 {
                b.text(
                    px + 24.0,
                    y,
                    17.0,
                    [140, 140, 130, 255],
                    "? ? ?  — вид не найден",
                );
                y += 28.0;
                continue;
            }
            b.text(
                px + 24.0,
                y,
                19.0,
                [230, 240, 215, 255],
                &format!("{} — {} (×{})", sp.name, sp.latin, self.found[s]),
            );
            y += b.line(19.0);
            for line in wrap(b, sp.note, 15.0, pw - 48.0) {
                b.text(px + 32.0, y, 15.0, [190, 200, 180, 255], &line);
                y += b.line(15.0);
            }
            y += 8.0;
        }
        if !self.notes.is_empty() {
            y += 6.0;
            b.text(px + 24.0, y, 21.0, [240, 225, 190, 255], "Наблюдения");
            y += b.line(21.0) + 4.0;
            for n in &self.notes {
                b.text(px + 24.0, y, 18.0, [235, 225, 200, 255], &n.title);
                y += b.line(18.0);
                for line in wrap(b, &n.text, 15.0, pw - 48.0) {
                    b.text(px + 32.0, y, 15.0, [200, 195, 180, 255], &line);
                    y += b.line(15.0);
                }
                y += 8.0;
            }
        }
    }
}

fn wrap(b: &UiBuilder, text: &str, px: f32, width: f32) -> Vec<String> {
    let mut out = Vec::new();
    let mut line = String::new();
    for word in text.split(' ') {
        let cand = if line.is_empty() {
            word.to_string()
        } else {
            format!("{line} {word}")
        };
        if b.font.width(&cand, px) > width && !line.is_empty() {
            out.push(std::mem::replace(&mut line, word.to_string()));
        } else {
            line = cand;
        }
    }
    if !line.is_empty() {
        out.push(line);
    }
    out
}
