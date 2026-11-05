mod hw;

use x86_64::structures::paging::PhysFrame;

use crate::drivers::pci::PciDevice;
use crate::drivers::traits::{NetDevice, NetDeviceError};
use crate::memory::{allocate_contiguous_frames, deallocate_frame, hhdm_offset, map_kernel_mmio};

pub use hw::*;

/// Высокопроизводительный драйвер сетевых контроллеров Intel Gigabit Ethernet (e1000).
/// Является универсальным стандартным сетевым адаптером во всех основных гипервизорах:
/// VirtualBox (Intel PRO/1000 MT Desktop), VMware (82545EM) и Proxmox/QEMU (-net nic,model=e1000).
pub struct E1000Device {
    mmio_base: *mut u8,
    mac: [u8; 6],
    dma_frame: PhysFrame,
    dma_virt: *mut u8,
    rx_cur: usize,
    tx_cur: usize,
    rx_bytes_count: usize,
    rx_packets_count: usize,
    tx_bytes_count: usize,
    tx_packets_count: usize,
}

unsafe impl Send for E1000Device {}
unsafe impl Sync for E1000Device {}

impl E1000Device {
    fn write_reg(&self, offset: usize, val: u32) {
        unsafe {
            core::ptr::write_volatile(self.mmio_base.add(offset) as *mut u32, val);
        }
    }

    fn read_reg(&self, offset: usize) -> u32 {
        unsafe {
            core::ptr::read_volatile(self.mmio_base.add(offset) as *const u32)
        }
    }

    pub fn new(pci: &PciDevice) -> Result<Self, &'static str> {
        pci.enable_memory_space();
        pci.enable_bus_mastering();

        let bar0 = pci.bar(0).ok_or("e1000: BAR0 MMIO not found")?;
        let mmio_virt = map_kernel_mmio(bar0.address, bar0.size as usize)
            .map_err(|_| "e1000: failed to map MMIO")?;
        let mmio_base = mmio_virt.as_mut_ptr::<u8>();

        // Выделяем 16 непрерывных фреймов (64 КиБ) под дескрипторы и буферы RX/TX
        let dma_frame = allocate_contiguous_frames(16)
            .ok_or("e1000: failed to allocate DMA frames")?;
        let dma_phys = dma_frame.start_address().as_u64();
        let dma_virt = (hhdm_offset() + dma_phys) as *mut u8;

        unsafe {
            core::ptr::write_bytes(dma_virt, 0, 16 * 4096);
        }

        let mut dev = Self {
            mmio_base,
            mac: [0u8; 6],
            dma_frame,
            dma_virt,
            rx_cur: 0,
            tx_cur: 0,
            rx_bytes_count: 0,
            rx_packets_count: 0,
            tx_bytes_count: 0,
            tx_packets_count: 0,
        };

        // Сброс и программная инициализация контроллера
        dev.reset();
        dev.read_mac_address();
        dev.init_rx(dma_phys);
        dev.init_tx(dma_phys);

