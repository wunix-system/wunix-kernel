pub mod desc;
pub mod regs;

use alloc::sync::Arc;
use spin::Mutex;
use x86_64::instructions::port::{PortReadOnly, PortWriteOnly};

use crate::drivers::manager::DeviceManager;
use crate::drivers::pci::PciDevice;
use crate::drivers::dma::Dma;
use crate::drivers::traits::{NetDevice, NetDeviceError};
use crate::serial_println;

use desc::{RtlDesc, DESC_EOR, DESC_FS, DESC_LS, DESC_OWN};
use regs::*;

const RING_SIZE: usize = 16;
const BUFFER_SIZE: usize = 2048;

/// Драйвер сетевого контроллера Realtek RTL8168/8111 PCIe Gigabit Ethernet
pub struct Rtl8168Device {
    io_base: u16,
    mac: [u8; 6],
    rx_ring: Dma<[RtlDesc; RING_SIZE]>,
    tx_ring: Dma<[RtlDesc; RING_SIZE]>,
    rx_buffers: [Dma<[u8; BUFFER_SIZE]>; RING_SIZE],
    tx_buffers: [Dma<[u8; BUFFER_SIZE]>; RING_SIZE],
    rx_cur: usize,
    tx_cur: usize,
}

unsafe impl Send for Rtl8168Device {}
unsafe impl Sync for Rtl8168Device {}

impl Rtl8168Device {
    fn in8(&self, reg: u16) -> u8 {
        let mut port = PortReadOnly::<u8>::new(self.io_base + reg);
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

        let bar0 = pci.bar(0).ok_or("rtl8168: BAR0 not found")?;
        let io_base = bar0.address as u16;

        let rx_ring = Dma::zeroed()?;
        let tx_ring = Dma::zeroed()?;

        let rx_buffers: [Dma<[u8; BUFFER_SIZE]>; RING_SIZE] = [
            Dma::zeroed()?, Dma::zeroed()?, Dma::zeroed()?, Dma::zeroed()?,
            Dma::zeroed()?, Dma::zeroed()?, Dma::zeroed()?, Dma::zeroed()?,
            Dma::zeroed()?, Dma::zeroed()?, Dma::zeroed()?, Dma::zeroed()?,
            Dma::zeroed()?, Dma::zeroed()?, Dma::zeroed()?, Dma::zeroed()?,
        ];

        let tx_buffers: [Dma<[u8; BUFFER_SIZE]>; RING_SIZE] = [
            Dma::zeroed()?, Dma::zeroed()?, Dma::zeroed()?, Dma::zeroed()?,
            Dma::zeroed()?, Dma::zeroed()?, Dma::zeroed()?, Dma::zeroed()?,
            Dma::zeroed()?, Dma::zeroed()?, Dma::zeroed()?, Dma::zeroed()?,
            Dma::zeroed()?, Dma::zeroed()?, Dma::zeroed()?, Dma::zeroed()?,
        ];

        let mut dev = Self {
            io_base,
            mac: [0; 6],
            rx_ring,
            tx_ring,
            rx_buffers,
            tx_buffers,
            rx_cur: 0,
            tx_cur: 0,
        };

        dev.init_hw()?;
        Ok(dev)
    }

    fn init_hw(&mut self) -> Result<(), &'static str> {
        // Программный сброс контроллера
        self.out8(REG_COMMAND, CMD_RST);
        for _ in 0..10_000 {
            if (self.in8(REG_COMMAND) & CMD_RST) == 0 {
                break;
            }
            core::hint::spin_loop();
        }

        // Читаем MAC-адрес
        for i in 0..6 {
            self.mac[i] = self.in8(REG_MAC0 + i as u16);
        }

        // Настраиваем дескрипторы кольца приема RX
        for i in 0..RING_SIZE {
            let mut opts = DESC_OWN | (BUFFER_SIZE as u32);
            if i == RING_SIZE - 1 {
                opts |= DESC_EOR;
            }
            self.rx_ring[i].opts1 = opts;
            self.rx_ring[i].opts2 = 0;
            self.rx_ring[i].buf_addr = self.rx_buffers[i].physical() as u64;
        }

        // Настраиваем кольцо передачи TX
        for i in 0..RING_SIZE {
            let mut opts = 0u32;
            if i == RING_SIZE - 1 {
                opts |= DESC_EOR;
            }
            self.tx_ring[i].opts1 = opts;
            self.tx_ring[i].opts2 = 0;
            self.tx_ring[i].buf_addr = self.tx_buffers[i].physical() as u64;
        }

