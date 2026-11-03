use alloc::boxed::Box;
use x86_64::instructions::port::Port;

use super::queue::Virtqueue;
use super::VirtioError;
use crate::drivers::pci::PciDevice;

/// Базовый типаж транспорта для обмена данными с устройством VirtIO
pub trait VirtioTransport: Send + Sync {
    /// Считывает битовую маску поддерживаемых устройством возможностей (Device Features)
    fn read_device_features(&mut self) -> u64;

    /// Передает устройству битовую маску возможностей, согласованных драйвером (Driver Features)
    fn write_driver_features(&mut self, features: u64);

    /// Считывает текущий байт статуса устройства (Device Status)
    fn get_status(&mut self) -> u8;

    /// Устанавливает байт статуса устройства
    fn set_status(&mut self, status: u8);

    /// Сбрасывает устройство в начальное состояние (запись 0 в статус)
    fn reset(&mut self) {
        self.set_status(0);
    }

    /// Выбирает активную очередь по ее индексу
    fn select_queue(&mut self, queue: u16);

    /// Возвращает максимальный размер текущей выбранной очереди (0, если очередь не поддерживается)
    fn get_queue_size(&mut self) -> u16;

    /// Регистрирует кольцевую очередь в устройстве (передает физические адреса дескрипторов)
    fn setup_queue(&mut self, queue: &Virtqueue);

    /// Отправляет уведомление устройству о поступлении новых буферов в очередь
    fn notify_queue(&mut self, queue: u16);

    /// Считывает байты из специфичного для типа устройства конфигурационного пространства
    fn read_device_config(&mut self, offset: usize, buf: &mut [u8]);

    /// Записывает байты в специфичное для типа устройства конфигурационное пространство
    fn write_device_config(&mut self, offset: usize, data: &[u8]);

    /// Читает регистр статуса прерывания ISR (чтение также сбрасывает флаг прерывания)
    fn read_isr(&mut self) -> u8;
}

/// Транспорт VirtIO через порты ввода-вывода классической шины PCI (Legacy PCI I/O ports)
pub struct LegacyPciTransport {
    io_base: u16,
}

impl LegacyPciTransport {
    pub const fn new(io_base: u16) -> Self {
        Self { io_base }
    }
}

// Смещения регистров в классическом I/O пространстве Legacy VirtIO PCI:
const LEGACY_DEVICE_FEATURES: u16 = 0x00;
const LEGACY_GUEST_FEATURES: u16 = 0x04;
const LEGACY_QUEUE_ADDRESS: u16 = 0x08;
const LEGACY_QUEUE_SIZE: u16 = 0x0C;
const LEGACY_QUEUE_SELECT: u16 = 0x0E;
const LEGACY_QUEUE_NOTIFY: u16 = 0x10;
const LEGACY_DEVICE_STATUS: u16 = 0x12;
const LEGACY_ISR_STATUS: u16 = 0x13;
const LEGACY_DEVICE_CONFIG: u16 = 0x14;

impl VirtioTransport for LegacyPciTransport {
    fn read_device_features(&mut self) -> u64 {
        unsafe {
            let mut port = Port::<u32>::new(self.io_base + LEGACY_DEVICE_FEATURES);
            port.read() as u64
        }
    }

    fn write_driver_features(&mut self, features: u64) {
        unsafe {
            let mut port = Port::<u32>::new(self.io_base + LEGACY_GUEST_FEATURES);
            port.write(features as u32);
        }
    }

    fn get_status(&mut self) -> u8 {
        unsafe {
            let mut port = Port::<u8>::new(self.io_base + LEGACY_DEVICE_STATUS);
            port.read()
        }
    }

    fn set_status(&mut self, status: u8) {
        unsafe {
            let mut port = Port::<u8>::new(self.io_base + LEGACY_DEVICE_STATUS);
            port.write(status);
        }
    }

    fn select_queue(&mut self, queue: u16) {
        unsafe {
            let mut port = Port::<u16>::new(self.io_base + LEGACY_QUEUE_SELECT);
            port.write(queue);
        }
    }

    fn get_queue_size(&mut self) -> u16 {
        unsafe {
            let mut port = Port::<u16>::new(self.io_base + LEGACY_QUEUE_SIZE);
            port.read()
        }
    }

