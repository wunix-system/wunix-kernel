pub mod identify;
pub mod queue;
pub mod regs;

use alloc::sync::Arc;
use spin::Mutex;

use crate::drivers::manager::DeviceManager;
use crate::drivers::pci::PciDevice;
use crate::drivers::dma::Dma;
use crate::drivers::traits::{BlockDevice, BlockDeviceError};
use crate::memory::map_kernel_mmio;
use crate::serial_println;

use identify::IdentifyNamespace;
use queue::{NvmeCmd, NvmeCqe};
use regs::*;

const QUEUE_DEPTH: usize = 32;

/// Драйвер высокоскоростных накопителей NVMe (PCIe M.2 / U.2, NVM Express 1.x..2.x)
pub struct NvmeDevice {
    mmio_base: *mut u8,
    dstrd: usize,
    admin_sq: Dma<[NvmeCmd; QUEUE_DEPTH]>,
    admin_cq: Dma<[NvmeCqe; QUEUE_DEPTH]>,
    admin_sq_tail: u16,
    admin_cq_head: u16,
    admin_cq_phase: bool,
    io_sq: Dma<[NvmeCmd; QUEUE_DEPTH]>,
    io_cq: Dma<[NvmeCqe; QUEUE_DEPTH]>,
    io_sq_tail: u16,
    io_cq_head: u16,
    io_cq_phase: bool,
    dma_bounce: Dma<[u8; 4096]>,
    sector_size: usize,
    sector_count: u64,
}

unsafe impl Send for NvmeDevice {}
unsafe impl Sync for NvmeDevice {}

impl NvmeDevice {
    fn read_reg32(&self, offset: usize) -> u32 {
        unsafe { core::ptr::read_volatile(self.mmio_base.add(offset) as *const u32) }
    }

    fn write_reg32(&self, offset: usize, val: u32) {
        unsafe { core::ptr::write_volatile(self.mmio_base.add(offset) as *mut u32, val) }
    }

    fn write_reg64(&self, offset: usize, val: u64) {
        unsafe { core::ptr::write_volatile(self.mmio_base.add(offset) as *mut u64, val) }
    }

    fn ring_doorbell(&self, qid: u32, is_completion: bool, val: u16) {
        let offset = doorbell_offset(self.dstrd, qid, is_completion);
        self.write_reg32(offset, val as u32);
    }

    /// Инициализирует контроллер NVMe
    pub fn new(pci: &PciDevice) -> Result<Self, &'static str> {
        pci.enable_memory_space();
        pci.enable_bus_mastering();

        let bar0 = pci.bar(0).ok_or("nvme: BAR0 MMIO not found")?;
        let mmio_virt = map_kernel_mmio(bar0.address, bar0.size as usize)
            .map_err(|_| "nvme: failed to map MMIO")?;
        let mmio_base = mmio_virt.as_mut_ptr::<u8>();

        let mut dev = Self {
            mmio_base,
            dstrd: 0,
            admin_sq: Dma::zeroed()?,
            admin_cq: Dma::zeroed()?,
            admin_sq_tail: 0,
            admin_cq_head: 0,
            admin_cq_phase: true,
            io_sq: Dma::zeroed()?,
            io_cq: Dma::zeroed()?,
            io_sq_tail: 0,
            io_cq_head: 0,
            io_cq_phase: true,
            dma_bounce: Dma::zeroed()?,
            sector_size: 512,
            sector_count: 0,
        };