        // Задаем адреса колец RX и TX в регистрах чипа
        let rx_phys = self.rx_ring.physical() as u64;
        self.out32(REG_RX_DESC_LO, rx_phys as u32);
        self.out32(REG_RX_DESC_HI, (rx_phys >> 32) as u32);

        let tx_phys = self.tx_ring.physical() as u64;
        self.out32(REG_TX_DESC_LO, tx_phys as u32);
        self.out32(REG_TX_DESC_HI, (tx_phys >> 32) as u32);

        // Устанавливаем максимальный размер пакета
        self.out16(REG_RMS, 1536);
        self.out8(REG_MTPS, 0x3F); // 0x3F = No early transmit (TxPacketMax)

        // Фильтр пакетов
        self.out32(REG_RCR, RCR_AAP | RCR_APM | RCR_AM | RCR_AB | RCR_MXDMA_UNLIM);

        // Включаем приемник и передатчик
        self.out8(REG_COMMAND, CMD_RE | CMD_TE);

        Ok(())
    }
}

impl NetDevice for Rtl8168Device {
    fn driver_name(&self) -> &'static str {
        "rtl8168"
    }

    fn mac_address(&self) -> [u8; 6] {
        self.mac
    }

    fn send_packet(&mut self, packet: &[u8]) -> Result<(), NetDeviceError> {
        if packet.len() > BUFFER_SIZE {
            return Err(NetDeviceError::PacketTooLarge);
        }

        let idx = self.tx_cur;
        let desc = &self.tx_ring[idx];

        // Если бит OWN всё ещё установлен, контроллер ещё не завершил отправку этого дескриптора
        if (desc.opts1 & DESC_OWN) != 0 {
            return Err(NetDeviceError::NotReady);
        }

        self.tx_buffers[idx][..packet.len()].copy_from_slice(packet);

        let mut opts = DESC_OWN | DESC_FS | DESC_LS | (packet.len() as u32);
        if idx == RING_SIZE - 1 {
            opts |= DESC_EOR;
        }

        self.tx_ring[idx].opts2 = 0;
        self.tx_ring[idx].opts1 = opts;

        core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::Release);

        // Пингуем очередь передачи (бит 6 регистра 0x38)
        self.out8(REG_TX_POLL, 0x40);

        self.tx_cur = (self.tx_cur + 1) % RING_SIZE;
        Ok(())
    }

    fn receive_packet(&mut self, buf: &mut [u8]) -> Result<Option<usize>, NetDeviceError> {
        let idx = self.rx_cur;
        let desc = self.rx_ring[idx];

        // Если OWN равен 1, пакет ещё не прибыл от контроллера
        if (desc.opts1 & DESC_OWN) != 0 {
            return Ok(None);
        }

        let len = (desc.opts1 & 0x3FFF) as usize;
        let packet_len = if len > 4 { len - 4 } else { len }; // Убираем CRC

        if buf.len() < packet_len {
            return Err(NetDeviceError::BufferTooSmall);
        }

        buf[..packet_len].copy_from_slice(&self.rx_buffers[idx][..packet_len]);

        // Возвращаем владение дескриптором контроллеру
        let mut opts = DESC_OWN | (BUFFER_SIZE as u32);
        if idx == RING_SIZE - 1 {
            opts |= DESC_EOR;
        }
        self.rx_ring[idx].opts1 = opts;

        self.rx_cur = (self.rx_cur + 1) % RING_SIZE;
        Ok(Some(packet_len))
    }
}

/// Поиск сетевых контроллеров Realtek RTL8168/8111/8169 (Vendor 0x10EC)
pub fn probe_pci(dev: &mut PciDevice, dm: &mut DeviceManager) -> bool {
    let is_rtl8168 = dev.vendor_id == 0x10EC
        && matches!(dev.device_id, 0x8168 | 0x8111 | 0x8136 | 0x8167 | 0x8169);

    if is_rtl8168 {
        match Rtl8168Device::new(dev) {
            Ok(nic) => {
                dev.driver_name = Some("rtl8168");
                let mac = nic.mac_address();
                let name = alloc::format!("eth{}", dm.net_device_count());
                serial_println!(
                    "rtl8168: registered {} (Realtek PCIe Gigabit, MAC: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x})",
                    name, mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
                );
                dm.register_net_device(Arc::new(Mutex::new(nic)));
                return true;
            }
            Err(e) => {
                serial_println!("rtl8168: init failed for {}: {}", dev.address, e);
            }
        }
    }
    false
}
