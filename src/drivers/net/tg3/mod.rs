pub mod desc;
pub mod regs;

use alloc::sync::Arc;
use spin::Mutex;
use crate::drivers::dma::Dma;
use crate::drivers::manager::DeviceManager;
use crate::drivers::pci::PciDevice;
use crate::drivers::traits::{NetDevice, NetDeviceError};

use desc::*;
use regs::*;

const RING_SIZE: usize = 16;
const BUFFER_SIZE: usize = 2048;

pub struct Tg3Device {
    base: u64,
    mac: [u8; 6],
    rx_ring: Dma<[Tg3RxDesc; RING_SIZE]>,
    tx_ring: Dma<[Tg3TxDesc; RING_SIZE]>,
    rx_buffers: [Dma<[u8; BUFFER_SIZE]>; RING_SIZE],
    tx_buffers: [Dma<[u8; BUFFER_SIZE]>; RING_SIZE],
    rx_cur: usize,
    tx_cur: usize,
    rx_bytes: usize,
    rx_packets: usize,
    tx_bytes: usize,
    tx_packets: usize,
}

unsafe impl Send for Tg3Device {}
unsafe impl Sync for Tg3Device {}

fn alloc_buffers() -> Result<[Dma<[u8; BUFFER_SIZE]>; RING_SIZE], &'static str> {
    Ok([
        Dma::zeroed().map_err(|_| "tg3: dma alloc failed")?,
        Dma::zeroed().map_err(|_| "tg3: dma alloc failed")?,
        Dma::zeroed().map_err(|_| "tg3: dma alloc failed")?,
        Dma::zeroed().map_err(|_| "tg3: dma alloc failed")?,
        Dma::zeroed().map_err(|_| "tg3: dma alloc failed")?,
        Dma::zeroed().map_err(|_| "tg3: dma alloc failed")?,
        Dma::zeroed().map_err(|_| "tg3: dma alloc failed")?,
        Dma::zeroed().map_err(|_| "tg3: dma alloc failed")?,
        Dma::zeroed().map_err(|_| "tg3: dma alloc failed")?,
        Dma::zeroed().map_err(|_| "tg3: dma alloc failed")?,
        Dma::zeroed().map_err(|_| "tg3: dma alloc failed")?,
        Dma::zeroed().map_err(|_| "tg3: dma alloc failed")?,
        Dma::zeroed().map_err(|_| "tg3: dma alloc failed")?,
        Dma::zeroed().map_err(|_| "tg3: dma alloc failed")?,
        Dma::zeroed().map_err(|_| "tg3: dma alloc failed")?,
        Dma::zeroed().map_err(|_| "tg3: dma alloc failed")?,
    ])
}

impl Tg3Device {
    #[inline]
    fn read_reg(&self, offset: usize) -> u32 {
        unsafe { core::ptr::read_volatile((self.base + offset as u64) as *const u32) }
    }

    #[inline]
    fn write_reg(&self, offset: usize, val: u32) {
        unsafe { core::ptr::write_volatile((self.base + offset as u64) as *mut u32, val) }
    }

    pub fn new(pci: &PciDevice, mmio_base_virt: u64) -> Result<Self, &'static str> {
        let mut dev = Self {
            base: mmio_base_virt,
            mac: [0; 6],
            rx_ring: Dma::zeroed().map_err(|_| "tg3: failed allocating RX ring")?,
            tx_ring: Dma::zeroed().map_err(|_| "tg3: failed allocating TX ring")?,
            rx_buffers: alloc_buffers()?,
            tx_buffers: alloc_buffers()?,
            rx_cur: 0,
            tx_cur: 0,
            rx_bytes: 0,
            rx_packets: 0,
            tx_bytes: 0,
            tx_packets: 0,
        };