        dev.init_controller()?;
        Ok(dev)
    }

    fn init_controller(&mut self) -> Result<(), &'static str> {
        // Читаем возможности контроллера (CAP)
        let cap = unsafe { core::ptr::read_volatile(self.mmio_base as *const u64) };
        self.dstrd = ((cap >> 32) & 0x0F) as usize;

        // Если контроллер уже запущен, выключаем и ждем сброса CSTS.RDY
        let cc = self.read_reg32(REG_CC);
        if (cc & CC_EN) != 0 {
            self.write_reg32(REG_CC, cc & !CC_EN);
            for _ in 0..100_000 {
                if (self.read_reg32(REG_CSTS) & CSTS_RDY) == 0 {
                    break;
                }
                core::hint::spin_loop();
            }
        }

        // Задаем атрибуты и адреса очередей администратора (Admin Queue)
        let q_entries = (QUEUE_DEPTH as u32) - 1;
        self.write_reg32(REG_AQA, (q_entries << 16) | q_entries);
        self.write_reg64(REG_ASQ, self.admin_sq.physical() as u64);
        self.write_reg64(REG_ACQ, self.admin_cq.physical() as u64);

        // Включаем контроллер в NVM режиме с 4K страницами
        let new_cc = CC_EN | CC_CSS_NVM | CC_MPS_4K | CC_IOSQES_64 | CC_IOCQES_16;
        self.write_reg32(REG_CC, new_cc);

        // Ждем готовности CSTS.RDY == 1
        let mut ready = false;
        for _ in 0..100_000 {
            if (self.read_reg32(REG_CSTS) & CSTS_RDY) != 0 {
                ready = true;
                break;
            }
            core::hint::spin_loop();
        }
        if !ready {
            return Err("nvme: controller failed to become ready");
        }

        // Создаем I/O очереди (CQ qid=1, SQ qid=1)
        self.create_io_queues()?;

        // Опрашиваем первый неймспейс (NSID 1)
        self.identify_namespace(1)?;

        Ok(())
    }

    fn submit_admin_cmd(&mut self, mut cmd: NvmeCmd) -> Result<NvmeCqe, &'static str> {
        let slot = self.admin_sq_tail as usize;
        cmd.cid = self.admin_sq_tail;
        self.admin_sq[slot] = cmd;

        self.admin_sq_tail = ((self.admin_sq_tail + 1) % QUEUE_DEPTH as u16) as u16;
        self.ring_doorbell(0, false, self.admin_sq_tail);

        let head = self.admin_cq_head as usize;
        for _ in 0..100_000 {
            let cqe = self.admin_cq[head];
            if cqe.is_phase_match(self.admin_cq_phase) {
                self.admin_cq_head = ((self.admin_cq_head + 1) % QUEUE_DEPTH as u16) as u16;
                if self.admin_cq_head == 0 {
                    self.admin_cq_phase = !self.admin_cq_phase;
                }
                self.ring_doorbell(0, true, self.admin_cq_head);
                if !cqe.is_success() {
                    return Err("nvme: admin command returned error status");
                }
                return Ok(cqe);
            }
            core::hint::spin_loop();
        }
        Err("nvme: admin command timeout")
    }

    fn create_io_queues(&mut self) -> Result<(), &'static str> {
        // Create I/O Completion Queue (opcode 0x05)
        let mut create_cq = NvmeCmd::default();
        create_cq.opcode = 0x05;
        create_cq.prp1 = self.io_cq.physical() as u64;
        create_cq.cdw10 = ((QUEUE_DEPTH as u32 - 1) << 16) | 1; // QID=1, QSIZE
        create_cq.cdw11 = 1; // Physically contiguous, interrupts disabled
        self.submit_admin_cmd(create_cq)?;

        // Create I/O Submission Queue (opcode 0x01)
        let mut create_sq = NvmeCmd::default();
        create_sq.opcode = 0x01;
        create_sq.prp1 = self.io_sq.physical() as u64;
        create_sq.cdw10 = ((QUEUE_DEPTH as u32 - 1) << 16) | 1; // QID=1, QSIZE
        create_sq.cdw11 = (1 << 16) | 1; // CQID=1, Physically contiguous
        self.submit_admin_cmd(create_sq)?;

        Ok(())
    }

    fn identify_namespace(&mut self, nsid: u32) -> Result<(), &'static str> {
        let mut id_cmd = NvmeCmd::default();
        id_cmd.opcode = 0x06; // Identify
        id_cmd.nsid = nsid;
        id_cmd.prp1 = self.dma_bounce.physical() as u64;
        id_cmd.cdw10 = 0x00; // CNS 0x00 = Identify Namespace
        self.submit_admin_cmd(id_cmd)?;

        let ns = unsafe { &*(self.dma_bounce.as_ptr() as *const IdentifyNamespace) };
        self.sector_count = ns.nsze;
        self.sector_size = ns.sector_size();

        Ok(())
    }

    fn submit_io_cmd(&mut self, mut cmd: NvmeCmd) -> Result<(), BlockDeviceError> {
        let slot = self.io_sq_tail as usize;
        cmd.cid = self.io_sq_tail;
        self.io_sq[slot] = cmd;

        self.io_sq_tail = ((self.io_sq_tail + 1) % QUEUE_DEPTH as u16) as u16;
        self.ring_doorbell(1, false, self.io_sq_tail);

        let head = self.io_cq_head as usize;
        for _ in 0..200_000 {
            let cqe = self.io_cq[head];
            if cqe.is_phase_match(self.io_cq_phase) {
                self.io_cq_head = ((self.io_cq_head + 1) % QUEUE_DEPTH as u16) as u16;
                if self.io_cq_head == 0 {
                    self.io_cq_phase = !self.io_cq_phase;
                }
                self.ring_doorbell(1, true, self.io_cq_head);
                if !cqe.is_success() {
                    return Err(BlockDeviceError::IoError);
                }
                return Ok(());
            }
            core::hint::spin_loop();
        }
        Err(BlockDeviceError::IoError)
    }
}

