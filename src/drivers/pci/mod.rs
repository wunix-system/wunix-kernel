pub mod config;
pub mod scan;

use core::fmt;

/// Адрес устройства на шине PCI: Bus:Device.Function (BDF)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct PciAddress {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
}

impl PciAddress {
    pub const fn new(bus: u8, device: u8, function: u8) -> Self {
        Self {
            bus,
            device: device & 0x1F,
            function: function & 0x07,
        }
    }
}

impl fmt::Display for PciAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:02x}:{:02x}.{}",
            self.bus, self.device, self.function
        )
    }
}

/// Тип базового адресного регистра (BAR)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarType {
    /// Отображение в память (MMIO 32-бит)
    Memory32,
    /// Отображение в память (MMIO 64-бит)
    Memory64,
    /// Пространство портов ввода/вывода (I/O Port)
    Io,
}

/// Разобранный базовый адресный регистр (Base Address Register)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PciBar {
    pub index: u8,
    pub bar_type: BarType,
    pub address: u64,
    pub size: u64,
    pub prefetchable: bool,
}

/// Классы устройств PCI
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PciClass;

impl PciClass {
    pub const STORAGE: u8 = 0x01;
    pub const NETWORK: u8 = 0x02;
    pub const DISPLAY: u8 = 0x03;
    pub const MULTIMEDIA: u8 = 0x04;
    pub const MEMORY: u8 = 0x05;
    pub const BRIDGE: u8 = 0x06;
    pub const SIMPLE_COMM: u8 = 0x07;
    pub const BASE_PERIPH: u8 = 0x08;
    pub const INPUT: u8 = 0x09;
    pub const PROCESSOR: u8 = 0x0B;
    pub const SERIAL_BUS: u8 = 0x0C;
    pub const WIRELESS: u8 = 0x0D;

    pub const SUBCLASS_PCI_BRIDGE: u8 = 0x04;

    /// Возвращает текстовое описание класса и подкласса
    pub fn name(class: u8, subclass: u8) -> &'static str {
        match (class, subclass) {
            (Self::STORAGE, 0x00) => "SCSI Controller",
            (Self::STORAGE, 0x01) => "IDE Controller",
            (Self::STORAGE, 0x06) => "SATA Controller",
            (Self::STORAGE, 0x08) => "Non-Volatile Memory (NVMe)",
            (Self::STORAGE, _) => "Mass Storage Controller",

            (Self::NETWORK, 0x00) => "Ethernet Controller",
            (Self::NETWORK, _) => "Network Controller",

            (Self::DISPLAY, 0x00) => "VGA Compatible Controller",
            (Self::DISPLAY, _) => "Display Controller",

            (Self::BRIDGE, 0x00) => "Host Bridge",
            (Self::BRIDGE, 0x01) => "ISA Bridge",
            (Self::BRIDGE, Self::SUBCLASS_PCI_BRIDGE) => "PCI-to-PCI Bridge",
            (Self::BRIDGE, _) => "Bridge Device",

            (Self::INPUT, 0x00) => "Keyboard Controller",
            (Self::INPUT, 0x02) => "Mouse Controller",
            (Self::INPUT, _) => "Input Device Controller",

            (Self::SERIAL_BUS, 0x03) => "USB Controller",
            (Self::SERIAL_BUS, _) => "Serial Bus Controller",

            _ => "Generic / Unknown Device",
        }
    }
}

/// Описание найденного устройства на шине PCI
#[derive(Debug, Clone)]
pub struct PciDevice {
    pub address: PciAddress,
    pub vendor_id: u16,
    pub device_id: u16,
    pub command: u16,
    pub status: u16,
    pub revision_id: u8,
    pub prog_if: u8,
    pub subclass: u8,
    pub class_code: u8,
    pub header_type: u8,
    pub subsystem_vendor_id: u16,
    pub subsystem_id: u16,
    pub interrupt_line: u8,
    pub interrupt_pin: u8,
    pub bars: [Option<PciBar>; 6],
    pub driver_name: Option<&'static str>,
}

