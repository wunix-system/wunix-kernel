// Размер кольцевой очереди символов. 128 байт достаточно, чтобы пользователь
// мог быстро набрать целую команду терминала даже при временной задержке обработки.
pub const KEY_QUEUE_CAPACITY: usize = 128;

pub struct RingBuffer<T, const CAP: usize> {
    buffer: [T; CAP],
    head: usize,
    tail: usize,
    len: usize,
}

impl<T: Copy, const CAP: usize> RingBuffer<T, CAP> {
    pub const fn new(default_val: T) -> Self {
        Self {
            buffer: [default_val; CAP],
            head: 0,
            tail: 0,
            len: 0,
        }
    }

    pub fn push(&mut self, item: T) -> bool {
        if self.len >= CAP {
            return false;
        }
        self.buffer[self.tail] = item;
        self.tail = (self.tail + 1) % CAP;
        self.len += 1;
        true
    }

    pub fn pop(&mut self) -> Option<T> {
        if self.len == 0 {
            return None;
        }
        let item = self.buffer[self.head];
        self.head = (self.head + 1) % CAP;
        self.len -= 1;
        Some(item)
    }

    pub fn clear(&mut self) {
        self.head = 0;
        self.tail = 0;
        self.len = 0;
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn is_full(&self) -> bool {
        self.len >= CAP
    }

    pub fn len(&self) -> usize {
        self.len
    }
}
