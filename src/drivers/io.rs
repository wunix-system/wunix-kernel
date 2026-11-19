use core::marker::PhantomData;
use core::ptr::{read_volatile, write_volatile};
use x86_64::instructions::port::{PortReadOnly, PortWriteOnly};

/// Базовый типаж аппаратного ввода-вывода ядра Wunix.
/// Унифицирует доступ к регистрам железа независимо от того,
/// сидят ли они на портах x86 (Port I/O) или отображены в физическую память (MMIO).
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

/// Отображаемый в память ввод-вывод (MMIO).
/// Гарантирует volatile-доступ, чтобы компилятор не оптимизировал
/// обращения к регистрам контроллеров оборудования.
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