    fn setup_queue(&mut self, queue: &Virtqueue) {
        unsafe {
            let mut sel_port = Port::<u16>::new(self.io_base + LEGACY_QUEUE_SELECT);
            sel_port.write(queue.queue_index());

            let pfn = (queue.phys_addr() / 4096) as u32;
            let mut addr_port = Port::<u32>::new(self.io_base + LEGACY_QUEUE_ADDRESS);
            addr_port.write(pfn);
        }
    }

    fn notify_queue(&mut self, queue: u16) {
        unsafe {
            let mut port = Port::<u16>::new(self.io_base + LEGACY_QUEUE_NOTIFY);
            port.write(queue);
        }
    }

    fn read_device_config(&mut self, offset: usize, buf: &mut [u8]) {
        for (i, byte) in buf.iter_mut().enumerate() {
            unsafe {
                let mut port = Port::<u8>::new(self.io_base + LEGACY_DEVICE_CONFIG + (offset + i) as u16);
                *byte = port.read();
            }
        }
    }

    fn write_device_config(&mut self, offset: usize, data: &[u8]) {
        for (i, &byte) in data.iter().enumerate() {
            unsafe {
                let mut port = Port::<u8>::new(self.io_base + LEGACY_DEVICE_CONFIG + (offset + i) as u16);
                port.write(byte);
            }
        }
    }

    fn read_isr(&mut self) -> u8 {
        unsafe {
            let mut port = Port::<u8>::new(self.io_base + LEGACY_ISR_STATUS);
            port.read()
        }
    }
}

/// Транспорт VirtIO 1.0+ через Capabilities шины PCI (Modern PCI Transport)
pub struct ModernPciTransport {
    common_cfg: *mut u8,
    notify_base: *mut u8,
    notify_off_multiplier: u32,
    isr: *mut u8,
    device_cfg: *mut u8,
}

unsafe impl Send for ModernPciTransport {}
unsafe impl Sync for ModernPciTransport {}

impl VirtioTransport for ModernPciTransport {
    fn read_device_features(&mut self) -> u64 {
        unsafe {
            core::ptr::write_volatile(self.common_cfg.add(0) as *mut u32, 0);
            let low = core::ptr::read_volatile(self.common_cfg.add(4) as *const u32) as u64;
            core::ptr::write_volatile(self.common_cfg.add(0) as *mut u32, 1);
            let high = core::ptr::read_volatile(self.common_cfg.add(4) as *const u32) as u64;
            (high << 32) | low
        }
    }

    fn write_driver_features(&mut self, features: u64) {
        unsafe {
            core::ptr::write_volatile(self.common_cfg.add(8) as *mut u32, 0);
            core::ptr::write_volatile(self.common_cfg.add(12) as *mut u32, features as u32);
            core::ptr::write_volatile(self.common_cfg.add(8) as *mut u32, 1);
            core::ptr::write_volatile(self.common_cfg.add(12) as *mut u32, (features >> 32) as u32);
        }
    }

    fn get_status(&mut self) -> u8 {
        unsafe { core::ptr::read_volatile(self.common_cfg.add(0x14)) }
    }

    fn set_status(&mut self, status: u8) {
        unsafe { core::ptr::write_volatile(self.common_cfg.add(0x14), status) }
    }

    fn select_queue(&mut self, queue: u16) {
        unsafe { core::ptr::write_volatile(self.common_cfg.add(0x16) as *mut u16, queue) }
    }

    fn get_queue_size(&mut self) -> u16 {
        unsafe { core::ptr::read_volatile(self.common_cfg.add(0x18) as *const u16) }
    }

    fn setup_queue(&mut self, queue: &Virtqueue) {
        unsafe {
            core::ptr::write_volatile(self.common_cfg.add(0x16) as *mut u16, queue.queue_index());
            core::ptr::write_volatile(self.common_cfg.add(0x18) as *mut u16, queue.queue_size());
            core::ptr::write_volatile(self.common_cfg.add(0x20) as *mut u64, queue.desc_phys());
            core::ptr::write_volatile(self.common_cfg.add(0x28) as *mut u64, queue.avail_phys());
            core::ptr::write_volatile(self.common_cfg.add(0x30) as *mut u64, queue.used_phys());
            core::ptr::write_volatile(self.common_cfg.add(0x1C) as *mut u16, 1);
        }
    }

