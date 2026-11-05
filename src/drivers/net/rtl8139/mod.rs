pub mod regs;

use alloc::sync::Arc;
use spin::Mutex;
use x86_64::instructions::port::{PortReadOnly, PortWriteOnly};

use crate::drivers::manager::DeviceManager;
use crate::drivers::pci::PciDevice;
use crate::drivers::redox_compat::Dma;
use crate::drivers::traits::{NetDevice, NetDeviceError};
use crate::serial_println;

use regs::*;

const RX_BUFFER_SIZE: usize = 8192 + 16 + 2048; // 8 КиБ кольцо + 16 байт CRC + 2 КиБ wrap
const TX_BUFFER_SIZE: usize = 2048;

/// Драйвер сетевого контроллера Realtek RTL8139 (Fast Ethernet 10/100)
pub struct Rtl8139Device {
    io_base: u16,
    mac: [u8; 6],
    rx_buffer: Dma<[u8; RX_BUFFER_SIZE]>,
    tx_buffers: [Dma<[u8; TX_BUFFER_SIZE]>; 4],
    tx_cur: usize,
    rx_offset: usize,
}

unsafe impl Send for Rtl8139Device {}
unsafe impl Sync for Rtl8139Device {}

impl Rtl8139Device {
    fn in8(&self, reg: u16) -> u8 {
        let mut port = PortReadOnly::<u8>::new(self.io_base + reg);
        unsafe { port.read() }
    }

    fn in16(&self, reg: u16) -> u16 {
        let mut port = PortReadOnly::<u16>::new(self.io_base + reg);
        unsafe { port.read() }
    }

    fn in32(&self, reg: u16) -> u32 {
        let mut port = PortReadOnly::<u32>::new(self.io_base + reg);
        unsafe { port.read() }
    }

    fn out8(&self, reg: u16, val: u8) {
        let mut port = PortWriteOnly::<u8>::new(self.io_base + reg);
        unsafe { port.write(val) }
    }

    fn out16(&self, reg: u16, val: u16) {
        let mut port = PortWriteOnly::<u16>::new(self.io_base + reg);
        unsafe { port.write(val) }
    }

    fn out32(&self, reg: u16, val: u32) {
        let mut port = PortWriteOnly::<u32>::new(self.io_base + reg);
        unsafe { port.write(val) }
    }

    pub fn new(pci: &PciDevice) -> Result<Self, &'static str> {
        pci.enable_io_space();
        pci.enable_bus_mastering();

        let bar0 = pci.bar(0).ok_or("rtl8139: BAR0 not found")?;
        let io_base = bar0.address as u16;

        let tx0 = Dma::zeroed()?;
        let tx1 = Dma::zeroed()?;
        let tx2 = Dma::zeroed()?;
        let tx3 = Dma::zeroed()?;

        let mut dev = Self {
            io_base,
            mac: [0; 6],
            rx_buffer: Dma::zeroed()?,
            tx_buffers: [tx0, tx1, tx2, tx3],
            tx_cur: 0,
            rx_offset: 0,
        };

        dev.init_hw()?;
        Ok(dev)
    }

    fn init_hw(&mut self) -> Result<(), &'static str> {
        // Включаем чип (выводим из режима энергосбережения ACPI)
        self.out8(REG_CONFIG1, 0x00);

        // Программный сброс контроллера (RST)
        self.out8(REG_CR, CR_RST);
        for _ in 0..10_000 {
            if (self.in8(REG_CR) & CR_RST) == 0 {
                break;
            }
            core::hint::spin_loop();
        }

        // Читаем аппаратный MAC-адрес чипа из IDR-портов
        for i in 0..6 {
            self.mac[i] = self.in8(REG_IDR0 + i as u16);
        }

        // Задаем адрес DMA кольцевого буфера приема
        self.out32(REG_RBSTART, self.rx_buffer.physical() as u32);

        // Настраиваем адреса буферов передачи
        self.out32(REG_TSAD0, self.tx_buffers[0].physical() as u32);
        self.out32(REG_TSAD1, self.tx_buffers[1].physical() as u32);
        self.out32(REG_TSAD2, self.tx_buffers[2].physical() as u32);
        self.out32(REG_TSAD3, self.tx_buffers[3].physical() as u32);

        // Настройка фильтра приема: Broadcast + Multicast + Physical Match
        self.out32(REG_RCR, RCR_AAP | RCR_APM | RCR_AM | RCR_AB | RCR_WRAP);

        // Включаем приемник и передатчик
        self.out8(REG_CR, CR_RE | CR_TE);

        // Сбрасываем указатель чтения CAPR
        self.out16(REG_CAPR, 0);

        Ok(())
    }
}