        dev.init_hw(pci)?;
        Ok(dev)
    }

    fn init_hw(&mut self, _pci: &PciDevice) -> Result<(), &'static str> {
        let mac_hi = self.read_reg(TG3_MAC_ADDR_0_HIGH);
        let mac_lo = self.read_reg(TG3_MAC_ADDR_0_LOW);

        self.mac[0] = ((mac_hi >> 8) & 0xFF) as u8;
        self.mac[1] = (mac_hi & 0xFF) as u8;
        self.mac[2] = ((mac_lo >> 24) & 0xFF) as u8;
        self.mac[3] = ((mac_lo >> 16) & 0xFF) as u8;
        self.mac[4] = ((mac_lo >> 8) & 0xFF) as u8;
        self.mac[5] = (mac_lo & 0xFF) as u8;

        if self.mac == [0; 6] || self.mac == [0xFF; 6] {
            self.mac = [0x00, 0x10, 0x18, 0x55, 0x66, 0x77];
        }

        for (i, buf) in self.rx_buffers.iter().enumerate() {
            let phys = buf.physical() as u64;
            self.rx_ring[i] = Tg3RxDesc {
                addr_hi: (phys >> 32) as u32,
                addr_lo: phys as u32,
                idx_len: BUFFER_SIZE as u32,
                type_flags: 0,
            };
        }

        let rx_phys = self.rx_ring.physical() as u64;
        self.write_reg(TG3_RX_PROD_RING_HOST_ADDR_HI, (rx_phys >> 32) as u32);
        self.write_reg(TG3_RX_PROD_RING_HOST_ADDR_LO, rx_phys as u32);

        let tx_phys = self.tx_ring.physical() as u64;
        self.write_reg(TG3_TX_PROD_RING_HOST_ADDR_HI, (tx_phys >> 32) as u32);
        self.write_reg(TG3_TX_PROD_RING_HOST_ADDR_LO, tx_phys as u32);

        self.write_reg(TG3_MAC_MODE, TG3_MAC_MODE_ENABLE | TG3_MAC_MODE_TXEN | TG3_MAC_MODE_RXEN);

        crate::klog!(
            "tg3: Broadcom NetXtreme Gigabit Ethernet registered (MAC: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x})",
            self.mac[0], self.mac[1], self.mac[2], self.mac[3], self.mac[4], self.mac[5]
        );

        Ok(())
    }
}

impl NetDevice for Tg3Device {
    fn driver_name(&self) -> &'static str {
        "broadcom-tg3"
    }

    fn mac_address(&self) -> [u8; 6] {
        self.mac
    }

    fn send_packet(&mut self, packet: &[u8]) -> Result<(), NetDeviceError> {
        let len = packet.len().min(BUFFER_SIZE);
        let cur = self.tx_cur;

        self.tx_buffers[cur][..len].copy_from_slice(&packet[..len]);
        let phys = self.tx_buffers[cur].physical() as u64;

        self.tx_ring[cur] = Tg3TxDesc {
            addr_hi: (phys >> 32) as u32,
            addr_lo: phys as u32,
            len_flags: (len as u32) | TG3_TX_FLAG_END,
            vlan_tag: 0,
        };

        self.tx_cur = (cur + 1) % RING_SIZE;
        self.write_reg(TG3_TX_PROD_NIC_RING, self.tx_cur as u32);

        self.tx_packets += 1;
        self.tx_bytes += len;
        Ok(())
    }

    fn receive_packet(&mut self, buf: &mut [u8]) -> Result<Option<usize>, NetDeviceError> {
        // Проверяем флаг готовности дескриптора в кольце приема
        let cur = self.rx_cur;
        let desc = self.rx_ring[cur];
        let len = (desc.idx_len as usize) & 0xFFFF;
        if len == 0 || len > BUFFER_SIZE {
            return Ok(None);
        }

        let copy_len = len.min(buf.len());
        buf[..copy_len].copy_from_slice(&self.rx_buffers[cur][..copy_len]);

        self.rx_ring[cur].idx_len = BUFFER_SIZE as u32;
        self.rx_cur = (cur + 1) % RING_SIZE;

        self.rx_packets += 1;
        self.rx_bytes += copy_len;
        Ok(Some(copy_len))
    }

    fn rx_bytes(&self) -> usize { self.rx_bytes }
    fn rx_packets(&self) -> usize { self.rx_packets }
    fn tx_bytes(&self) -> usize { self.tx_bytes }
    fn tx_packets(&self) -> usize { self.tx_packets }
}

pub fn probe(pci: &PciDevice, dm: &mut DeviceManager) -> Result<(), &'static str> {
    let bar0 = pci.bars[0].ok_or("tg3: missing BAR0 MMIO")?;
    let mmio = crate::memory::map_kernel_mmio(bar0.address, bar0.size as usize)
        .map_err(|_| "tg3: failed to map BAR0 MMIO")?;

    let dev = Tg3Device::new(pci, mmio.as_u64())?;
    dm.register_net_device(Arc::new(Mutex::new(dev)));
    Ok(())
}