    fn notify_queue(&mut self, queue: u16) {
        unsafe {
            core::ptr::write_volatile(self.common_cfg.add(0x16) as *mut u16, queue);
            let notify_off = core::ptr::read_volatile(self.common_cfg.add(0x1E) as *const u16) as u32;
            let doorbell = self.notify_base.add((notify_off * self.notify_off_multiplier) as usize) as *mut u16;
            core::ptr::write_volatile(doorbell, queue);
        }
    }

    fn read_device_config(&mut self, offset: usize, buf: &mut [u8]) {
        for (i, byte) in buf.iter_mut().enumerate() {
            unsafe {
                *byte = core::ptr::read_volatile(self.device_cfg.add(offset + i));
            }
        }
    }

    fn write_device_config(&mut self, offset: usize, data: &[u8]) {
        for (i, &byte) in data.iter().enumerate() {
            unsafe {
                core::ptr::write_volatile(self.device_cfg.add(offset + i), byte);
            }
        }
    }

    fn read_isr(&mut self) -> u8 {
        unsafe { core::ptr::read_volatile(self.isr) }
    }
}

/// Транспорт VirtIO через прямое отображение регистров в память (MMIO)
pub struct MmioTransport {
    base_virt: *mut u8,
}

unsafe impl Send for MmioTransport {}
unsafe impl Sync for MmioTransport {}

impl MmioTransport {
    pub const fn new(base_virt: *mut u8) -> Self {
        Self { base_virt }
    }

    #[inline]
    unsafe fn read_reg32(&self, offset: usize) -> u32 {
        unsafe { core::ptr::read_volatile(self.base_virt.add(offset) as *const u32) }
    }

    #[inline]
    unsafe fn write_reg32(&mut self, offset: usize, val: u32) {
        unsafe { core::ptr::write_volatile(self.base_virt.add(offset) as *mut u32, val) };
    }
}

// Смещения регистров стандарта VirtIO MMIO
pub const MMIO_MAGIC: usize = 0x00;
pub const MMIO_VERSION: usize = 0x04;
pub const MMIO_DEVICE_ID: usize = 0x08;
const MMIO_DEVICE_FEATURES: usize = 0x10;
const MMIO_DEVICE_FEATURES_SEL: usize = 0x14;
const MMIO_DRIVER_FEATURES: usize = 0x20;
const MMIO_DRIVER_FEATURES_SEL: usize = 0x24;
const MMIO_QUEUE_SEL: usize = 0x30;
const MMIO_QUEUE_NUM_MAX: usize = 0x34;
const MMIO_QUEUE_NUM: usize = 0x38;
const MMIO_QUEUE_READY: usize = 0x44;
const MMIO_QUEUE_NOTIFY: usize = 0x50;
const MMIO_INTERRUPT_STATUS: usize = 0x60;
const MMIO_INTERRUPT_ACK: usize = 0x64;
const MMIO_STATUS: usize = 0x70;
const MMIO_QUEUE_DESC_LOW: usize = 0x80;
const MMIO_QUEUE_DESC_HIGH: usize = 0x84;
const MMIO_QUEUE_DRIVER_LOW: usize = 0x90;
const MMIO_QUEUE_DRIVER_HIGH: usize = 0x94;
const MMIO_QUEUE_DEVICE_LOW: usize = 0xA0;
const MMIO_QUEUE_DEVICE_HIGH: usize = 0xA4;
const MMIO_CONFIG: usize = 0x100;

impl VirtioTransport for MmioTransport {
    fn read_device_features(&mut self) -> u64 {
        unsafe {
            self.write_reg32(MMIO_DEVICE_FEATURES_SEL, 0);
            let low = self.read_reg32(MMIO_DEVICE_FEATURES) as u64;
            self.write_reg32(MMIO_DEVICE_FEATURES_SEL, 1);
            let high = self.read_reg32(MMIO_DEVICE_FEATURES) as u64;
            (high << 32) | low
        }
    }