        Ok(dev)
    }

    fn reset(&self) {
        // Отключаем прерывания на время начальной конфигурации
        self.write_reg(REG_IMC, 0xFFFF_FFFF);

        // Инициируем программный сброс контроллера через бит 26 регистра CTRL
        let ctrl = self.read_reg(REG_CTRL);
        self.write_reg(REG_CTRL, ctrl | (1 << 26));

        // Ожидаем завершения аппаратного сброса (контроллер сбрасывает бит 26 обратно в 0)
        let mut timeout = 10_000;
        while timeout > 0 && (self.read_reg(REG_CTRL) & (1 << 26)) != 0 {
            core::hint::spin_loop();
            timeout -= 1;
        }

        // Повторно отключаем маски прерываний и сбрасываем причину
        self.write_reg(REG_IMC, 0xFFFF_FFFF);
        let _ = self.read_reg(REG_ICR);

        // Обнуляем Multicast Table Array (MTA)
        for i in 0..128 {
            self.write_reg(REG_MTA + i * 4, 0);
        }

        // Включаем Auto-Negotiation скорости линии (1000/100/10 Мбит/с Full Duplex)
        let ctrl_val = self.read_reg(REG_CTRL);
        self.write_reg(REG_CTRL, ctrl_val | (1 << 5) | (1 << 6));
    }

    fn read_mac_address(&mut self) {
        let ral = self.read_reg(REG_RAL);
        let rah = self.read_reg(REG_RAH);

        // Проверяем бит валидности адреса (бит 31 в RAH)
        if (rah & (1 << 31)) != 0 {
            self.mac[0] = (ral & 0xFF) as u8;
            self.mac[1] = ((ral >> 8) & 0xFF) as u8;
            self.mac[2] = ((ral >> 16) & 0xFF) as u8;
            self.mac[3] = ((ral >> 24) & 0xFF) as u8;
            self.mac[4] = (rah & 0xFF) as u8;
            self.mac[5] = ((rah >> 8) & 0xFF) as u8;
        } else {
            // Если гипервизор не заполнил RAL/RAH при старте, опрашиваем внутренний EEPROM контроллера
            self.mac[0] = self.read_eeprom_word(0) as u8;
            self.mac[1] = (self.read_eeprom_word(0) >> 8) as u8;
            self.mac[2] = self.read_eeprom_word(1) as u8;
            self.mac[3] = (self.read_eeprom_word(1) >> 8) as u8;
            self.mac[4] = self.read_eeprom_word(2) as u8;
            self.mac[5] = (self.read_eeprom_word(2) >> 8) as u8;
        }

        // Если MAC остался пустым, используем стандартный паравиртуальный MAC
        if self.mac == [0; 6] {
            self.mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
        }

        // Программируем фильтр MAC-адреса с установленным битом валидности (AV = 1 << 31),
        // чтобы сетевая карта не сбрасывала входящие Unicast пакеты (ARP, DHCP, TCP, ICMP)
        let ral_val = (self.mac[0] as u32)
            | ((self.mac[1] as u32) << 8)
            | ((self.mac[2] as u32) << 16)
            | ((self.mac[3] as u32) << 24);
        let rah_val = (self.mac[4] as u32)
            | ((self.mac[5] as u32) << 8)
            | (1 << 31);
        self.write_reg(REG_RAL, ral_val);
        self.write_reg(REG_RAH, rah_val);
    }

    fn read_eeprom_word(&self, addr: u8) -> u16 {
        // Бит 0: старт чтения, биты 8..15: адрес EEPROM
        self.write_reg(REG_EERD, 1 | ((addr as u32) << 8));
        let mut timeout = 10_000;
        while timeout > 0 {
            let val = self.read_reg(REG_EERD);
            if (val & (1 << 4)) != 0 {
                // Бит 4: флаг завершения чтения (Done)
                return ((val >> 16) & 0xFFFF) as u16;
            }
            core::hint::spin_loop();
            timeout -= 1;
        }
        0
    }

    fn init_rx(&mut self, dma_phys: u64) {
        let rx_ring_phys = dma_phys;
        let rx_descs = unsafe {
            core::slice::from_raw_parts_mut(self.dma_virt as *mut E1000RxDesc, NUM_RX_DESCRIPTORS)
        };

        // Буферы входящих пакетов размещаем со смещения 4 КиБ
        let rx_buf_base_phys = dma_phys + RX_BUFFERS_OFFSET as u64;
        for i in 0..NUM_RX_DESCRIPTORS {
            rx_descs[i].addr = rx_buf_base_phys + (i * BUFFER_SIZE) as u64;
            rx_descs[i].status = 0;
        }

        self.write_reg(REG_RDBAL, rx_ring_phys as u32);
        self.write_reg(REG_RDBAH, (rx_ring_phys >> 32) as u32);
        self.write_reg(REG_RDLEN, (NUM_RX_DESCRIPTORS * core::mem::size_of::<E1000RxDesc>()) as u32);
        self.write_reg(REG_RDH, 0);
        self.write_reg(REG_RDT, (NUM_RX_DESCRIPTORS - 1) as u32);

        // Активируем приемник пакетов с приемом Broadcast и отсечением CRC
        let rctl = RCTL_EN | RCTL_SBP | RCTL_UPE | RCTL_MPE | RCTL_BAM | RCTL_BSIZE_2048 | RCTL_SECRC;
        self.write_reg(REG_RCTL, rctl);
    }

    fn init_tx(&mut self, dma_phys: u64) {
        let tx_ring_phys = dma_phys + TX_RING_OFFSET as u64;
        let tx_descs = unsafe {
            core::slice::from_raw_parts_mut(
                (self.dma_virt as usize + TX_RING_OFFSET) as *mut E1000TxDesc,
                NUM_TX_DESCRIPTORS,
            )
        };

        // Буферы передачи размещаем со смещения 36 КиБ
        let tx_buf_base_phys = dma_phys + TX_BUFFERS_OFFSET as u64;
        for i in 0..NUM_TX_DESCRIPTORS {
            tx_descs[i].addr = tx_buf_base_phys + (i * BUFFER_SIZE) as u64;
            tx_descs[i].status = TXD_STAT_DD;
            tx_descs[i].cmd = 0;
        }

        self.write_reg(REG_TDBAL, tx_ring_phys as u32);
        self.write_reg(REG_TDBAH, (tx_ring_phys >> 32) as u32);
        self.write_reg(REG_TDLEN, (NUM_TX_DESCRIPTORS * core::mem::size_of::<E1000TxDesc>()) as u32);
        self.write_reg(REG_TDH, 0);
        self.write_reg(REG_TDT, 0);

        // Активируем передатчик с параметрами коллизий по спецификации Intel 82540
        let tctl = TCTL_EN | TCTL_PSP | (0x0F << TCTL_CT_SHIFT) | (0x40 << TCTL_COLD_SHIFT);
        self.write_reg(REG_TCTL, tctl);
    }
}

