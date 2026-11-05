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

pub struct IxgbeDevice {
    base: u64,
    mac: [u8; 6],
    rx_ring: Dma<[IxgbeAdvRxDesc; RING_SIZE]>,
    tx_ring: Dma<[IxgbeAdvTxDesc; RING_SIZE]>,
    rx_buffers: [Dma<[u8; BUFFER_SIZE]>; RING_SIZE],
    tx_buffers: [Dma<[u8; BUFFER_SIZE]>; RING_SIZE],
    rx_cur: usize,
    tx_cur: usize,
    rx_bytes: usize,
    rx_packets: usize,
    tx_bytes: usize,
    tx_packets: usize,
}

unsafe impl Send for IxgbeDevice {}
unsafe impl Sync for IxgbeDevice {}

fn alloc_buffers() -> Result<[Dma<[u8; BUFFER_SIZE]>; RING_SIZE], &'static str> {
    Ok([
        Dma::zeroed().map_err(|_| "ixgbe: dma alloc failed")?,
        Dma::zeroed().map_err(|_| "ixgbe: dma alloc failed")?,
        Dma::zeroed().map_err(|_| "ixgbe: dma alloc failed")?,
        Dma::zeroed().map_err(|_| "ixgbe: dma alloc failed")?,
        Dma::zeroed().map_err(|_| "ixgbe: dma alloc failed")?,
        Dma::zeroed().map_err(|_| "ixgbe: dma alloc failed")?,
        Dma::zeroed().map_err(|_| "ixgbe: dma alloc failed")?,
        Dma::zeroed().map_err(|_| "ixgbe: dma alloc failed")?,
        Dma::zeroed().map_err(|_| "ixgbe: dma alloc failed")?,
        Dma::zeroed().map_err(|_| "ixgbe: dma alloc failed")?,
        Dma::zeroed().map_err(|_| "ixgbe: dma alloc failed")?,
        Dma::zeroed().map_err(|_| "ixgbe: dma alloc failed")?,
        Dma::zeroed().map_err(|_| "ixgbe: dma alloc failed")?,
        Dma::zeroed().map_err(|_| "ixgbe: dma alloc failed")?,
        Dma::zeroed().map_err(|_| "ixgbe: dma alloc failed")?,
        Dma::zeroed().map_err(|_| "ixgbe: dma alloc failed")?,
    ])
}

impl IxgbeDevice {
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
            rx_ring: Dma::zeroed().map_err(|_| "ixgbe: failed allocating RX ring")?,
            tx_ring: Dma::zeroed().map_err(|_| "ixgbe: failed allocating TX ring")?,
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
        // Считываем заводской MAC-адрес из регистров фильтрации RAL/RAH
        let ral = self.read_reg(IXGBE_RAL);
        let rah = self.read_reg(IXGBE_RAH);
        self.mac[0] = (ral & 0xFF) as u8;
        self.mac[1] = ((ral >> 8) & 0xFF) as u8;
        self.mac[2] = ((ral >> 16) & 0xFF) as u8;
        self.mac[3] = ((ral >> 24) & 0xFF) as u8;
        self.mac[4] = (rah & 0xFF) as u8;
        self.mac[5] = ((rah >> 8) & 0xFF) as u8;

        // Если MAC пустой, генерируем локальный
        if self.mac == [0; 6] || self.mac == [0xFF; 6] {
            self.mac = [0x00, 0x1B, 0x21, 0x88, 0x99, 0xAA];
            let new_ral = self.mac[0] as u32
                | ((self.mac[1] as u32) << 8)
                | ((self.mac[2] as u32) << 16)
                | ((self.mac[3] as u32) << 24);
            let new_rah = (self.mac[4] as u32 | ((self.mac[5] as u32) << 8)) | IXGBE_RAH_AV;
            self.write_reg(IXGBE_RAL, new_ral);
            self.write_reg(IXGBE_RAH, new_rah);
        }

        // Настраиваем дескрипторы RX
        for (i, buf) in self.rx_buffers.iter().enumerate() {
            self.rx_ring[i] = IxgbeAdvRxDesc {
                pkt_addr: buf.physical() as u64,
                hdr_addr: 0,
            };
        }

