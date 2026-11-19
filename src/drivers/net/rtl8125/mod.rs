pub mod desc;
pub mod regs;

use alloc::sync::Arc;
use spin::Mutex;

use crate::drivers::dma::Dma;
use crate::drivers::manager::DeviceManager;
use crate::drivers::pci::PciDevice;
use crate::drivers::traits::{NetDevice, NetDeviceError};
use crate::memory::map_kernel_mmio;
use crate::serial_println;

use desc::{Rtl8125Desc, DESC_EOR, DESC_FS, DESC_LS, DESC_OWN};
use regs::*;

const RING_SIZE: usize = 16;
const BUFFER_SIZE: usize = 2048;

/// Драйвер современного сетевого контроллера Realtek RTL8125 (2.5 Gigabit Ethernet)
pub struct Rtl8125Device {
    mmio_base: *mut u8,
    mac: [u8; 6],
    rx_ring: Dma<[Rtl8125Desc; RING_SIZE]>,
    tx_ring: Dma<[Rtl8125Desc; RING_SIZE]>,
    rx_buffers: [Dma<[u8; BUFFER_SIZE]>; RING_SIZE],
    tx_buffers: [Dma<[u8; BUFFER_SIZE]>; RING_SIZE],
    rx_cur: usize,
    tx_cur: usize,
    rx_bytes_count: usize,
    rx_packets_count: usize,
    tx_bytes_count: usize,
    tx_packets_count: usize,
}

unsafe impl Send for Rtl8125Device {}
unsafe impl Sync for Rtl8125Device {}

impl Rtl8125Device {
    fn read8(&self, reg: usize) -> u8 {
        unsafe { core::ptr::read_volatile(self.mmio_base.add(reg)) }
    }

    fn write8(&self, reg: usize, val: u8) {
        unsafe { core::ptr::write_volatile(self.mmio_base.add(reg), val) }
    }

    fn write16(&self, reg: usize, val: u16) {
        unsafe { core::ptr::write_volatile(self.mmio_base.add(reg) as *mut u16, val) }
    }

    fn write32(&self, reg: usize, val: u32) {
        unsafe { core::ptr::write_volatile(self.mmio_base.add(reg) as *mut u32, val) }
    }

    pub fn new(pci: &PciDevice) -> Result<Self, &'static str> {
        pci.enable_memory_space();
        pci.enable_bus_mastering();

        // RTL8125 использует MMIO через BAR2 (или BAR0)
        let bar = pci.bar(2).or_else(|| pci.bar(0)).ok_or("rtl8125: MMIO BAR not found")?;
        let mmio_virt = map_kernel_mmio(bar.address, bar.size as usize)
            .map_err(|_| "rtl8125: failed to map MMIO")?;
        let mmio_base = mmio_virt.as_mut_ptr::<u8>();

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
            mmio_base,
            mac: [0; 6],
            rx_ring,
            tx_ring,
            rx_buffers,
            tx_buffers,
            rx_cur: 0,
            tx_cur: 0,
            rx_bytes_count: 0,
            rx_packets_count: 0,
            tx_bytes_count: 0,
            tx_packets_count: 0,
        };

        dev.init_hw()?;
        Ok(dev)
    }

    fn init_hw(&mut self) -> Result<(), &'static str> {
        // Программный сброс чипа
        self.write8(REG_COMMAND, CMD_RST);
        for _ in 0..10_000 {
            if (self.read8(REG_COMMAND) & CMD_RST) == 0 {
                break;
            }
            core::hint::spin_loop();
        }

        // Читаем аппаратный MAC-адрес
        for i in 0..6 {
            self.mac[i] = self.read8(REG_MAC0 + i);
        }

        // Настраиваем дескрипторы RX
        for i in 0..RING_SIZE {
            let mut opts = DESC_OWN | (BUFFER_SIZE as u32);
            if i == RING_SIZE - 1 {
                opts |= DESC_EOR;
            }
            self.rx_ring[i].opts1 = opts;
            self.rx_ring[i].opts2 = 0;
            self.rx_ring[i].buf_addr = self.rx_buffers[i].physical() as u64;
        }

        // Настраиваем дескрипторы TX
        for i in 0..RING_SIZE {
            let mut opts = 0u32;
            if i == RING_SIZE - 1 {
                opts |= DESC_EOR;
            }
            self.tx_ring[i].opts1 = opts;
            self.tx_ring[i].opts2 = 0;
            self.tx_ring[i].buf_addr = self.tx_buffers[i].physical() as u64;
        }

        // Передаем физические адреса колец в регистры чипа
        let rx_phys = self.rx_ring.physical() as u64;
        self.write32(REG_RDSAR_LO, rx_phys as u32);
        self.write32(REG_RDSAR_HI, (rx_phys >> 32) as u32);

        let tx_phys = self.tx_ring.physical() as u64;
        self.write32(REG_TNPDS_LO, tx_phys as u32);
        self.write32(REG_TNPDS_HI, (tx_phys >> 32) as u32);

        // Максимальный размер пакета 16383 байт
        self.write16(REG_RMS, 16383);
        self.write8(REG_MTPS, (BUFFER_SIZE / 128) as u8);

        // Настройка фильтрации пакетов и DMA
        self.write32(REG_RCR, RCR_AAP | RCR_APM | RCR_AM | RCR_AB | RCR_MXDMA_UNLIMITED);
        self.write32(REG_TCR, TCR_MXDMA_UNLIMITED);

        // Включаем приёмник и передатчик
        self.write8(REG_COMMAND, CMD_RE | CMD_TE);

        Ok(())
    }
}

