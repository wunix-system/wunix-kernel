use x86_64::structures::paging::PhysFrame;

use crate::drivers::pci::PciDevice;
use crate::drivers::traits::{BlockDevice, BlockDeviceError};
use crate::memory::{allocate_contiguous_frames, deallocate_frame, hhdm_offset, map_kernel_mmio};

pub const VMWARE_PVSCSI_DEVICE_ID: u16 = 0x07C0;

const PVSCSI_REG_COMMAND: usize = 0x00;
const PVSCSI_REG_DATA: usize = 0x04;
const PVSCSI_REG_REQ_RING_NUM_PAGES: usize = 0x200;
const PVSCSI_REG_CMP_RING_NUM_PAGES: usize = 0x204;
const PVSCSI_REG_KICK_NON_RW_IO: usize = 0x208;
const PVSCSI_REG_KICK_RW_IO: usize = 0x20C;

const PVSCSI_CMD_ADAPTER_RESET: u32 = 1;
const PVSCSI_CMD_SETUP_RINGS: u32 = 3;

const SCSI_READ_10: u8 = 0x28;
const SCSI_WRITE_10: u8 = 0x2A;
const SCSI_READ_CAPACITY_10: u8 = 0x25;

const PVSCSI_FLAG_CMD_DIR_TO_DEV: u8 = 1 << 0;
const PVSCSI_FLAG_CMD_DIR_FROM_DEV: u8 = 1 << 1;

#[repr(C)]
struct PvscsiRingsState {
    req_prod_idx: u32,
    req_cons_idx: u32,
    req_num_entries_log2: u32,
    cmp_prod_idx: u32,
    cmp_cons_idx: u32,
    cmp_num_entries_log2: u32,
    pad: [u8; 104],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct PvscsiRingReqDesc {
    context: u64,
    data_addr: u64,
    data_len: u64,
    sense_addr: u64,
    sense_len: u32,
    cdb_len: u8,
    cdb: [u8; 16],
    flags: u8,
    tag: u8,
    bus: u8,
    target: u8,
    lun: [u8; 8],
    pad: [u8; 26],
}

/// Паравиртуальный SCSI дисковый контроллер VMware PVSCSI.
pub struct PvscsiDevice {
    mmio_base: *mut u8,
    dma_frame: PhysFrame,
    dma_virt: *mut u8,
    sector_count: u64,
    sector_size: usize,
    req_idx: u32,
}

unsafe impl Send for PvscsiDevice {}
unsafe impl Sync for PvscsiDevice {}

impl PvscsiDevice {
    fn write_reg(&self, offset: usize, val: u32) {
        unsafe {
            core::ptr::write_volatile(self.mmio_base.add(offset) as *mut u32, val);
        }
    }

    fn write_cmd(&self, cmd: u32, args: &[u32]) {
        self.write_reg(PVSCSI_REG_COMMAND, cmd);
        for &arg in args {
            self.write_reg(PVSCSI_REG_DATA, arg);
        }
    }

    pub fn new(pci: &PciDevice) -> Result<Self, &'static str> {
        pci.enable_memory_space();
        pci.enable_bus_mastering();

        let bar0 = pci.bar(0).ok_or("PVSCSI: BAR0 MMIO not found")?;
        let mmio_virt = map_kernel_mmio(bar0.address, bar0.size as usize)
            .map_err(|_| "PVSCSI: failed to map MMIO registers")?;
        let mmio_base = mmio_virt.as_mut_ptr::<u8>();

        let dma_frame = allocate_contiguous_frames(4)
            .ok_or("PVSCSI: failed to allocate DMA frames")?;
        let dma_phys = dma_frame.start_address().as_u64();
        let dma_virt = (hhdm_offset() + dma_phys) as *mut u8;

        unsafe {
            core::ptr::write_bytes(dma_virt, 0, 4 * 4096);
        }

        let mut dev = Self {
            mmio_base,
            dma_frame,
            dma_virt,
            sector_count: 0,
            sector_size: 512,
            req_idx: 0,
        };

        dev.write_cmd(PVSCSI_CMD_ADAPTER_RESET, &[]);

        let rings_state = unsafe { &mut *(dma_virt as *mut PvscsiRingsState) };
        rings_state.req_num_entries_log2 = 5;
        rings_state.cmp_num_entries_log2 = 5;

        dev.write_reg(PVSCSI_REG_REQ_RING_NUM_PAGES, 1);
        dev.write_reg(PVSCSI_REG_CMP_RING_NUM_PAGES, 1);

        let req_ring_phys = dma_phys + 0x1000;
        let cmp_ring_phys = dma_phys + 0x2000;

        dev.write_cmd(
            PVSCSI_CMD_SETUP_RINGS,
            &[
                1,
                1,
                (dma_phys & 0xFFFF_FFFF) as u32,
                (dma_phys >> 32) as u32,
                (req_ring_phys & 0xFFFF_FFFF) as u32,
                (req_ring_phys >> 32) as u32,
                (cmp_ring_phys & 0xFFFF_FFFF) as u32,
                (cmp_ring_phys >> 32) as u32,
            ],
        );

        let _ = dev.read_capacity();
        Ok(dev)
    }

