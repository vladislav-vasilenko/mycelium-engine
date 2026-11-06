//! Список проходов кадра и вычисление барьеров.
//!
//! Каждый проход объявляет чтения и записи с указанием стадии. Планировщик
//! идёт по проходам в порядке добавления и учитывает зависимости на весь ресурс:
//! RAW (чтение после записи), WAR (запись после чтения) и WAW (запись после записи).
//! Кадры идут друг за другом в одной очереди, поэтому учитываются и доступы
//! предыдущего кадра: первая запись кадра N+1 ждёт чтений кадра N. Если состав
//! проходов меняется между кадрами (например, readback только по запросу),
//! план строится по фактическому списку предыдущего кадра (`plan_after`).
//! Бэкенд превращает результат в барьеры своего API.

use std::collections::HashMap;
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ResourceId(pub u32);

/// Стадии GPU в терминах проекта. Metal: `MTLStages`, D3D12: sync/access.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Stages(pub u32);

impl Stages {
    pub const NONE: Stages = Stages(0);
    pub const VERTEX: Stages = Stages(1 << 0);
    pub const FRAGMENT: Stages = Stages(1 << 1);
    pub const DISPATCH: Stages = Stages(1 << 2);
    pub const BLIT: Stages = Stages(1 << 3);

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
    pub fn contains(self, other: Stages) -> bool {
        self.0 & other.0 == other.0
    }
}

