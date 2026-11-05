use crate::drivers::dma::Dma;
use super::trb::{Trb, TRB_CYCLE, TRB_TC, TRB_TYPE_LINK};

pub const RING_SIZE: usize = 64;

/// Кольцевой буфер передачи запросов TRB (Command Ring или Transfer Ring)
pub struct TrbRing {
    pub dma: Dma<[Trb; RING_SIZE]>,
    pub enqueue: usize,
    pub cycle: bool,
}

impl TrbRing {
    pub fn new() -> Result<Self, &'static str> {
        let mut dma: Dma<[Trb; RING_SIZE]> = Dma::zeroed()?;
        let phys = dma.physical() as u64;

        // Последний слот в кольце резервируется под Link TRB, зацикливающий кольцо
        let last = RING_SIZE - 1;
        dma[last] = Trb::new(phys, 0, (TRB_TYPE_LINK << 10) | TRB_TC | TRB_CYCLE);

        Ok(Self {
            dma,
            enqueue: 0,
            cycle: true,
        })
    }

    pub fn physical(&self) -> u64 {
        self.dma.physical() as u64
    }

    /// Помещает TRB в кольцо и переключает бит цикла при необходимости
    pub fn enqueue(&mut self, mut trb: Trb) -> u64 {
        let idx = self.enqueue;
        let trb_phys = self.physical() + (idx * 16) as u64;

        if self.cycle {
            trb.control |= TRB_CYCLE;
        } else {
            trb.control &= !TRB_CYCLE;
        }
        self.dma[idx] = trb;

        self.enqueue += 1;
        if self.enqueue == RING_SIZE - 1 {
            // Переход по Link TRB
            let mut link = self.dma[RING_SIZE - 1];
            if self.cycle {
                link.control |= TRB_CYCLE;
            } else {
                link.control &= !TRB_CYCLE;
            }
            self.dma[RING_SIZE - 1] = link;
            self.enqueue = 0;
            self.cycle = !self.cycle;
        }

        trb_phys
    }
}

/// Запись таблицы сегментов кольца событий (Event Ring Segment Table Entry)
#[repr(C, align(64))]
#[derive(Clone, Copy, Debug, Default)]
pub struct ErstEntry {
    pub ring_segment_base_address: u64,
    pub ring_segment_size: u32,
    pub reserved: u32,
}

/// Кольцо событий хост-контроллера (Event Ring)
pub struct EventRing {
    pub dma: Dma<[Trb; RING_SIZE]>,
    pub erst: Dma<[ErstEntry; 1]>,
    pub dequeue: usize,
    pub cycle: bool,
}

impl EventRing {
    pub fn new() -> Result<Self, &'static str> {
        let dma: Dma<[Trb; RING_SIZE]> = Dma::zeroed()?;
        let mut erst: Dma<[ErstEntry; 1]> = Dma::zeroed()?;

        erst[0] = ErstEntry {
            ring_segment_base_address: dma.physical() as u64,
            ring_segment_size: RING_SIZE as u32,
            reserved: 0,
        };

        Ok(Self {
            dma,
            erst,
            dequeue: 0,
            cycle: true,
        })
    }

    pub fn erst_phys(&self) -> u64 {
        self.erst.physical() as u64
    }

    pub fn current_dequeue_phys(&self) -> u64 {
        self.dma.physical() as u64 + (self.dequeue * 16) as u64
    }

    /// Проверяет появление нового события в очереди.
    /// Возвращает Some(trb) если событие готово, обновляя указатель dequeue.
    pub fn poll_event(&mut self) -> Option<Trb> {
        let trb = self.dma[self.dequeue];
        let trb_cycle = (trb.control & TRB_CYCLE) != 0;

        if trb_cycle == self.cycle {
            let ev = trb;
            self.dequeue += 1;
            if self.dequeue >= RING_SIZE {
                self.dequeue = 0;
                self.cycle = !self.cycle;
            }
            Some(ev)
        } else {
            None
        }
    }
}