    fn read_capacity(&mut self) -> Result<(), BlockDeviceError> {
        let mut buf = [0u8; 8];
        let mut cdb = [0u8; 16];
        cdb[0] = SCSI_READ_CAPACITY_10;

        self.execute_scsi_cmd(&cdb, 10, &mut buf, PVSCSI_FLAG_CMD_DIR_FROM_DEV)?;

        let max_lba = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as u64;
        let block_len = u32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]) as usize;

        if block_len == 512 || block_len == 4096 {
            self.sector_size = block_len;
            self.sector_count = max_lba + 1;
        } else {
            self.sector_size = 512;
            self.sector_count = 1024 * 1024;
        }

        Ok(())
    }

    fn execute_scsi_cmd(&mut self, cdb: &[u8; 16], cdb_len: u8, data: &mut [u8], flags: u8) -> Result<(), BlockDeviceError> {
        let req_ring = unsafe {
            core::slice::from_raw_parts_mut((self.dma_virt as usize + 0x1000) as *mut PvscsiRingReqDesc, 32)
        };

        let idx = (self.req_idx % 32) as usize;
        let data_phys = self.dma_frame.start_address().as_u64() + 0x3000;
        let data_virt = (hhdm_offset() + data_phys) as *mut u8;

        if (flags & PVSCSI_FLAG_CMD_DIR_TO_DEV) != 0 && !data.is_empty() {
            unsafe {
                core::ptr::copy_nonoverlapping(data.as_ptr(), data_virt, data.len().min(4096));
            }
        }

        let desc = &mut req_ring[idx];
        desc.context = self.req_idx as u64;
        desc.data_addr = data_phys;
        desc.data_len = data.len() as u64;
        desc.cdb_len = cdb_len;
        desc.cdb = *cdb;
        desc.flags = flags;
        desc.bus = 0;
        desc.target = 0;

        let rings_state = unsafe { &mut *(self.dma_virt as *mut PvscsiRingsState) };
        self.req_idx = self.req_idx.wrapping_add(1);
        rings_state.req_prod_idx = self.req_idx;

        if (flags & (PVSCSI_FLAG_CMD_DIR_TO_DEV | PVSCSI_FLAG_CMD_DIR_FROM_DEV)) != 0 {
            self.write_reg(PVSCSI_REG_KICK_RW_IO, 0);
        } else {
            self.write_reg(PVSCSI_REG_KICK_NON_RW_IO, 0);
        }

        if (flags & PVSCSI_FLAG_CMD_DIR_FROM_DEV) != 0 && !data.is_empty() {
            unsafe {
                core::ptr::copy_nonoverlapping(data_virt, data.as_mut_ptr(), data.len().min(4096));
            }
        }

        Ok(())
    }
}

impl BlockDevice for PvscsiDevice {
    fn read_sectors(&mut self, start_sector: u64, buf: &mut [u8]) -> Result<(), BlockDeviceError> {
        let num_sectors = (buf.len() / self.sector_size) as u32;
        let mut offset = 0;

        for s in 0..num_sectors {
            let sector_lba = (start_sector + s as u64) as u32;
            let mut cdb = [0u8; 16];
            cdb[0] = SCSI_READ_10;
            cdb[2] = ((sector_lba >> 24) & 0xFF) as u8;
            cdb[3] = ((sector_lba >> 16) & 0xFF) as u8;
            cdb[4] = ((sector_lba >> 8) & 0xFF) as u8;
            cdb[5] = (sector_lba & 0xFF) as u8;
            cdb[8] = 1;

            self.execute_scsi_cmd(
                &cdb,
                10,
                &mut buf[offset..offset + self.sector_size],
                PVSCSI_FLAG_CMD_DIR_FROM_DEV,
            )?;
            offset += self.sector_size;
        }

        Ok(())
    }

    fn write_sectors(&mut self, start_sector: u64, buf: &[u8]) -> Result<(), BlockDeviceError> {
        let num_sectors = (buf.len() / self.sector_size) as u32;
        let mut offset = 0;

        for s in 0..num_sectors {
            let sector_lba = (start_sector + s as u64) as u32;
            let mut cdb = [0u8; 16];
            cdb[0] = SCSI_WRITE_10;
            cdb[2] = ((sector_lba >> 24) & 0xFF) as u8;
            cdb[3] = ((sector_lba >> 16) & 0xFF) as u8;
            cdb[4] = ((sector_lba >> 8) & 0xFF) as u8;
            cdb[5] = (sector_lba & 0xFF) as u8;
            cdb[8] = 1;

            let mut sec_buf = [0u8; 512];
            sec_buf.copy_from_slice(&buf[offset..offset + self.sector_size]);

            self.execute_scsi_cmd(
                &cdb,
                10,
                &mut sec_buf,
                PVSCSI_FLAG_CMD_DIR_TO_DEV,
            )?;
            offset += self.sector_size;
        }

        Ok(())
    }

    fn sector_size(&self) -> usize {
        self.sector_size
    }

    fn sector_count(&self) -> u64 {
        self.sector_count
    }
}

impl Drop for PvscsiDevice {
    fn drop(&mut self) {
        deallocate_frame(self.dma_frame);
    }
}
