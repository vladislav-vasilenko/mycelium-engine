//! GPU-таймеры проходов (02.10, сессия 4): метка времени командного буфера
//! между проходами (`writeTimestampIntoHeap`) — проходы не трогаем. Метки слота читаются, когда слот освободился; разности —
//! время прохода, сглаженное по кадрам.

use objc2::runtime::ProtocolObject;
use objc2_foundation::NSRange;
use objc2_metal::*;

use super::frame::FRAME_SLOTS;
use super::shaders::Obj;

/// Меток в куче слота (проходов + 1).
const STAMPS: usize = 64;

pub(super) struct GpuTimers {
    heaps: Vec<Obj<dyn MTL4CounterHeap>>,
    /// Имена проходов кадра, записанного в слот (метка i — начало прохода i).
    names: Vec<Vec<&'static str>>,
    /// Сглаженное время по проходам, мс (в порядке последнего плана).
    pub avg: Vec<(&'static str, f64)>,
    ticks_per_ms: f64,
}

impl GpuTimers {
    pub fn new(device: &ProtocolObject<dyn MTLDevice>) -> Option<GpuTimers> {
        let d = MTL4CounterHeapDescriptor::new();
        unsafe {
            d.setType(MTL4CounterHeapType::Timestamp);
            d.setCount(STAMPS);
        }
        let heaps = (0..FRAME_SLOTS)
            .map(|_| device.newCounterHeapWithDescriptor_error(&d).ok())
            .collect::<Option<Vec<_>>>()?;
        let freq = device.queryTimestampFrequency();
        if freq == 0 {
            return None;
        }
        Some(GpuTimers {
            heaps,
            names: vec![Vec::new(); FRAME_SLOTS],
            avg: Vec::new(),
            ticks_per_ms: freq as f64 / 1000.0,
        })
    }

    /// Метка `index` в слоте (между кодировщиками).
    pub fn stamp(&self, cmd: &ProtocolObject<dyn MTL4CommandBuffer>, slot: usize, index: usize) {
        if index >= STAMPS {
            return;
        }
        unsafe { cmd.writeTimestampIntoHeap_atIndex(&self.heaps[slot], index) };
    }

    pub fn set_names(&mut self, slot: usize, names: Vec<&'static str>) {
        self.names[slot] = names;
    }

    /// Метки кадра, завершённого в слоте, → среднее по проходам.
    pub fn collect(&mut self, slot: usize) {
        let n = self.names[slot].len();
        if n == 0 || n + 1 > STAMPS {
            return;
        }
        let Some(data) = (unsafe { self.heaps[slot].resolveCounterRange(NSRange::new(0, n + 1)) })
        else {
            return;
        };
        let bytes = data.to_vec();
        let ts: Vec<u64> = bytes
            .as_chunks::<8>()
            .0
            .iter()
            .map(|c| u64::from_le_bytes(*c))
            .collect();
        if ts.len() < n + 1 || ts.contains(&0) {
            return;
        }
        let names = self.names[slot].clone();
        for (i, name) in names.iter().enumerate() {
            let ms = ts[i + 1].saturating_sub(ts[i]) as f64 / self.ticks_per_ms;
            match self.avg.iter_mut().find(|(k, _)| k == name) {
                Some((_, v)) => *v += (ms - *v) * 0.05,
                None => self.avg.push((name, ms)),
            }
        }
    }
}
