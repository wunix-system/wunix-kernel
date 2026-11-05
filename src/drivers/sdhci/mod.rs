pub mod regs;

use alloc::sync::Arc;
use spin::Mutex;
use crate::drivers::manager::DeviceManager;
use crate::drivers::pci::PciDevice;
use crate::drivers::traits::{BlockDevice, BlockDeviceError};

use regs::*;

pub struct SdhciDevice {
    base: u64,
    sector_count: u64,
    pub _card_rca: u16,
}

unsafe impl Send for SdhciDevice {}
unsafe impl Sync for SdhciDevice {}

impl SdhciDevice {
    #[inline]
    fn read8(&self, offset: usize) -> u8 {
        unsafe { core::ptr::read_volatile((self.base + offset as u64) as *const u8) }
    }

    #[inline]
    fn write8(&self, offset: usize, val: u8) {
        unsafe { core::ptr::write_volatile((self.base + offset as u64) as *mut u8, val) }
    }

    #[inline]
    fn read16(&self, offset: usize) -> u16 {
        unsafe { core::ptr::read_volatile((self.base + offset as u64) as *const u16) }
    }

    #[inline]
    fn write16(&self, offset: usize, val: u16) {
        unsafe { core::ptr::write_volatile((self.base + offset as u64) as *mut u16, val) }
    }

    #[inline]
    fn read32(&self, offset: usize) -> u32 {
        unsafe { core::ptr::read_volatile((self.base + offset as u64) as *const u32) }
    }

    #[inline]
    fn write32(&self, offset: usize, val: u32) {
        unsafe { core::ptr::write_volatile((self.base + offset as u64) as *mut u32, val) }
    }

    pub fn new(mmio_base_virt: u64) -> Result<Self, &'static str> {
        let mut dev = Self {
            base: mmio_base_virt,
            sector_count: 64 * 1024 * 1024 / 512, // 64 МБ по умолчанию
            _card_rca: 0,
        };

        dev.init_controller()?;
        Ok(dev)
    }

    fn init_controller(&mut self) -> Result<(), &'static str> {
        // Программный сброс всех блоков SDHCI
        self.write8(SDHCI_SOFTWARE_RESET, SDHCI_RESET_ALL);
        for _ in 0..10_000 {
            if (self.read8(SDHCI_SOFTWARE_RESET) & SDHCI_RESET_ALL) == 0 {
                break;
            }
        }

        // Включаем питание шины (3.3V)
        self.write8(SDHCI_POWER_CONTROL, 0x0E | 0x01); // 3.3V + SD Bus Power On

        // Включаем внутренний генератор тактовой частоты и тактирование шины SD (Clock Enable)
        self.write16(SDHCI_CLOCK_CONTROL, 0x0001); // Internal Clock Enable
        for _ in 0..10_000 {
            if (self.read16(SDHCI_CLOCK_CONTROL) & 0x0002) != 0 {
                break; // Internal Clock Stable
            }
        }
        self.write16(SDHCI_CLOCK_CONTROL, 0x0007); // SD Clock Enable

        // Разрешаем прерывания
        self.write32(SDHCI_INT_ENABLE, 0xFFFF_FFFF);
        self.write32(SDHCI_SIGNAL_ENABLE, 0xFFFF_FFFF);

        crate::klog!("sdhci: controller initialized (PCI Class 08:05 SD/eMMC)");
        Ok(())
    }

    fn send_command(&mut self, cmd: u16, arg: u32) -> Result<u32, BlockDeviceError> {
        // Ждем готовности линии команд
        for _ in 0..100_000 {
            if (self.read32(SDHCI_PRESENT_STATE) & SDHCI_CMD_INHIBIT) == 0 {
                break;
            }
        }

        self.write32(SDHCI_ARGUMENT, arg);
        self.write16(SDHCI_COMMAND, cmd);

        for _ in 0..100_000 {
            let int_stat = self.read32(SDHCI_INT_STATUS);
            if (int_stat & SDHCI_INT_CMD_COMPLETE) != 0 {
                self.write32(SDHCI_INT_STATUS, SDHCI_INT_CMD_COMPLETE);
                return Ok(self.read32(SDHCI_RESPONSE_0));
            }
        }

        Err(BlockDeviceError::IoError)
    }
}

impl BlockDevice for SdhciDevice {
    fn driver_name(&self) -> &'static str {
        "sdhci-mmc"
    }

    fn read_sectors(&mut self, start_sector: u64, buf: &mut [u8]) -> Result<(), BlockDeviceError> {
        let count = buf.len() / 512;
        if count == 0 {
            return Ok(());
        }

        for s in 0..count {
            self.write16(SDHCI_BLOCK_SIZE, 512);
            self.write16(SDHCI_BLOCK_COUNT, 1);
            self.write16(SDHCI_TRANSFER_MODE, 0x0010); // Read, Single block

            // CMD17: Read Single Block
            self.send_command(0x113A, (start_sector + s as u64) as u32)?;

            // Ждем готовности буфера данных
            let mut ready = false;
            for _ in 0..100_000 {
                if (self.read32(SDHCI_INT_STATUS) & SDHCI_INT_BUFFER_READ_READY) != 0 {
                    self.write32(SDHCI_INT_STATUS, SDHCI_INT_BUFFER_READ_READY);
                    ready = true;
                    break;
                }
            }
            if !ready {
                return Err(BlockDeviceError::IoError);
            }

            let offset = s * 512;
            for i in 0..128 {
                let dword = self.read32(SDHCI_BUFFER_DATA);
                let bytes = dword.to_le_bytes();
                buf[offset + i * 4..offset + i * 4 + 4].copy_from_slice(&bytes);
            }
        }

        Ok(())
    }

    fn write_sectors(&mut self, start_sector: u64, buf: &[u8]) -> Result<(), BlockDeviceError> {
        let count = buf.len() / 512;
        if count == 0 {
            return Ok(());
        }

        for s in 0..count {
            self.write16(SDHCI_BLOCK_SIZE, 512);
            self.write16(SDHCI_BLOCK_COUNT, 1);
            self.write16(SDHCI_TRANSFER_MODE, 0x0000); // Write, Single block

            // CMD24: Write Single Block
            self.send_command(0x183A, (start_sector + s as u64) as u32)?;

            let offset = s * 512;
            for i in 0..128 {
                let dword = u32::from_le_bytes([
                    buf[offset + i * 4],
                    buf[offset + i * 4 + 1],
                    buf[offset + i * 4 + 2],
                    buf[offset + i * 4 + 3],
                ]);
                self.write32(SDHCI_BUFFER_DATA, dword);
            }
        }

        Ok(())
    }

    fn sector_size(&self) -> usize { 512 }
    fn sector_count(&self) -> u64 { self.sector_count }
}

pub fn probe(pci: &PciDevice, dm: &mut DeviceManager) -> Result<(), &'static str> {
    let bar0 = pci.bars[0].ok_or("sdhci: missing BAR0 MMIO")?;
    let mmio = crate::memory::map_kernel_mmio(bar0.address, bar0.size as usize)
        .map_err(|_| "sdhci: failed to map BAR0 MMIO")?;

    let dev = SdhciDevice::new(mmio.as_u64())?;
    dm.register_block_device(alloc::string::String::from("mmcblk0"), Arc::new(Mutex::new(dev)));
    crate::klog!("sdhci: registered block device /dev/mmcblk0 (SD/eMMC)");
    Ok(())
}
