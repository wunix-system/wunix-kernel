use alloc::boxed::Box;
use x86_64::structures::paging::PhysFrame;

use super::queue::{VirtioBuffer, Virtqueue};
use super::transport::VirtioTransport;
use super::{
    VirtioDevice, VirtioDeviceType, VirtioError, VIRTIO_STATUS_ACKNOWLEDGE, VIRTIO_STATUS_DRIVER,
    VIRTIO_STATUS_DRIVER_OK, VIRTIO_STATUS_FEATURES_OK,
};
use crate::drivers::traits::{BlockDevice, BlockDeviceError};
use crate::memory::{allocate_contiguous_frames, deallocate_frame, hhdm_offset};

const SCSI_READ_10: u8 = 0x28;
const SCSI_WRITE_10: u8 = 0x2A;
const SCSI_READ_CAPACITY_10: u8 = 0x25;

const VIRTIO_SCSI_S_OK: u8 = 0;

/// Запрос команды SCSI по спецификации VirtIO SCSI (virtio_scsi_req_cmd).
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct VirtioScsiReqCmd {
    pub lun: [u8; 8],
    pub id: u64,
    pub task_attr: u8,
    pub prio: u8,
    pub crn: u8,
    pub cdb: [u8; 32],
}

/// Ответ устройства на команду SCSI (virtio_scsi_resp_cmd).
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct VirtioScsiRespCmd {
    pub sense_len: u32,
    pub resid: u32,
    pub status_qualifier: u16,
    pub status: u8,
    pub response: u8,
    pub sense: [u8; 96],
}

/// Драйвер блочного контроллера VirtIO SCSI (QEMU/KVM/Proxmox VE).
pub struct VirtioScsi {
    transport: Box<dyn VirtioTransport>,
    _control_queue: Virtqueue,
    _event_queue: Virtqueue,
    req_queue: Virtqueue,
    sector_count: u64,
    sector_size: usize,
    dma_frame: PhysFrame,
    dma_virt: *mut u8,
    req_id: u64,
}

unsafe impl Send for VirtioScsi {}
unsafe impl Sync for VirtioScsi {}

impl VirtioScsi {
    /// Инициализация очередей и согласование протокола VirtIO SCSI.
    pub fn new(mut transport: Box<dyn VirtioTransport>) -> Result<Self, VirtioError> {
        transport.reset();
        transport.set_status(VIRTIO_STATUS_ACKNOWLEDGE | VIRTIO_STATUS_DRIVER);

        let features = transport.read_device_features();
        transport.write_driver_features(features);
        transport.set_status(
            VIRTIO_STATUS_ACKNOWLEDGE | VIRTIO_STATUS_DRIVER | VIRTIO_STATUS_FEATURES_OK,
        );

        // В VirtIO SCSI строго фиксированные роли первых очередей:
        // Queue 0 — controlq (управление шиной и сброс LUN)
        // Queue 1 — eventq (асинхронные уведомления хоста о подключении дисков)
        // Queue 2 — req_queue (очередь команд ввода-вывода)
        transport.select_queue(0);
        let ctl_sz = transport.get_queue_size();
        let control_queue = Virtqueue::new(0, ctl_sz)?;
        transport.setup_queue(&control_queue);

        transport.select_queue(1);
        let evt_sz = transport.get_queue_size();
        let event_queue = Virtqueue::new(1, evt_sz)?;
        transport.setup_queue(&event_queue);

        transport.select_queue(2);
        let req_sz = transport.get_queue_size();
        if req_sz == 0 {
            return Err(VirtioError::QueueTooSmall);
        }
        let req_queue = Virtqueue::new(2, req_sz)?;
        transport.setup_queue(&req_queue);

        transport.set_status(
            VIRTIO_STATUS_ACKNOWLEDGE
                | VIRTIO_STATUS_DRIVER
                | VIRTIO_STATUS_FEATURES_OK
                | VIRTIO_STATUS_DRIVER_OK,
        );

        let dma_frame = allocate_contiguous_frames(2).ok_or(VirtioError::AllocationFailed)?;
        let dma_phys = dma_frame.start_address().as_u64();
        let dma_virt = (hhdm_offset() + dma_phys) as *mut u8;

        unsafe {
            core::ptr::write_bytes(dma_virt, 0, 2 * 4096);
        }

        let mut scsi = Self {
            transport,
            _control_queue: control_queue,
            _event_queue: event_queue,
            req_queue,
            sector_count: 0,
            sector_size: 512,
            dma_frame,
            dma_virt,
            req_id: 1,
        };

        scsi.query_capacity()?;
        Ok(scsi)
    }

    /// Опрос емкости диска через команду SCSI READ_CAPACITY_10.
    fn query_capacity(&mut self) -> Result<(), VirtioError> {
        let mut cdb = [0u8; 16];
        cdb[0] = SCSI_READ_CAPACITY_10;

        let mut data = [0u8; 8];
        if self.execute_cmd(&cdb, &mut data, false).is_err() {
            // SCSI устройство часто возвращает Unit Attention (0x02, Power-on Reset) при первом запросе.
            // Повторяем запрос для чтения актуальных данных.
            self.execute_cmd(&cdb, &mut data, false)?;
        }

        let max_lba = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as u64;
        let block_len = u32::from_be_bytes([data[4], data[5], data[6], data[7]]) as usize;

        if block_len == 512 || block_len == 4096 {
            self.sector_size = block_len;
            self.sector_count = max_lba + 1;
        } else {
            self.sector_size = 512;
            self.sector_count = 1024 * 1024;
        }

        Ok(())
    }