        let rx_phys = self.rx_ring.physical() as u64;
        self.write_reg(IXGBE_RDBAL, rx_phys as u32);
        self.write_reg(IXGBE_RDBAH, (rx_phys >> 32) as u32);
        self.write_reg(IXGBE_RDLEN, (RING_SIZE * core::mem::size_of::<IxgbeAdvRxDesc>()) as u32);
        self.write_reg(IXGBE_RDH, 0);
        self.write_reg(IXGBE_RDT, (RING_SIZE - 1) as u32);

        // Настраиваем дескрипторы TX
        let tx_phys = self.tx_ring.physical() as u64;
        self.write_reg(IXGBE_TDBAL, tx_phys as u32);
        self.write_reg(IXGBE_TDBAH, (tx_phys >> 32) as u32);
        self.write_reg(IXGBE_TDLEN, (RING_SIZE * core::mem::size_of::<IxgbeAdvTxDesc>()) as u32);
        self.write_reg(IXGBE_TDH, 0);
        self.write_reg(IXGBE_TDT, 0);

        // Включаем передачу и прием
        self.write_reg(IXGBE_DMATXCTL, IXGBE_DMATXCTL_TE);
        self.write_reg(IXGBE_FCTRL, IXGBE_FCTRL_BAM | IXGBE_FCTRL_MPE | IXGBE_FCTRL_UPE);
        self.write_reg(IXGBE_RXCTRL, IXGBE_RXCTRL_RXEN);

        crate::klog!(
            "ixgbe: 10GbE controller registered (MAC: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x})",
            self.mac[0], self.mac[1], self.mac[2], self.mac[3], self.mac[4], self.mac[5]
        );

        Ok(())
    }
}

impl NetDevice for IxgbeDevice {
    fn driver_name(&self) -> &'static str {
        "intel-ixgbe"
    }

    fn mac_address(&self) -> [u8; 6] {
        self.mac
    }

    fn send_packet(&mut self, packet: &[u8]) -> Result<(), NetDeviceError> {
        let len = packet.len().min(BUFFER_SIZE);
        let cur = self.tx_cur;

        self.tx_buffers[cur][..len].copy_from_slice(&packet[..len]);
        self.tx_ring[cur] = IxgbeAdvTxDesc {
            buffer_addr: self.tx_buffers[cur].physical() as u64,
            cmd_type_len: (len as u32) | IXGBE_TXD_CMD_EOP | IXGBE_TXD_CMD_IFCS | IXGBE_TXD_CMD_RS | IXGBE_TXD_CMD_DEXT,
            olinfo_status: (len as u32) << 20, // Packet length in PAYLEN field
        };

        self.tx_cur = (cur + 1) % RING_SIZE;
        self.write_reg(IXGBE_TDT, self.tx_cur as u32);

        self.tx_packets += 1;
        self.tx_bytes += len;
        Ok(())
    }

    fn receive_packet(&mut self, buf: &mut [u8]) -> Result<Option<usize>, NetDeviceError> {
        let cur = self.rx_cur;
        let rdh = self.read_reg(IXGBE_RDH) as usize;
        if cur == rdh {
            return Ok(None);
        }

        let len = BUFFER_SIZE.min(buf.len());
        buf[..len].copy_from_slice(&self.rx_buffers[cur][..len]);

        self.write_reg(IXGBE_RDT, cur as u32);
        self.rx_cur = (cur + 1) % RING_SIZE;

        self.rx_packets += 1;
        self.rx_bytes += len;
        Ok(Some(len))
    }

    fn rx_bytes(&self) -> usize { self.rx_bytes }
    fn rx_packets(&self) -> usize { self.rx_packets }
    fn tx_bytes(&self) -> usize { self.tx_bytes }
    fn tx_packets(&self) -> usize { self.tx_packets }
}

pub fn probe(pci: &PciDevice, dm: &mut DeviceManager) -> Result<(), &'static str> {
    let bar0 = pci.bars[0].ok_or("ixgbe: missing BAR0 MMIO")?;
    let mmio = crate::memory::map_kernel_mmio(bar0.address, bar0.size as usize)
        .map_err(|_| "ixgbe: failed to map BAR0 MMIO")?;

    let dev = IxgbeDevice::new(pci, mmio.as_u64())?;
    dm.register_net_device(Arc::new(Mutex::new(dev)));
    Ok(())
}