    fn write_driver_features(&mut self, features: u64) {
        unsafe {
            self.write_reg32(MMIO_DRIVER_FEATURES_SEL, 0);
            self.write_reg32(MMIO_DRIVER_FEATURES, features as u32);
            self.write_reg32(MMIO_DRIVER_FEATURES_SEL, 1);
            self.write_reg32(MMIO_DRIVER_FEATURES, (features >> 32) as u32);
        }
    }

    fn get_status(&mut self) -> u8 {
        unsafe { (self.read_reg32(MMIO_STATUS) & 0xFF) as u8 }
    }

    fn set_status(&mut self, status: u8) {
        unsafe {
            self.write_reg32(MMIO_STATUS, status as u32);
        }
    }

    fn select_queue(&mut self, queue: u16) {
        unsafe {
            self.write_reg32(MMIO_QUEUE_SEL, queue as u32);
        }
    }

    fn get_queue_size(&mut self) -> u16 {
        unsafe { (self.read_reg32(MMIO_QUEUE_NUM_MAX) & 0xFFFF) as u16 }
    }

    fn setup_queue(&mut self, queue: &Virtqueue) {
        unsafe {
            self.write_reg32(MMIO_QUEUE_SEL, queue.queue_index() as u32);
            self.write_reg32(MMIO_QUEUE_NUM, queue.queue_size() as u32);

            let desc = queue.desc_phys();
            let avail = queue.avail_phys();
            let used = queue.used_phys();

            self.write_reg32(MMIO_QUEUE_DESC_LOW, desc as u32);
            self.write_reg32(MMIO_QUEUE_DESC_HIGH, (desc >> 32) as u32);

            self.write_reg32(MMIO_QUEUE_DRIVER_LOW, avail as u32);
            self.write_reg32(MMIO_QUEUE_DRIVER_HIGH, (avail >> 32) as u32);

            self.write_reg32(MMIO_QUEUE_DEVICE_LOW, used as u32);
            self.write_reg32(MMIO_QUEUE_DEVICE_HIGH, (used >> 32) as u32);

            self.write_reg32(MMIO_QUEUE_READY, 1);
        }
    }

    fn notify_queue(&mut self, queue: u16) {
        unsafe {
            self.write_reg32(MMIO_QUEUE_NOTIFY, queue as u32);
        }
    }

    fn read_device_config(&mut self, offset: usize, buf: &mut [u8]) {
        for (i, byte) in buf.iter_mut().enumerate() {
            unsafe {
                *byte = core::ptr::read_volatile(self.base_virt.add(MMIO_CONFIG + offset + i));
            }
        }
    }

    fn write_device_config(&mut self, offset: usize, data: &[u8]) {
        for (i, &byte) in data.iter().enumerate() {
            unsafe {
                core::ptr::write_volatile(self.base_virt.add(MMIO_CONFIG + offset + i), byte);
            }
        }
    }

    fn read_isr(&mut self) -> u8 {
        unsafe {
            let status = (self.read_reg32(MMIO_INTERRUPT_STATUS) & 0xFF) as u8;
            self.write_reg32(MMIO_INTERRUPT_ACK, status as u32);
            status
        }
    }
}

// Типы структур capabilities в стандарте VirtIO 1.0 (PCI_CAP_ID_VNDR: 0x09)
const VIRTIO_PCI_CAP_COMMON_CFG: u8 = 1;
const VIRTIO_PCI_CAP_NOTIFY_CFG: u8 = 2;
const VIRTIO_PCI_CAP_ISR_CFG: u8 = 3;
const VIRTIO_PCI_CAP_DEVICE_CFG: u8 = 4;

#[derive(Debug, Clone, Copy, Default)]
struct PciCapInfo {
    bar: u8,
    offset: u32,
    _length: u32,
    extra: u32,
}

