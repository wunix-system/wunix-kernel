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
    rx_std_ring: Dma<[Tg3RxDesc; RING_SIZE]>,
    rx_rcb_ring: Dma<[Tg3RxDesc; RING_SIZE]>,
    tx_ring: Dma<[Tg3TxDesc; RING_SIZE]>,
    status_block: Dma<Tg3HwStatus>,
    rx_buffers: [Dma<[u8; BUFFER_SIZE]>; RING_SIZE],
    tx_buffers: [Dma<[u8; BUFFER_SIZE]>; RING_SIZE],
    rx_rcb_ptr: usize,
    rx_std_prod_idx: usize,
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
            rx_std_ring: Dma::zeroed().map_err(|_| "tg3: failed allocating RX std ring")?,
            rx_rcb_ring: Dma::zeroed().map_err(|_| "tg3: failed allocating RX return ring")?,
            tx_ring: Dma::zeroed().map_err(|_| "tg3: failed allocating TX ring")?,
            status_block: Dma::zeroed().map_err(|_| "tg3: failed allocating status block")?,
            rx_buffers: alloc_buffers()?,
            tx_buffers: alloc_buffers()?,
            rx_rcb_ptr: 0,
            rx_std_prod_idx: 0,
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

        // 1. Инициализируем дескрипторы RX Producer Ring
        for (i, buf) in self.rx_buffers.iter().enumerate() {
            let phys = buf.physical() as u64;
            self.rx_std_ring[i] = Tg3RxDesc {
                addr_hi: (phys >> 32) as u32,
                addr_lo: phys as u32,
                idx_len: BUFFER_SIZE as u32,
                type_flags: RXD_FLAG_END,
                ip_tcp_csum: 0,
                err_vlan: 0,
                reserved: 0,
                opaque: RXD_OPAQUE_RING_STD | (i as u32),
            };
        }

        // 2. Регистрируем RX Standard Producer Ring в RCVDBDI_STD_BD (0x2450)
        let rx_std_phys = self.rx_std_ring.physical() as u64;
        self.write_reg(TG3_RCVDBDI_STD_BD, (rx_std_phys >> 32) as u32);
        self.write_reg(TG3_RCVDBDI_STD_BD + 4, rx_std_phys as u32);
        self.write_reg(TG3_RCVDBDI_STD_BD + 8, (RING_SIZE as u32) << 16);
        self.write_reg(TG3_RCVDBDI_STD_BD + 12, TG3_NIC_SRAM_RX_BUFFER_DESC as u32);

        // Отключаем Mini Ring
        self.write_reg(TG3_RCVDBDI_MINI_BD + 8, TG3_BDINFO_FLAGS_DISABLED);

        // Уведомляем контроллер о доступных буферах приема
        self.rx_std_prod_idx = RING_SIZE;
        self.write_reg(TG3_MAILBOX_RCV_STD_PROD_IDX, (RING_SIZE as u32) & 0xFFFF);

        // 3. Инициализируем RX Return Ring 0 в NIC_SRAM_RCV_RET_RCB (0x8200)
        let rx_rcb_phys = self.rx_rcb_ring.physical() as u64;
        self.write_reg(TG3_NIC_SRAM_RCV_RET_RCB, (rx_rcb_phys >> 32) as u32);
        self.write_reg(TG3_NIC_SRAM_RCV_RET_RCB + 4, rx_rcb_phys as u32);
        self.write_reg(TG3_NIC_SRAM_RCV_RET_RCB + 8, (RING_SIZE as u32) << 16);
        self.write_reg(TG3_NIC_SRAM_RCV_RET_RCB + 12, 0);

        // Сбрасываем Return Consumer Mailbox
        self.rx_rcb_ptr = 0;
        self.write_reg(TG3_MAILBOX_RCVRET_CON_IDX_0, 0);

        // 4. Инициализируем TX Ring в NIC_SRAM_SEND_RCB (0x8100)
        let tx_phys = self.tx_ring.physical() as u64;
        self.write_reg(TG3_NIC_SRAM_SEND_RCB, (tx_phys >> 32) as u32);
        self.write_reg(TG3_NIC_SRAM_SEND_RCB + 4, tx_phys as u32);
        self.write_reg(TG3_NIC_SRAM_SEND_RCB + 8, (RING_SIZE as u32) << 16);
        self.write_reg(TG3_NIC_SRAM_SEND_RCB + 12, TG3_NIC_SRAM_TX_BUFFER_DESC as u32);
        self.write_reg(TG3_MAILBOX_SNDHOST_PROD_IDX_0, 0);

        // 5. Настраиваем Host Coalescing & Status Block DMA
        let status_phys = self.status_block.physical() as u64;
        self.write_reg(TG3_HOSTCC_STATUS_BLK_HOST_ADDR_HI, (status_phys >> 32) as u32);
        self.write_reg(TG3_HOSTCC_STATUS_BLK_HOST_ADDR_LO, status_phys as u32);
        self.write_reg(TG3_HOSTCC_STATUS_BLK_NIC_ADDR, TG3_NIC_SRAM_STATUS_BLK as u32);
        self.write_reg(TG3_HOSTCC_MODE, TG3_HOSTCC_MODE_ENABLE | TG3_HOSTCC_MODE_NOW);

        // 6. Включаем MAC приемопередатчик
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

        core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::Release);

        self.tx_cur = (cur + 1) % RING_SIZE;
        self.write_reg(TG3_MAILBOX_SNDHOST_PROD_IDX_0, self.tx_cur as u32);

        self.tx_packets += 1;
        self.tx_bytes += len;
        Ok(())
    }

    fn receive_packet(&mut self, buf: &mut [u8]) -> Result<Option<usize>, NetDeviceError> {
        // Читаем аппаратный индекс продюсера из статус-блока
        let hw_idx = unsafe {
            let ptr = &self.status_block.idx[0].rx_producer as *const u16;
            core::ptr::read_volatile(ptr) as usize
        };

        if self.rx_rcb_ptr == hw_idx {
            return Ok(None);
        }

        core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::Acquire);

        let cur_rcb = self.rx_rcb_ptr;
        let rcb_desc = unsafe {
            let ptr = &self.rx_rcb_ring[cur_rcb] as *const Tg3RxDesc;
            core::ptr::read_volatile(ptr)
        };

        let raw_len = (rcb_desc.idx_len & 0xFFFF) as usize;
        let len = raw_len.saturating_sub(4); // Минус CRC (FCS)
        let buf_idx = (rcb_desc.opaque & RXD_OPAQUE_INDEX_MASK) as usize;

        if buf_idx >= RING_SIZE || len == 0 || len > BUFFER_SIZE {
            // Продвигаем указатель при некорректном дескрипторе
            self.rx_rcb_ptr = (cur_rcb + 1) % RING_SIZE;
            self.write_reg(TG3_MAILBOX_RCVRET_CON_IDX_0, self.rx_rcb_ptr as u32);
            return Ok(None);
        }

        let copy_len = len.min(buf.len());
        buf[..copy_len].copy_from_slice(&self.rx_buffers[buf_idx][..copy_len]);

        // Возвращаем буфер в Standard RX Ring
        let phys = self.rx_buffers[buf_idx].physical() as u64;
        self.rx_std_ring[buf_idx] = Tg3RxDesc {
            addr_hi: (phys >> 32) as u32,
            addr_lo: phys as u32,
            idx_len: BUFFER_SIZE as u32,
            type_flags: RXD_FLAG_END,
            ip_tcp_csum: 0,
            err_vlan: 0,
            reserved: 0,
            opaque: RXD_OPAQUE_RING_STD | (buf_idx as u32),
        };

        core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::Release);

        self.rx_std_prod_idx = (self.rx_std_prod_idx + 1) % RING_SIZE;
        self.write_reg(TG3_MAILBOX_RCV_STD_PROD_IDX, self.rx_std_prod_idx as u32);

        self.rx_rcb_ptr = (cur_rcb + 1) % RING_SIZE;
        self.write_reg(TG3_MAILBOX_RCVRET_CON_IDX_0, self.rx_rcb_ptr as u32);

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
    pci.enable_memory_space();
    pci.enable_bus_mastering();

    let bar0 = pci.bars[0].ok_or("tg3: missing BAR0 MMIO")?;
    let mmio = crate::memory::map_kernel_mmio(bar0.address, bar0.size as usize)
        .map_err(|_| "tg3: failed to map BAR0 MMIO")?;

    let dev = Tg3Device::new(pci, mmio.as_u64())?;
    dm.register_net_device(Arc::new(Mutex::new(dev)));
    Ok(())
}
