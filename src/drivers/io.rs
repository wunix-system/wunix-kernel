use core::marker::PhantomData;
use core::ptr::{read_volatile, write_volatile};
use wunix_drv::BusIo;
use x86_64::instructions::port::{PortReadOnly, PortWriteOnly};

/// Базовый типаж аппаратного ввода-вывода ядра Wunix.
pub trait Io {
    type Value: Copy;

    fn read(&self) -> Self::Value;
    fn write(&mut self, value: Self::Value);

    #[inline]
    fn readf(&self, flags: Self::Value) -> bool
    where
        Self::Value: core::ops::BitAnd<Output = Self::Value> + PartialEq,
    {
        (self.read() & flags) == flags
    }

    #[inline]
    fn writef(&mut self, flags: Self::Value, value: bool)
    where
        Self::Value: core::ops::BitAnd<Output = Self::Value>
            + core::ops::BitOr<Output = Self::Value>
            + core::ops::Not<Output = Self::Value>,
    {
        let current = self.read();
        let new_val = if value {
            current | flags
        } else {
            current & !flags
        };
        self.write(new_val);
    }
}

/// Портовый ввод-вывод (Port I/O) для архитектуры x86_64.
pub struct Pio<T> {
    port: u16,
    _phantom: PhantomData<T>,
}

impl<T> Pio<T> {
    #[inline]
    pub const fn new(port: u16) -> Self {
        Self {
            port,
            _phantom: PhantomData,
        }
    }

    #[inline]
    pub const fn port(&self) -> u16 {
        self.port
    }
}

impl Io for Pio<u8> {
    type Value = u8;

    #[inline]
    fn read(&self) -> u8 {
        let mut port = PortReadOnly::<u8>::new(self.port);
        unsafe { port.read() }
    }

    #[inline]
    fn write(&mut self, value: u8) {
        let mut port = PortWriteOnly::<u8>::new(self.port);
        unsafe { port.write(value) }
    }
}

impl Io for Pio<u16> {
    type Value = u16;

    #[inline]
    fn read(&self) -> u16 {
        let mut port = PortReadOnly::<u16>::new(self.port);
        unsafe { port.read() }
    }

    #[inline]
    fn write(&mut self, value: u16) {
        let mut port = PortWriteOnly::<u16>::new(self.port);
        unsafe { port.write(value) }
    }
}

impl Io for Pio<u32> {
    type Value = u32;

    #[inline]
    fn read(&self) -> u32 {
        let mut port = PortReadOnly::<u32>::new(self.port);
        unsafe { port.read() }
    }

    #[inline]
    fn write(&mut self, value: u32) {
        let mut port = PortWriteOnly::<u32>::new(self.port);
        unsafe { port.write(value) }
    }
}

/// Отображаемый в память ввод-вывод (MMIO) поверх одного регистра.
#[repr(transparent)]
pub struct Mmio<T> {
    value: T,
}

impl<T: Copy> Mmio<T> {
    #[inline]
    pub fn read(&self) -> T {
        unsafe { read_volatile(&self.value as *const T) }
    }

    #[inline]
    pub fn write(&mut self, value: T) {
        unsafe { write_volatile(&mut self.value as *mut T, value) }
    }
}

impl<T: Copy> Io for Mmio<T> {
    type Value = T;

    #[inline]
    fn read(&self) -> T {
        self.read()
    }

    #[inline]
    fn write(&mut self, value: T) {
        self.write(value)
    }
}

/// Окно MMIO с проверкой границ, реализующее `wunix_drv::BusIo`.
/// Позволяет драйверам работать по единому контракту с реальным железом и с `MockBusIo`.
pub struct MmioRegion {
    base: *mut u8,
    size: usize,
}

unsafe impl Send for MmioRegion {}
unsafe impl Sync for MmioRegion {}

impl MmioRegion {
    /// # Safety
    /// `base` должен указывать на корректно замапленный MMIO-регион длиной `size` байт.
    pub const unsafe fn from_raw(base: *mut u8, size: usize) -> Self {
        Self { base, size }
    }

    pub fn base_ptr(&self) -> *mut u8 {
        self.base
    }

    pub fn size(&self) -> usize {
        self.size
    }
}

impl BusIo for MmioRegion {
    #[inline]
    fn read_u8(&self, offset: usize) -> u8 {
        if offset >= self.size {
            return 0xFF;
        }
        unsafe { read_volatile(self.base.add(offset)) }
    }

    #[inline]
    fn read_u16(&self, offset: usize) -> u16 {
        if offset + 2 > self.size {
            return 0xFFFF;
        }
        unsafe { read_volatile(self.base.add(offset) as *const u16) }
    }

    #[inline]
    fn read_u32(&self, offset: usize) -> u32 {
        if offset + 4 > self.size {
            return 0xFFFF_FFFF;
        }
        unsafe { read_volatile(self.base.add(offset) as *const u32) }
    }

    #[inline]
    fn write_u8(&mut self, offset: usize, val: u8) {
        if offset < self.size {
            unsafe { write_volatile(self.base.add(offset), val) }
        }
    }

    #[inline]
    fn write_u16(&mut self, offset: usize, val: u16) {
        if offset + 2 <= self.size {
            unsafe { write_volatile(self.base.add(offset) as *mut u16, val) }
        }
    }

    #[inline]
    fn write_u32(&mut self, offset: usize, val: u32) {
        if offset + 4 <= self.size {
            unsafe { write_volatile(self.base.add(offset) as *mut u32, val) }
        }
    }
}

/// Диапазон портов ввода-вывода x86, реализующий `wunix_drv::BusIo`.
pub struct PioRegion {
    base_port: u16,
    size: u16,
}

impl PioRegion {
    pub const fn new(base_port: u16, size: u16) -> Self {
        Self { base_port, size }
    }
}

impl BusIo for PioRegion {
    #[inline]
    fn read_u8(&self, offset: usize) -> u8 {
        if offset >= self.size as usize {
            return 0xFF;
        }
        Pio::<u8>::new(self.base_port + offset as u16).read()
    }

    #[inline]
    fn read_u16(&self, offset: usize) -> u16 {
        if offset + 2 > self.size as usize {
            return 0xFFFF;
        }
        Pio::<u16>::new(self.base_port + offset as u16).read()
    }

    #[inline]
    fn read_u32(&self, offset: usize) -> u32 {
        if offset + 4 > self.size as usize {
            return 0xFFFF_FFFF;
        }
        Pio::<u32>::new(self.base_port + offset as u16).read()
    }

    #[inline]
    fn write_u8(&mut self, offset: usize, val: u8) {
        if offset < self.size as usize {
            Pio::<u8>::new(self.base_port + offset as u16).write(val);
        }
    }

    #[inline]
    fn write_u16(&mut self, offset: usize, val: u16) {
        if offset + 2 <= self.size as usize {
            Pio::<u16>::new(self.base_port + offset as u16).write(val);
        }
    }

    #[inline]
    fn write_u32(&mut self, offset: usize, val: u32) {
        if offset + 4 <= self.size as usize {
            Pio::<u32>::new(self.base_port + offset as u16).write(val);
        }
    }
}
