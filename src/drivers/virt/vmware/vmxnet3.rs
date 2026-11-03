use x86_64::structures::paging::PhysFrame;

use crate::drivers::pci::PciDevice;
use crate::drivers::traits::NetDevice;
use crate::drivers::virtio::VirtioError;
use crate::memory::{allocate_contiguous_frames, deallocate_frame, hhdm_offset, map_kernel_mmio};

pub const VMXNET3_DEVICE_ID: u16 = 0x07B0;

const VMXNET3_REG_VRRS: usize = 0x00;
const VMXNET3_REG_UVRS: usize = 0x08;
const VMXNET3_REG_DSAL: usize = 0x10;
const VMXNET3_REG_DSAH: usize = 0x18;
const VMXNET3_REG_CMD: usize = 0x20;
const VMXNET3_REG_MACL: usize = 0x28;
const VMXNET3_REG_MACH: usize = 0x30;
const VMXNET3_REG_TXPROD: usize = 0x600;
const VMXNET3_REG_RXPROD: usize = 0x800;

const VMXNET3_CMD_ACTIVATE: u32 = 0x00;
const VMXNET3_CMD_QUIESCE: u32 = 0x01;
const VMXNET3_CMD_RESET: u32 = 0x02;

const RING_SIZE: usize = 32;
const PKT_BUF_SIZE: usize = 1536;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Vmxnet3TxDesc {
    addr: u64,
    len: u32,
    flags: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Vmxnet3RxDesc {
    addr: u64,
    len: u32,
    flags: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Vmxnet3CompDesc {
    rsvd: u32,
    len: u32,
    flags: u32,
    generation: u32,
}

#[repr(C)]
struct Vmxnet3SharedData {
    magic: u32,
    pad: u32,
    version: u32,
    guest_os: u32,
    tx_ring_addr: u64,
    rx_ring_addr: u64,
    tx_comp_addr: u64,
    rx_comp_addr: u64,
    tx_ring_size: u32,
    rx_ring_size: u32,
    tx_comp_size: u32,
    rx_comp_size: u32,
}

/// Высокопроизводительный паравиртуальный сетевой адаптер VMware vmxnet3.
pub struct Vmxnet3Device {
    mmio_base: *mut u8,
    mac: [u8; 6],
    dma_frame: PhysFrame,
    dma_virt: *mut u8,
    tx_prod: usize,
    rx_cons: usize,
}

unsafe impl Send for Vmxnet3Device {}
unsafe impl Sync for Vmxnet3Device {}

impl Vmxnet3Device {
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

        let bar0 = pci.bar(0).ok_or("vmxnet3: BAR0 MMIO not found")?;
        let mmio_virt = map_kernel_mmio(bar0.address, bar0.size as usize)
            .map_err(|_| "vmxnet3: failed to map MMIO registers")?;
        let mmio_base = mmio_virt.as_mut_ptr::<u8>();

        let dma_frame = allocate_contiguous_frames(4)
            .ok_or("vmxnet3: failed to allocate DMA frames")?;
        let dma_phys = dma_frame.start_address().as_u64();
        let dma_virt = (hhdm_offset() + dma_phys) as *mut u8;

        unsafe {
            core::ptr::write_bytes(dma_virt, 0, 4 * 4096);
        }

        let mut dev = Self {
            mmio_base,
            mac: [0u8; 6],
            dma_frame,
            dma_virt,
            tx_prod: 0,
            rx_cons: 0,
        };

        let mac_l = dev.read_reg(VMXNET3_REG_MACL);
        let mac_h = dev.read_reg(VMXNET3_REG_MACH);
        dev.mac[0] = (mac_l & 0xFF) as u8;
        dev.mac[1] = ((mac_l >> 8) & 0xFF) as u8;
        dev.mac[2] = ((mac_l >> 16) & 0xFF) as u8;
        dev.mac[3] = ((mac_l >> 24) & 0xFF) as u8;
        dev.mac[4] = (mac_h & 0xFF) as u8;
        dev.mac[5] = ((mac_h >> 8) & 0xFF) as u8;

        dev.write_reg(VMXNET3_REG_VRRS, 1);
        dev.write_reg(VMXNET3_REG_UVRS, 1);

        let shared = unsafe { &mut *(dma_virt as *mut Vmxnet3SharedData) };
        shared.magic = 0xBABE_FACE;
        shared.version = 1;
        shared.guest_os = 0x50000;

        let tx_ring_phys = dma_phys + 0x200;
        let rx_ring_phys = dma_phys + 0x600;
        let tx_comp_phys = dma_phys + 0xA00;
        let rx_comp_phys = dma_phys + 0xE00;

        shared.tx_ring_addr = tx_ring_phys;
        shared.rx_ring_addr = rx_ring_phys;
        shared.tx_comp_addr = tx_comp_phys;
        shared.rx_comp_addr = rx_comp_phys;
        shared.tx_ring_size = RING_SIZE as u32;
        shared.rx_ring_size = RING_SIZE as u32;
        shared.tx_comp_size = RING_SIZE as u32;
        shared.rx_comp_size = RING_SIZE as u32;

        let rx_descs = unsafe { core::slice::from_raw_parts_mut((dma_virt as usize + 0x600) as *mut Vmxnet3RxDesc, RING_SIZE) };
        let rx_bufs_phys = dma_phys + 0x1000;
        for i in 0..RING_SIZE {
            rx_descs[i].addr = rx_bufs_phys + (i * PKT_BUF_SIZE) as u64;
            rx_descs[i].len = PKT_BUF_SIZE as u32;
            rx_descs[i].flags = 1 << 31;
        }

        dev.write_reg(VMXNET3_REG_DSAL, dma_phys as u32);
        dev.write_reg(VMXNET3_REG_DSAH, (dma_phys >> 32) as u32);

        dev.write_reg(VMXNET3_REG_CMD, VMXNET3_CMD_ACTIVATE);
        let status = dev.read_reg(VMXNET3_REG_CMD);
        if status != 0 {
            return Err("vmxnet3: activation failed");
        }

        dev.write_reg(VMXNET3_REG_RXPROD, RING_SIZE as u32);
        Ok(dev)
    }
}

impl NetDevice for Vmxnet3Device {
    fn mac_address(&self) -> [u8; 6] {
        self.mac
    }

    fn send_packet(&mut self, packet: &[u8]) -> Result<(), VirtioError> {
        if packet.len() > PKT_BUF_SIZE {
            return Err(VirtioError::IoError);
        }

        let tx_descs = unsafe {
            core::slice::from_raw_parts_mut((self.dma_virt as usize + 0x200) as *mut Vmxnet3TxDesc, RING_SIZE)
        };

        let idx = self.tx_prod % RING_SIZE;
        let tx_buf_phys = self.dma_frame.start_address().as_u64() + 0x1000 + (RING_SIZE * PKT_BUF_SIZE) as u64;
        let tx_buf_virt = (hhdm_offset() + tx_buf_phys) as *mut u8;

        unsafe {
            core::ptr::copy_nonoverlapping(packet.as_ptr(), tx_buf_virt, packet.len());
        }

        tx_descs[idx].addr = tx_buf_phys;
        tx_descs[idx].len = packet.len() as u32;
        tx_descs[idx].flags = (1 << 12) | (1 << 13) | (1 << 14);

        self.tx_prod = self.tx_prod.wrapping_add(1);
        self.write_reg(VMXNET3_REG_TXPROD, self.tx_prod as u32);

        Ok(())
    }

    fn receive_packet(&mut self, buf: &mut [u8]) -> Result<Option<usize>, VirtioError> {
        let rx_comp = unsafe {
            core::slice::from_raw_parts((self.dma_virt as usize + 0xE00) as *const Vmxnet3CompDesc, RING_SIZE)
        };

        let idx = self.rx_cons % RING_SIZE;
        let comp = &rx_comp[idx];

        if (comp.flags & (1 << 31)) == 0 {
            return Ok(None);
        }

        let len = (comp.len & 0x3FFF) as usize;
        if len > 0 && len <= buf.len() {
            let rx_buf_phys = self.dma_frame.start_address().as_u64() + 0x1000 + (idx * PKT_BUF_SIZE) as u64;
            let rx_buf_virt = (hhdm_offset() + rx_buf_phys) as *const u8;

            unsafe {
                core::ptr::copy_nonoverlapping(rx_buf_virt, buf.as_mut_ptr(), len);
            }

            self.rx_cons = self.rx_cons.wrapping_add(1);
            self.write_reg(VMXNET3_REG_RXPROD, self.rx_cons as u32);
            Ok(Some(len))
        } else {
            Ok(None)
        }
    }
}

impl Drop for Vmxnet3Device {
    fn drop(&mut self) {
        self.write_reg(VMXNET3_REG_CMD, VMXNET3_CMD_QUIESCE);
        self.write_reg(VMXNET3_REG_CMD, VMXNET3_CMD_RESET);
        deallocate_frame(self.dma_frame);
    }
}