    /// Отправка SCSI команды в очередь req_queue и синхронное ожидание ответа.
    fn execute_cmd(&mut self, cdb: &[u8], data: &mut [u8], is_write: bool) -> Result<(), VirtioError> {
        let base_phys = self.dma_frame.start_address().as_u64();
        let req_phys = base_phys;
        let resp_phys = base_phys + 0x100;
        let data_phys = base_phys + 0x1000;

        let req_virt = self.dma_virt as *mut VirtioScsiReqCmd;
        let resp_virt = unsafe { self.dma_virt.add(0x100) } as *mut VirtioScsiRespCmd;
        let data_virt = unsafe { self.dma_virt.add(0x1000) };

        // Инициализируем плоскую адресацию LUN (Target 0, LUN 0)
        let mut lun = [0u8; 8];
        lun[0] = 1;

        let mut full_cdb = [0u8; 32];
        let copy_len = cdb.len().min(32);
        full_cdb[..copy_len].copy_from_slice(&cdb[..copy_len]);

        unsafe {
            (*req_virt) = VirtioScsiReqCmd {
                lun,
                id: self.req_id,
                task_attr: 0,
                prio: 0,
                crn: 0,
                cdb: full_cdb,
            };
            (*resp_virt) = VirtioScsiRespCmd {
                sense_len: 0,
                resid: 0,
                status_qualifier: 0,
                status: 0xFF,
                response: 0xFF,
                sense: [0u8; 96],
            };
        }
        self.req_id = self.req_id.wrapping_add(1);

        if is_write && !data.is_empty() {
            unsafe {
                core::ptr::copy_nonoverlapping(data.as_ptr(), data_virt, data.len().min(4096));
            }
        }

        let buffers = if is_write {
            [
                VirtioBuffer { phys_addr: req_phys, len: core::mem::size_of::<VirtioScsiReqCmd>() as u32, is_write: false },
                VirtioBuffer { phys_addr: data_phys, len: data.len().min(4096) as u32, is_write: false },
                VirtioBuffer { phys_addr: resp_phys, len: core::mem::size_of::<VirtioScsiRespCmd>() as u32, is_write: true },
            ]
        } else if !data.is_empty() {
            [
                VirtioBuffer { phys_addr: req_phys, len: core::mem::size_of::<VirtioScsiReqCmd>() as u32, is_write: false },
                VirtioBuffer { phys_addr: resp_phys, len: core::mem::size_of::<VirtioScsiRespCmd>() as u32, is_write: true },
                VirtioBuffer { phys_addr: data_phys, len: data.len().min(4096) as u32, is_write: true },
            ]
        } else {
            [
                VirtioBuffer { phys_addr: req_phys, len: core::mem::size_of::<VirtioScsiReqCmd>() as u32, is_write: false },
                VirtioBuffer { phys_addr: resp_phys, len: core::mem::size_of::<VirtioScsiRespCmd>() as u32, is_write: true },
                VirtioBuffer { phys_addr: 0, len: 0, is_write: true },
            ]
        };

        let valid_buffers = if data.is_empty() { &buffers[0..2] } else { &buffers[0..3] };
        let head = self.req_queue.add_buffer(valid_buffers)?;
        self.transport.notify_queue(2);

        let mut spins = 0;
        while !self.req_queue.can_pop() {
            core::hint::spin_loop();
            spins += 1;
            if spins > 1_000_000 {
                return Err(VirtioError::IoError);
            }
        }

        self.req_queue.pop_used();
        self.req_queue.free_chain(head);

        let (resp_code, scsi_status) = unsafe { ((*resp_virt).response, (*resp_virt).status) };
        if resp_code != VIRTIO_SCSI_S_OK || scsi_status != 0 {
            return Err(VirtioError::DeviceError(resp_code));
        }

        if !is_write && !data.is_empty() {
            unsafe {
                core::ptr::copy_nonoverlapping(data_virt, data.as_mut_ptr(), data.len().min(4096));
            }
        }

        Ok(())
    }
}

impl BlockDevice for VirtioScsi {
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

            self.execute_cmd(&cdb, &mut buf[offset..offset + self.sector_size], false)
                .map_err(BlockDeviceError::from)?;
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

            self.execute_cmd(&cdb, &mut sec_buf, true)
                .map_err(BlockDeviceError::from)?;
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

impl VirtioDevice for VirtioScsi {
    fn name(&self) -> &str {
        "VirtIO SCSI Controller"
    }

    fn device_type(&self) -> VirtioDeviceType {
        VirtioDeviceType::Scsi
    }

    fn reset(&mut self) {
        self.transport.reset();
    }
}

impl Drop for VirtioScsi {
    fn drop(&mut self) {
        deallocate_frame(self.dma_frame);
    }
}