impl std::ops::BitOr for Stages {
    type Output = Stages;
    fn bitor(self, rhs: Stages) -> Stages {
        Stages(self.0 | rhs.0)
    }
}
impl std::ops::BitOrAssign for Stages {
    fn bitor_assign(&mut self, rhs: Stages) {
        self.0 |= rhs.0;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PassKind {
    Compute,
    Render,
}

impl PassKind {
    fn allowed(self) -> Stages {
        match self {
            PassKind::Compute => Stages::DISPATCH | Stages::BLIT,
            PassKind::Render => Stages::VERTEX | Stages::FRAGMENT,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hazard {
    Raw,
    War,
    Waw,
}

#[derive(Clone, Debug)]
pub struct PassDecl {
    pub name: &'static str,
    pub kind: PassKind,
    pub reads: Vec<(ResourceId, Stages)>,
    pub writes: Vec<(ResourceId, Stages)>,
}

/// Барьер перед проходом: дождаться `after` предыдущей работы до начала `before`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Barrier {
    pub after: Stages,
    pub before: Stages,
    pub hazards: Vec<(ResourceId, Hazard)>,
}

#[derive(Clone, Debug)]
pub struct FramePlan {
    pub passes: Vec<PassDecl>,
    /// Барьер для каждого прохода, тот же индекс.
    pub barriers: Vec<Barrier>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum GraphError {
    ReadBeforeWrite {
        pass: &'static str,
        resource: ResourceId,
    },
    StageNotAllowed {
        pass: &'static str,
        stages: Stages,
    },
}

impl fmt::Display for GraphError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GraphError::ReadBeforeWrite { pass, resource } => write!(
                f,
                "проход `{pass}` читает ресурс {resource:?} до записи в кадре; пометь его импортированным"
            ),
            GraphError::StageNotAllowed { pass, stages } => {
                write!(
                    f,
                    "проход `{pass}` использует недопустимые стадии {stages:?}"
                )
            }
        }
    }
}

#[derive(Clone, Copy, Default)]
struct Track {
    last_write: Stages,
    reads_since_write: Stages,
}

/// Строит план для кадра, который повторяет тот же список проходов.
/// `imported` — ресурсы с содержимым из-за пределов кадра.
#[cfg(test)]
pub fn plan(passes: Vec<PassDecl>, imported: &[ResourceId]) -> Result<FramePlan, GraphError> {
    let previous = passes.clone();
    plan_after(&previous, passes, imported)
}

/// Строит план кадра, перед которым в той же очереди выполнен кадр
/// со списком `previous`. Барьеры учитывают доступы обоих кадров.
pub fn plan_after(
    previous: &[PassDecl],
    passes: Vec<PassDecl>,
    imported: &[ResourceId],
) -> Result<FramePlan, GraphError> {
    // Валидация: стадии и чтение до записи.
    let mut written: Vec<ResourceId> = imported.to_vec();
    for p in &passes {
        let allowed = p.kind.allowed();
        for (_, s) in p.reads.iter().chain(p.writes.iter()) {
            if !allowed.contains(*s) {
                return Err(GraphError::StageNotAllowed {
                    pass: p.name,
                    stages: *s,
                });
            }
        }
        for (r, _) in &p.reads {
            if !written.contains(r) {
                return Err(GraphError::ReadBeforeWrite {
                    pass: p.name,
                    resource: *r,
                });
            }
        }
        for (r, _) in &p.writes {
            written.push(*r);
        }
    }

    // Первый круг — проходы предыдущего кадра, только состояние ресурсов.
    // Второй — проходы текущего кадра, барьеры.
    let mut tracks: HashMap<ResourceId, Track> = HashMap::new();
    let mut barriers = vec![Barrier::default(); passes.len()];
    for round in 0..2 {
        let list: &[PassDecl] = if round == 0 { previous } else { &passes };
        for (i, p) in list.iter().enumerate() {
            let mut b = Barrier::default();
            for (r, stage) in &p.reads {
                let t = tracks.entry(*r).or_default();
                if !t.last_write.is_empty() {
                    b.after |= t.last_write;
                    b.before |= *stage;
                    b.hazards.push((*r, Hazard::Raw));
                }
            }
            for (r, stage) in &p.writes {
                let t = tracks.entry(*r).or_default();
                if !t.reads_since_write.is_empty() {
                    b.after |= t.reads_since_write;
                    b.before |= *stage;
                    b.hazards.push((*r, Hazard::War));
                } else if !t.last_write.is_empty() {
                    b.after |= t.last_write;
                    b.before |= *stage;
                    b.hazards.push((*r, Hazard::Waw));
                }
            }
            // Применяем доступы прохода после вычисления его барьера.
            for (r, stage) in &p.reads {
                tracks.entry(*r).or_default().reads_since_write |= *stage;
            }
            for (r, stage) in &p.writes {
                let t = tracks.entry(*r).or_default();
                t.last_write = *stage;
                t.reads_since_write = Stages::NONE;
            }
            if round == 1 {
                barriers[i] = b;
            }
        }
    }
    Ok(FramePlan { passes, barriers })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATTERN: ResourceId = ResourceId(1);
    const HDR: ResourceId = ResourceId(2);
    const BACKBUFFER: ResourceId = ResourceId(3);
    const ARGS: ResourceId = ResourceId(4);

    fn frame() -> Vec<PassDecl> {
        vec![
            PassDecl {
                name: "pattern",
                kind: PassKind::Compute,
                reads: vec![],
                writes: vec![(PATTERN, Stages::DISPATCH)],
            },
            PassDecl {
                name: "scene",
                kind: PassKind::Render,
                reads: vec![(PATTERN, Stages::FRAGMENT)],
                writes: vec![(HDR, Stages::FRAGMENT)],
            },
            PassDecl {
                name: "tonemap",
                kind: PassKind::Render,
                reads: vec![(HDR, Stages::FRAGMENT)],
                writes: vec![(BACKBUFFER, Stages::FRAGMENT)],
            },
        ]
    }

    #[test]
    fn compute_write_then_fragment_read_is_raw() {
        let plan = plan(frame(), &[]).unwrap();
        let b = &plan.barriers[1];
        assert!(b.hazards.contains(&(PATTERN, Hazard::Raw)));
        assert!(b.after.contains(Stages::DISPATCH));
        assert!(b.before.contains(Stages::FRAGMENT));
    }

    #[test]
    fn next_frame_write_waits_for_previous_reads() {
        let plan = plan(frame(), &[]).unwrap();
        // Кадр N+1 пишет pattern, пока кадр N мог его читать во fragment.
        let b = &plan.barriers[0];
        assert_eq!(b.hazards, vec![(PATTERN, Hazard::War)]);
        assert_eq!(b.after, Stages::FRAGMENT);
        assert_eq!(b.before, Stages::DISPATCH);
    }

    /// Кадр с readback (копия pattern и depth в стадии blit), затем обычный кадр.
    /// Запись pattern и depth следующего кадра должна ждать blit предыдущего.
    #[test]
    fn plan_change_waits_for_previous_frame_stages() {
        const DEPTH: ResourceId = ResourceId(5);
        let mut with_readback = frame();
        with_readback[1].writes.push((DEPTH, Stages::FRAGMENT));
        with_readback.insert(
            2,
            PassDecl {
                name: "readback",
                kind: PassKind::Compute,
                reads: vec![(PATTERN, Stages::BLIT), (DEPTH, Stages::BLIT)],
                writes: vec![(ARGS, Stages::BLIT)],
            },
        );
        let mut normal = frame();
        normal[1].writes.push((DEPTH, Stages::FRAGMENT));

        let after_readback = plan_after(&with_readback, normal.clone(), &[]).unwrap();
        assert!(
            after_readback.barriers[0].after.contains(Stages::BLIT),
            "pattern ждёт blit"
        );
        assert!(
            after_readback.barriers[1].after.contains(Stages::BLIT),
            "scene (depth) ждёт blit"
        );

        // Повтор обычного кадра blit не ждёт.
        let steady = plan_after(&normal, normal.clone(), &[]).unwrap();
        assert!(!steady.barriers[0].after.contains(Stages::BLIT));
        assert!(!steady.barriers[1].after.contains(Stages::BLIT));
    }

    #[test]
    fn write_after_write_without_reads_is_waw() {
        let passes = vec![
            PassDecl {
                name: "a",
                kind: PassKind::Compute,
                reads: vec![],
                writes: vec![(ARGS, Stages::DISPATCH)],
            },
            PassDecl {
                name: "b",
                kind: PassKind::Compute,
                reads: vec![],
                writes: vec![(ARGS, Stages::BLIT)],
            },
        ];
        let plan = plan(passes, &[]).unwrap();
        assert_eq!(plan.barriers[1].hazards, vec![(ARGS, Hazard::Waw)]);
        assert_eq!(plan.barriers[1].after, Stages::DISPATCH);
    }

    #[test]
    fn read_before_write_is_rejected_unless_imported() {
        let passes = vec![PassDecl {
            name: "consumer",
            kind: PassKind::Render,
            reads: vec![(PATTERN, Stages::FRAGMENT)],
            writes: vec![],
        }];
        assert_eq!(
            plan(passes.clone(), &[]).unwrap_err(),
            GraphError::ReadBeforeWrite {
                pass: "consumer",
                resource: PATTERN
            }
        );
        assert!(plan(passes, &[PATTERN]).is_ok());
    }

    #[test]
    fn wrong_stage_for_pass_kind_is_rejected() {
        let passes = vec![PassDecl {
            name: "bad",
            kind: PassKind::Compute,
            reads: vec![],
            writes: vec![(HDR, Stages::FRAGMENT)],
        }];
        assert!(matches!(
            plan(passes, &[]),
            Err(GraphError::StageNotAllowed { .. })
        ));
    }
}