/// Ищет все специфичные для VirtIO 1.0 capabilities в конфигурационном пространстве PCI
fn find_modern_caps(dev: &PciDevice) -> (Option<PciCapInfo>, Option<PciCapInfo>, Option<PciCapInfo>, Option<PciCapInfo>) {
    let mut common = None;
    let mut notify = None;
    let mut isr = None;
    let mut device = None;

    let status = dev.read_config_u16(0x06);
    if (status & (1 << 4)) == 0 {
        return (None, None, None, None);
    }

    let mut cap_offset = dev.read_config_u8(0x34) & !0x3;
    while cap_offset != 0 {
        let cap_id = dev.read_config_u8(cap_offset);
        let cap_next = dev.read_config_u8(cap_offset + 1) & !0x3;

        if cap_id == 0x09 {
            let cfg_type = dev.read_config_u8(cap_offset + 3);
            let bar = dev.read_config_u8(cap_offset + 4);
            let offset = dev.read_config_u32(cap_offset + 8);
            let length = dev.read_config_u32(cap_offset + 12);
            let extra = if cfg_type == VIRTIO_PCI_CAP_NOTIFY_CFG {
                dev.read_config_u32(cap_offset + 16)
            } else {
                0
            };

            let info = PciCapInfo {
                bar,
                offset,
                _length: length,
                extra,
            };

            match cfg_type {
                VIRTIO_PCI_CAP_COMMON_CFG => common = Some(info),
                VIRTIO_PCI_CAP_NOTIFY_CFG => notify = Some(info),
                VIRTIO_PCI_CAP_ISR_CFG => isr = Some(info),
                VIRTIO_PCI_CAP_DEVICE_CFG => device = Some(info),
                _ => {}
            }
        }

        cap_offset = cap_next;
    }

    (common, notify, isr, device)
}

/// Получает виртуальный указатель на Capability внутри соответствующего BAR
fn map_cap_pointer(dev: &PciDevice, cap: PciCapInfo) -> Result<*mut u8, VirtioError> {
    let bar = dev.bar(cap.bar as usize).ok_or(VirtioError::InvalidBar)?;
    let virt_addr = crate::memory::map_kernel_mmio(bar.address, bar.size as usize)
        .map_err(|_| VirtioError::InvalidBar)?;
    let ptr = (virt_addr.as_u64() + cap.offset as u64) as *mut u8;
    Ok(ptr)
}

/// Создает транспорт VirtIO для заданного PCI устройства.
///
/// Автоматически активирует Bus Mastering на шине PCI и выбирает
/// подходящий интерфейс:
/// 1. Классический Legacy I/O port (если устройство Transitional и содержит I/O BAR).
/// 2. Современный VirtIO 1.0 Modern PCI Transport (через PCI Capabilities и MMIO).
/// 3. Прямой MMIO транспорт при отсутствии capabilities.
pub fn create_transport_from_pci(dev: &PciDevice) -> Result<Box<dyn VirtioTransport>, VirtioError> {
    dev.enable_bus_mastering();

    // 1. Проверяем наличие Legacy I/O BAR (обычно BAR0 в Transitional VirtIO)
    if let Some(io_bar) = dev.first_io_bar() {
        dev.enable_io_space();
        return Ok(Box::new(LegacyPciTransport::new(io_bar.address as u16)));
    }

    // 2. Проверяем наличие Modern VirtIO Capabilities (VirtIO 1.0+)
    let (common_cap, notify_cap, isr_cap, device_cap) = find_modern_caps(dev);
    if let (Some(common), Some(notify), Some(isr), Some(device)) = (common_cap, notify_cap, isr_cap, device_cap) {
        dev.enable_memory_space();

        let common_cfg = map_cap_pointer(dev, common)?;
        let notify_base = map_cap_pointer(dev, notify)?;
        let isr_ptr = map_cap_pointer(dev, isr)?;
        let device_cfg = map_cap_pointer(dev, device)?;

        return Ok(Box::new(ModernPciTransport {
            common_cfg,
            notify_base,
            notify_off_multiplier: notify.extra,
            isr: isr_ptr,
            device_cfg,
        }));
    }

    // 3. Прямой Memory BAR fallback
    if let Some(mem_bar) = dev.first_memory_bar() {
        dev.enable_memory_space();
        let virt_addr = crate::memory::map_kernel_mmio(mem_bar.address, mem_bar.size as usize)
            .map_err(|_| VirtioError::InvalidBar)?;
        return Ok(Box::new(MmioTransport::new(virt_addr.as_mut_ptr())));
    }

    Err(VirtioError::InvalidBar)
}