impl PciDevice {
    #[inline]
    pub fn read_config_u32(&self, offset: u8) -> u32 {
        config::read_u32(self.address, offset)
    }

    #[inline]
    pub fn write_config_u32(&self, offset: u8, val: u32) {
        config::write_u32(self.address, offset, val);
    }

    #[inline]
    pub fn read_config_u16(&self, offset: u8) -> u16 {
        config::read_u16(self.address, offset)
    }

    #[inline]
    pub fn write_config_u16(&self, offset: u8, val: u16) {
        config::write_u16(self.address, offset, val);
    }

    #[inline]
    pub fn read_config_u8(&self, offset: u8) -> u8 {
        config::read_u8(self.address, offset)
    }

    #[inline]
    pub fn write_config_u8(&self, offset: u8, val: u8) {
        config::write_u8(self.address, offset, val);
    }

    /// Включает Bus Mastering (бит 2 регистра Command), позволяя устройству
    /// выполнять прямой доступ к памяти (DMA/VirtIO кольца).
    pub fn enable_bus_mastering(&self) {
        let cmd = self.read_config_u16(0x04);
        self.write_config_u16(0x04, cmd | (1 << 2));
    }

    /// Включает адресацию в пространстве портов ввода-вывода (бит 0 регистра Command).
    pub fn enable_io_space(&self) {
        let cmd = self.read_config_u16(0x04);
        self.write_config_u16(0x04, cmd | (1 << 0));
    }

    /// Включает отображение памяти MMIO (бит 1 регистра Command).
    pub fn enable_memory_space(&self) {
        let cmd = self.read_config_u16(0x04);
        self.write_config_u16(0x04, cmd | (1 << 1));
    }

    /// Возвращает BAR по индексу (0..5)
    pub fn bar(&self, index: usize) -> Option<&PciBar> {
        if index < 6 {
            self.bars[index].as_ref()
        } else {
            None
        }
    }

    /// Ищет первый доступный I/O BAR (используется для Legacy VirtIO)
    pub fn first_io_bar(&self) -> Option<&PciBar> {
        self.bars
            .iter()
            .filter_map(|b| b.as_ref())
            .find(|b| b.bar_type == BarType::Io)
    }

    /// Ищет первый доступный Memory BAR (используется для Modern VirtIO / MMIO)
    pub fn first_memory_bar(&self) -> Option<&PciBar> {
        self.bars
            .iter()
            .filter_map(|b| b.as_ref())
            .find(|b| matches!(b.bar_type, BarType::Memory32 | BarType::Memory64))
    }

    /// Ищет указанную возможность (PCI Capability) по идентификатору `cap_id`.
    ///
    /// Проверяет бит 4 регистра Status. Если поддерживаются capabilities,
    /// проходит по связанному списку, начиная с регистра 0x34.
    pub fn find_capability(&self, cap_id: u8) -> Option<u8> {
        let status = self.read_config_u16(0x06);
        if (status & (1 << 4)) == 0 {
            return None;
        }

        let mut cap_offset = self.read_config_u8(0x34) & !0x3;
        while cap_offset != 0 {
            let id = self.read_config_u8(cap_offset);
            if id == cap_id {
                return Some(cap_offset);
            }
            cap_offset = self.read_config_u8(cap_offset + 1) & !0x3;
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_pci_address_masking() {
        let addr = PciAddress::new(5, 40, 10);
        // device & 0x1F: 40 & 31 = 8
        // function & 0x07: 10 & 7 = 2
        assert_eq!(addr.bus, 5);
        assert_eq!(addr.device, 8);
        assert_eq!(addr.function, 2);
    }

    #[test_case]
    fn test_pci_class_names() {
        assert_eq!(
            PciClass::name(PciClass::NETWORK, 0x00),
            "Ethernet Controller"
        );
        assert_eq!(
            PciClass::name(PciClass::STORAGE, 0x08),
            "Non-Volatile Memory (NVMe)"
        );
        assert_eq!(
            PciClass::name(PciClass::BRIDGE, PciClass::SUBCLASS_PCI_BRIDGE),
            "PCI-to-PCI Bridge"
        );
    }
}