impl BlockDevice for NvmeDevice {
    fn read_sectors(&mut self, start_sector: u64, buf: &mut [u8]) -> Result<(), BlockDeviceError> {
        let sec_sz = self.sector_size;
        let count = buf.len() / sec_sz;
        if count == 0 {
            return Ok(());
        }

        for (i, chunk) in buf.chunks_mut(sec_sz).enumerate() {
            let lba = start_sector + (i as u64);
            let mut cmd = NvmeCmd::default();
            cmd.opcode = 0x02; // NVM Read
            cmd.nsid = 1;
            cmd.prp1 = self.dma_bounce.physical() as u64;
            cmd.cdw10 = lba as u32;
            cmd.cdw11 = (lba >> 32) as u32;
            cmd.cdw12 = 0; // 0 означает 1 блок (0-based)

            self.submit_io_cmd(cmd)?;
            chunk.copy_from_slice(&self.dma_bounce[..sec_sz]);
        }
        Ok(())
    }

    fn write_sectors(&mut self, start_sector: u64, buf: &[u8]) -> Result<(), BlockDeviceError> {
        let sec_sz = self.sector_size;
        let count = buf.len() / sec_sz;
        if count == 0 {
            return Ok(());
        }

        for (i, chunk) in buf.chunks(sec_sz).enumerate() {
            let lba = start_sector + (i as u64);
            self.dma_bounce[..sec_sz].copy_from_slice(chunk);

            let mut cmd = NvmeCmd::default();
            cmd.opcode = 0x01; // NVM Write
            cmd.nsid = 1;
            cmd.prp1 = self.dma_bounce.physical() as u64;
            cmd.cdw10 = lba as u32;
            cmd.cdw11 = (lba >> 32) as u32;
            cmd.cdw12 = 0;

            self.submit_io_cmd(cmd)?;
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

/// Поиск и регистрация обнаруженных контроллеров NVMe (Class 0x01, Subclass 0x08)
pub fn probe_pci(dev: &PciDevice, dm: &mut DeviceManager) -> bool {
    if dev.class_code == 0x01 && dev.subclass == 0x08 {
        match NvmeDevice::new(dev) {
            Ok(nvme) => {
                let sec_sz = nvme.sector_size();
                let cap_mb = (nvme.sector_count() * sec_sz as u64) / (1024 * 1024);
                let name = alloc::string::String::from("nvme0n1");
                serial_println!(
                    "nvme: registered /dev/{} ({} MB, sector size {} bytes)",
                    name, cap_mb, sec_sz
                );
                dm.register_block_device(name, Arc::new(Mutex::new(nvme)));
                return true;
            }
            Err(e) => {
                serial_println!("nvme: initialization failed for {}: {}", dev.address, e);
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_nvme_struct_sizes() {
        assert_eq!(core::mem::size_of::<queue::NvmeCmd>(), 64);
        assert_eq!(core::mem::size_of::<queue::NvmeCqe>(), 16);
        assert_eq!(core::mem::size_of::<identify::IdentifyNamespace>(), 4096);
    }

    #[test_case]
    fn test_doorbell_calculation() {
        // dstrd = 0 (stride = 4 байта):
        // Admin SQ (qid 0, false) -> 0x1000 + (0 * 4) = 0x1000
        // Admin CQ (qid 0, true)  -> 0x1000 + (1 * 4) = 0x1004
        // I/O SQ 1 (qid 1, false) -> 0x1000 + (2 * 4) = 0x1008
        // I/O CQ 1 (qid 1, true)  -> 0x1000 + (3 * 4) = 0x100C
        assert_eq!(doorbell_offset(0, 0, false), 0x1000);
        assert_eq!(doorbell_offset(0, 0, true), 0x1004);
        assert_eq!(doorbell_offset(0, 1, false), 0x1008);
        assert_eq!(doorbell_offset(0, 1, true), 0x100C);
    }
}