impl NetDevice for E1000Device {
    fn driver_name(&self) -> &'static str {
        "intel-e1000"
    }

    fn mac_address(&self) -> [u8; 6] {
        self.mac
    }

    fn send_packet(&mut self, packet: &[u8]) -> Result<(), NetDeviceError> {
        if packet.len() > BUFFER_SIZE {
            return Err(NetDeviceError::PacketTooLarge);
        }

        let tx_descs = unsafe {
            core::slice::from_raw_parts_mut(
                (self.dma_virt as usize + TX_RING_OFFSET) as *mut E1000TxDesc,
                NUM_TX_DESCRIPTORS,
            )
        };

        let idx = self.tx_cur;
        let tx_status = unsafe {
            let ptr = &tx_descs[idx].status as *const u8;
            core::ptr::read_volatile(ptr)
        };
        if (tx_status & TXD_STAT_DD) == 0 {
            return Err(NetDeviceError::NotReady);
        }

        let tx_buf_virt = (self.dma_virt as usize + TX_BUFFERS_OFFSET + (idx * BUFFER_SIZE)) as *mut u8;
        unsafe {
            core::ptr::copy_nonoverlapping(packet.as_ptr(), tx_buf_virt, packet.len());
        }

        tx_descs[idx].length = packet.len() as u16;
        tx_descs[idx].cmd = TXD_CMD_EOP | TXD_CMD_IFCS | TXD_CMD_RS;
        unsafe {
            core::ptr::write_volatile(&mut tx_descs[idx].status, 0);
        }

        core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::Release);

        self.tx_cur = (self.tx_cur + 1) % NUM_TX_DESCRIPTORS;
        self.write_reg(REG_TDT, self.tx_cur as u32);
        self.tx_packets_count += 1;
        self.tx_bytes_count += packet.len();

        Ok(())
    }

    fn receive_packet(&mut self, buf: &mut [u8]) -> Result<Option<usize>, NetDeviceError> {
        let rx_descs = unsafe {
            core::slice::from_raw_parts_mut(self.dma_virt as *mut E1000RxDesc, NUM_RX_DESCRIPTORS)
        };

        let idx = self.rx_cur;
        let rx_status = unsafe {
            let ptr = &rx_descs[idx].status as *const u8;
            core::ptr::read_volatile(ptr)
        };
        if (rx_status & RXD_STAT_DD) == 0 {
            return Ok(None);
        }

        core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::Acquire);

        let pkt_len = rx_descs[idx].length as usize;
        if pkt_len > buf.len() {
            return Err(NetDeviceError::BufferTooSmall);
        }

        let rx_buf_virt = (self.dma_virt as usize + RX_BUFFERS_OFFSET + (idx * BUFFER_SIZE)) as *const u8;
        unsafe {
            core::ptr::copy_nonoverlapping(rx_buf_virt, buf.as_mut_ptr(), pkt_len);
            core::ptr::write_volatile(&mut rx_descs[idx].status, 0);
        }

        core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::Release);

        self.write_reg(REG_RDT, idx as u32);
        self.rx_cur = (self.rx_cur + 1) % NUM_RX_DESCRIPTORS;
        self.rx_packets_count += 1;
        self.rx_bytes_count += pkt_len;

        Ok(Some(pkt_len))
    }

    fn rx_bytes(&self) -> usize { self.rx_bytes_count }
    fn rx_packets(&self) -> usize { self.rx_packets_count }
    fn tx_bytes(&self) -> usize { self.tx_bytes_count }
    fn tx_packets(&self) -> usize { self.tx_packets_count }
}

impl Drop for E1000Device {
    fn drop(&mut self) {
        self.write_reg(REG_RCTL, 0);
        self.write_reg(REG_TCTL, 0);
        for i in 0..16 {
            let frame = PhysFrame::containing_address(
                self.dma_frame.start_address() + (i as u64 * 4096)
            );
            deallocate_frame(frame);
        }
    }
}

/// Проверяет принадлежность PCI устройства семейству Intel e1000 и инициализирует драйвер
pub fn probe_pci(dev: &PciDevice) -> Option<E1000Device> {
    if dev.vendor_id != INTEL_PCI_VENDOR_ID {
        return None;
    }

    match dev.device_id {
        E1000_DEV_82540EM
        | E1000_DEV_82545EM
        | E1000_DEV_82543GC
        | E1000_DEV_82571EB
        | E1000_DEV_82572EI
        | E1000_DEV_82573L
        | E1000_DEV_82574L
        | E1000_DEV_82579LM
        | E1000_DEV_82579V
        | E1000_DEV_I217LM
        | E1000_DEV_I217V
        | E1000_DEV_I218V
        | E1000_DEV_I219LM
        | E1000_DEV_I219V
        | E1000_DEV_I219V2
        | E1000_DEV_82575EB
        | E1000_DEV_82576
        | E1000_DEV_I350
        | E1000_DEV_I210
        | E1000_DEV_I211
        | E1000_DEV_I225_LM
        | E1000_DEV_I225_V
        | E1000_DEV_I226_V
        | E1000_DEV_I226_LM => E1000Device::new(dev).ok(),
        _ => None,
    }
}
