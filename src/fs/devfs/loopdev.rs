use alloc::sync::Arc;
use spin::Mutex;

use crate::drivers::{BlockDevice, BlockDeviceError};
use crate::fs::vfs::Inode;

pub const LOOP_SET_FD: u32 = 0x4C00;
pub const LOOP_CLR_FD: u32 = 0x4C01;
pub const LOOP_GET_STATUS64: u32 = 0x4C05;

/// Виртуальное блочное устройство Loop (/dev/loopX)
pub struct LoopDevice {
    pub index: u32,
    pub backing_file: Option<Arc<dyn Inode>>,
    pub sector_count: u64,
}

impl LoopDevice {
    pub fn new(index: u32) -> Self {
        Self {
            index,
            backing_file: None,
            sector_count: 0,
        }
    }

    pub fn attach_inode(&mut self, inode: Arc<dyn Inode>) {
        let meta = inode.metadata();
        self.sector_count = (meta.size as u64) / 512;
        self.backing_file = Some(inode);
    }

    pub fn detach(&mut self) {
        self.backing_file = None;
        self.sector_count = 0;
    }
}

impl BlockDevice for LoopDevice {
    fn read_sectors(&mut self, start_sector: u64, buf: &mut [u8]) -> Result<(), BlockDeviceError> {
        let inode = self.backing_file.as_ref().ok_or(BlockDeviceError::NotReady)?;
        let offset = (start_sector * 512) as usize;
        inode.read_at(offset, buf).map_err(|_| BlockDeviceError::IoError)?;
        Ok(())
    }

    fn write_sectors(&mut self, start_sector: u64, buf: &[u8]) -> Result<(), BlockDeviceError> {
        let inode = self.backing_file.as_ref().ok_or(BlockDeviceError::NotReady)?;
        let offset = (start_sector * 512) as usize;
        inode.write_at(offset, buf).map_err(|_| BlockDeviceError::IoError)?;
        Ok(())
    }

    fn sector_size(&self) -> usize {
        512
    }

    fn sector_count(&self) -> u64 {
        self.sector_count
    }
}

pub fn create_loop_device(index: u32) -> Arc<Mutex<LoopDevice>> {
    Arc::new(Mutex::new(LoopDevice::new(index)))
}
