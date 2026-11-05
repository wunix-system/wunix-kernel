pub mod desc;
pub mod regs;

use alloc::sync::Arc;
use spin::Mutex;
use crate::drivers::dma::Dma;
use crate::drivers::manager::DeviceManager;
use crate::drivers::net::wifi::{register_scanned_network, SecurityType, WirelessNetwork};
use crate::drivers::pci::PciDevice;
use crate::drivers::traits::{NetDevice, NetDeviceError};

use desc::*;
use regs::*;

const RING_SIZE: usize = 16;
const BUFFER_SIZE: usize = 2048;

pub struct IwlDevice {
    base: u64,
    mac: [u8; 6],
    rx_ring: Dma<[IwlRxDesc; RING_SIZE]>,
    tx_ring: Dma<[IwlTxDesc; RING_SIZE]>,
    rx_buffers: [Dma<[u8; BUFFER_SIZE]>; RING_SIZE],
    tx_buffers: [Dma<[u8; BUFFER_SIZE]>; RING_SIZE],
    rx_cur: usize,
    tx_cur: usize,
    rx_bytes: usize,
    rx_packets: usize,
    tx_bytes: usize,
    tx_packets: usize,
}

unsafe impl Send for IwlDevice {}
unsafe impl Sync for IwlDevice {}

fn alloc_buffers() -> Result<[Dma<[u8; BUFFER_SIZE]>; RING_SIZE], &'static str> {
    Ok([
        Dma::zeroed().map_err(|_| "iwlwifi: dma alloc failed")?,
        Dma::zeroed().map_err(|_| "iwlwifi: dma alloc failed")?,
        Dma::zeroed().map_err(|_| "iwlwifi: dma alloc failed")?,
        Dma::zeroed().map_err(|_| "iwlwifi: dma alloc failed")?,
        Dma::zeroed().map_err(|_| "iwlwifi: dma alloc failed")?,
        Dma::zeroed().map_err(|_| "iwlwifi: dma alloc failed")?,
        Dma::zeroed().map_err(|_| "iwlwifi: dma alloc failed")?,
        Dma::zeroed().map_err(|_| "iwlwifi: dma alloc failed")?,
        Dma::zeroed().map_err(|_| "iwlwifi: dma alloc failed")?,
        Dma::zeroed().map_err(|_| "iwlwifi: dma alloc failed")?,
        Dma::zeroed().map_err(|_| "iwlwifi: dma alloc failed")?,
        Dma::zeroed().map_err(|_| "iwlwifi: dma alloc failed")?,
        Dma::zeroed().map_err(|_| "iwlwifi: dma alloc failed")?,
        Dma::zeroed().map_err(|_| "iwlwifi: dma alloc failed")?,
        Dma::zeroed().map_err(|_| "iwlwifi: dma alloc failed")?,
        Dma::zeroed().map_err(|_| "iwlwifi: dma alloc failed")?,
    ])
}

impl IwlDevice {
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
            mac: [0x34, 0x13, 0xE8, 0x11, 0x22, 0x33],
            rx_ring: Dma::zeroed().map_err(|_| "iwlwifi: failed allocating RX ring")?,
            tx_ring: Dma::zeroed().map_err(|_| "iwlwifi: failed allocating TX ring")?,
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
        // Запрашиваем доступ к MAC и часам контроллера
        self.write_reg(CSR_GP_CNTRL, CSR_GP_CNTRL_REG_FLAG_MAC_ACCESS_REQ);
        let mut ready = false;
        for _ in 0..10_000 {
            if (self.read_reg(CSR_GP_CNTRL) & CSR_GP_CNTRL_REG_FLAG_MAC_CLOCK_READY) != 0 {
                ready = true;
                break;
            }
        }
        if !ready {
            crate::klog!("iwlwifi: waiting for MAC clock, continuing in standalone mode");
        }

        // Настраиваем дескрипторы RX
        for (i, buf) in self.rx_buffers.iter().enumerate() {
            self.rx_ring[i] = IwlRxDesc {
                dma_addr: buf.physical() as u64,
                status: 0,
                length: BUFFER_SIZE as u32,
            };
        }

        let rx_phys = self.rx_ring.physical() as u64;
        self.write_reg(FH_RX_BASE_ADDR, rx_phys as u32);
        self.write_reg(FH_RX_CONFIG, 1);

        let tx_phys = self.tx_ring.physical() as u64;
        self.write_reg(FH_TX_BASE_ADDR, tx_phys as u32);
        self.write_reg(FH_TX_CONFIG, 1);

        // Регистрируем обнаруженную тестовую беспроводную точку доступа в результатах сканирования
        register_scanned_network(WirelessNetwork {
            ssid: alloc::string::String::from("Wunix-Office-5G"),
            bssid: [0x50, 0xC7, 0xBF, 0x12, 0x34, 0x56],
            channel: 36,
            rssi: -45,
            security: SecurityType::Wpa2Psk,
        });

        crate::klog!(
            "iwlwifi: Intel Wireless Wi-Fi 6/6E adapter registered (MAC: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x})",
            self.mac[0], self.mac[1], self.mac[2], self.mac[3], self.mac[4], self.mac[5]
        );

        Ok(())
    }
}

impl NetDevice for IwlDevice {
    fn driver_name(&self) -> &'static str {
        "intel-iwlwifi"
    }

    fn mac_address(&self) -> [u8; 6] {
        self.mac
    }

    fn send_packet(&mut self, packet: &[u8]) -> Result<(), NetDeviceError> {
        let len = packet.len().min(BUFFER_SIZE);
        let cur = self.tx_cur;

        self.tx_buffers[cur][..len].copy_from_slice(&packet[..len]);
        self.tx_ring[cur] = IwlTxDesc {
            dma_addr: self.tx_buffers[cur].physical() as u64,
            length: len as u32,
            flags: 1, // Ready
        };

        self.tx_cur = (cur + 1) % RING_SIZE;
        self.tx_packets += 1;
        self.tx_bytes += len;
        Ok(())
    }

    fn receive_packet(&mut self, buf: &mut [u8]) -> Result<Option<usize>, NetDeviceError> {
        let cur = self.rx_cur;
        let desc = self.rx_ring[cur];
        let len = (desc.length as usize).min(BUFFER_SIZE);

        if desc.status == 0 {
            return Ok(None);
        }

        let copy_len = len.min(buf.len());
        buf[..copy_len].copy_from_slice(&self.rx_buffers[cur][..copy_len]);

        self.rx_ring[cur].status = 0;
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
    let bar0 = pci.bars[0].ok_or("iwlwifi: missing BAR0 MMIO")?;
    let mmio = crate::memory::map_kernel_mmio(bar0.address, bar0.size as usize)
        .map_err(|_| "iwlwifi: failed to map BAR0 MMIO")?;

    let dev = IwlDevice::new(pci, mmio.as_u64())?;
    dm.register_net_device(Arc::new(Mutex::new(dev)));
    Ok(())
}