impl NetDevice for Rtl8125Device {
    fn driver_name(&self) -> &'static str {
        "rtl8125"
    }

    fn mac_address(&self) -> [u8; 6] {
        self.mac
    }

    fn send_packet(&mut self, packet: &[u8]) -> Result<(), NetDeviceError> {
        if packet.len() > BUFFER_SIZE {
            return Err(NetDeviceError::PacketTooLarge);
        }

        let idx = self.tx_cur;
        if (self.tx_ring[idx].opts1 & DESC_OWN) != 0 {
            return Err(NetDeviceError::NotReady);
        }

        self.tx_buffers[idx][..packet.len()].copy_from_slice(packet);

        let mut opts = DESC_OWN | DESC_FS | DESC_LS | (packet.len() as u32 & 0x1FFF);
        if idx == RING_SIZE - 1 {
            opts |= DESC_EOR;
        }
        self.tx_ring[idx].opts2 = 0;
        self.tx_ring[idx].opts1 = opts;

        core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::Release);

        // Пингуем контроллер на отправку
        self.write8(REG_TPPOLL, 0x40);

        self.tx_cur = (self.tx_cur + 1) % RING_SIZE;
        self.tx_packets_count += 1;
        self.tx_bytes_count += packet.len();

        Ok(())
    }

    fn receive_packet(&mut self, buf: &mut [u8]) -> Result<Option<usize>, NetDeviceError> {
        let idx = self.rx_cur;
        let opts = self.rx_ring[idx].opts1;

        if (opts & DESC_OWN) != 0 {
            return Ok(None);
        }

        let pkt_len = (opts & 0x3FFF).saturating_sub(4) as usize; // Минус CRC
        if pkt_len > buf.len() {
            return Err(NetDeviceError::BufferTooSmall);
        }

        if pkt_len > 0 {
            buf[..pkt_len].copy_from_slice(&self.rx_buffers[idx][..pkt_len]);
        }

        let mut next_opts = DESC_OWN | (BUFFER_SIZE as u32);
        if idx == RING_SIZE - 1 {
            next_opts |= DESC_EOR;
        }
        self.rx_ring[idx].opts1 = next_opts;

        self.rx_cur = (self.rx_cur + 1) % RING_SIZE;
        self.rx_packets_count += 1;
        self.rx_bytes_count += pkt_len;

        Ok(Some(pkt_len))
    }

    fn rx_bytes(&self) -> usize { self.rx_bytes_count }
    fn rx_packets(&self) -> usize { self.rx_packets_count }
    fn tx_bytes(&self) -> usize { self.tx_bytes_count }
    fn tx_packets(&self) -> usize { self.tx_packets_count }
}

pub fn probe_pci(dev: &mut PciDevice, dm: &mut DeviceManager) -> bool {
    // Realtek RTL8125 / RTL8125B (0x10EC:0x8125, 0x8126, 0x3000)
    if dev.vendor_id == 0x10EC && (dev.device_id == 0x8125 || dev.device_id == 0x8126 || dev.device_id == 0x3000) {
        match Rtl8125Device::new(dev) {
            Ok(nic) => {
                dev.driver_name = Some("rtl8125");
                let mac = nic.mac_address();
                let name = alloc::format!("eth{}", dm.net_device_count());
                serial_println!(
                    "rtl8125: registered {} (Realtek RTL8125 2.5GbE, MAC: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x})",
                    name, mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
                );
                dm.register_net_device(Arc::new(Mutex::new(nic)));
                return true;
            }
            Err(e) => {
                serial_println!("rtl8125: init failed for {}: {}", dev.address, e);
            }
        }
    }
    false
}