impl NetDevice for Rtl8139Device {
    fn driver_name(&self) -> &'static str {
        "rtl8139"
    }

    fn mac_address(&self) -> [u8; 6] {
        self.mac
    }

    fn send_packet(&mut self, packet: &[u8]) -> Result<(), NetDeviceError> {
        if packet.len() > TX_BUFFER_SIZE {
            return Err(NetDeviceError::PacketTooLarge);
        }

        let desc_idx = self.tx_cur;
        let tsd_reg = REG_TSD0 + (desc_idx as u16 * 4);

        // Проверяем бит OWN в дескрипторе: если 0, контроллер ещё занят отправкой
        if (self.in32(tsd_reg) & TSD_OWN) == 0 && self.in32(tsd_reg) != 0 {
            return Err(NetDeviceError::NotReady);
        }

        // Копируем пакет в соответствующий DMA-буфер передачи
        self.tx_buffers[desc_idx][..packet.len()].copy_from_slice(packet);

        // Запись размера пакета в TSD запускает передачу пакета по DMA
        self.out32(tsd_reg, packet.len() as u32 & 0x1FFF);

        self.tx_cur = (self.tx_cur + 1) % 4;
        Ok(())
    }

    fn receive_packet(&mut self, buf: &mut [u8]) -> Result<Option<usize>, NetDeviceError> {
        let cr = self.in8(REG_CR);
        if (cr & CR_BUFE) != 0 {
            return Ok(None);
        }

        let offset = self.rx_offset;
        let cbr = self.in16(REG_CBR) as usize;

        if offset == cbr {
            return Ok(None);
        }

        // В буфере RTL8139 каждый пакет начинается с 4-байтного заголовка:
        // [статус: u16, размер_с_CRC: u16, полезная_нагрузка...]
        let status = u16::from_le_bytes([self.rx_buffer[offset], self.rx_buffer[offset + 1]]);
        let len_with_crc = u16::from_le_bytes([self.rx_buffer[offset + 2], self.rx_buffer[offset + 3]]) as usize;

        // Бит 0 в статусе: ROK (Receive OK)
        if (status & 1) == 0 || len_with_crc < 4 || len_with_crc > 1536 {
            // Ошибка пакета, пропускаем до следующего CBR
            self.rx_offset = cbr;
            self.out16(REG_CAPR, ((cbr as u16).wrapping_sub(16)) & 0xFFFF);
            return Ok(None);
        }

        let packet_len = len_with_crc - 4; // Вычитаем 4 байта CRC
        if buf.len() < packet_len {
            return Err(NetDeviceError::BufferTooSmall);
        }

        let payload_start = offset + 4;
        buf[..packet_len].copy_from_slice(&self.rx_buffer[payload_start..payload_start + packet_len]);

        // Обновляем смещение кольца с учетом 4-байтного заголовка и 4-байтного выравнивания (DWORD)
        let next_offset = (offset + len_with_crc + 4 + 3) & !3;
        self.rx_offset = next_offset % 8192;

        // По спецификации Realtek в CAPR записывается смещение за вычетом 16 байт
        let capr_val = ((self.rx_offset as u16).wrapping_sub(16)) & 0xFFFF;
        self.out16(REG_CAPR, capr_val);

        Ok(Some(packet_len))
    }
}

/// Поиск и регистрация сетевых контроллеров Realtek RTL8139 (Vendor 0x10EC, Device 0x8139)
pub fn probe_pci(dev: &PciDevice, dm: &mut DeviceManager) -> bool {
    if dev.vendor_id == 0x10EC && dev.device_id == 0x8139 {
        match Rtl8139Device::new(dev) {
            Ok(nic) => {
                let mac = nic.mac_address();
                let name = alloc::format!("eth{}", dm.net_device_count());
                serial_println!(
                    "rtl8139: registered {} (Realtek RTL8139 10/100, MAC: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x})",
                    name, mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
                );
                dm.register_net_device(Arc::new(Mutex::new(nic)));
                return true;
            }
            Err(e) => {
                serial_println!("rtl8139: init failed for {}: {}", dev.address, e);
            }
        }
    }
    false
}
