pub mod bdl;
pub mod regs;

use alloc::sync::Arc;
use spin::Mutex;

use crate::drivers::manager::DeviceManager;
use crate::drivers::pci::PciDevice;
use crate::drivers::redox_compat::Dma;
use crate::drivers::traits::{AudioDevice, AudioDeviceError};
use crate::memory::map_kernel_mmio;
use crate::serial_println;

use bdl::BdlEntry;
use regs::*;

const PCM_BUFFER_SIZE: usize = 16384;

/// Драйвер звукового контроллера Intel High Definition Audio (Intel HDA)
pub struct IntelHdaDevice {
    mmio_base: *mut u8,
    out_stream_offset: usize,
    bdl: Dma<[BdlEntry; 2]>,
    pcm_buffer: Dma<[u8; PCM_BUFFER_SIZE]>,
    volume: u8,
}

unsafe impl Send for IntelHdaDevice {}
unsafe impl Sync for IntelHdaDevice {}

impl IntelHdaDevice {
    fn read_reg32(&self, offset: usize) -> u32 {
        unsafe { core::ptr::read_volatile(self.mmio_base.add(offset) as *const u32) }
    }

    fn write_reg32(&self, offset: usize, val: u32) {
        unsafe { core::ptr::write_volatile(self.mmio_base.add(offset) as *mut u32, val) }
    }

    fn write_reg16(&self, offset: usize, val: u16) {
        unsafe { core::ptr::write_volatile(self.mmio_base.add(offset) as *mut u16, val) }
    }

    fn write_reg8(&self, offset: usize, val: u8) {
        unsafe { core::ptr::write_volatile(self.mmio_base.add(offset), val) }
    }

    pub fn new(pci: &PciDevice) -> Result<Self, &'static str> {
        pci.enable_memory_space();
        pci.enable_bus_mastering();

        let bar0 = pci.bar(0).ok_or("hda: BAR0 not found")?;
        let mmio_virt = map_kernel_mmio(bar0.address, bar0.size as usize)
            .map_err(|_| "hda: failed to map MMIO")?;
        let mmio_base = mmio_virt.as_mut_ptr::<u8>();

        let gcap = unsafe { core::ptr::read_volatile(mmio_base as *const u16) };
        let in_streams = ((gcap >> 8) & 0x0F) as usize;
        // Первый выходной стрим (Output Stream) начинается сразу за входными
        let out_stream_offset = stream_offset(in_streams);

        let mut dev = Self {
            mmio_base,
            out_stream_offset,
            bdl: Dma::zeroed()?,
            pcm_buffer: Dma::zeroed()?,
            volume: 80,
        };

        dev.init_hw()?;
        Ok(dev)
    }

    fn init_hw(&mut self) -> Result<(), &'static str> {
        // Сброс контроллера: снимаем бит CRST
        let gctl = self.read_reg32(REG_GCTL);
        self.write_reg32(REG_GCTL, gctl & !GCTL_CRST);
        for _ in 0..10_000 {
            if (self.read_reg32(REG_GCTL) & GCTL_CRST) == 0 {
                break;
            }
            core::hint::spin_loop();
        }

        // Выводим контроллер из сброса
        self.write_reg32(REG_GCTL, GCTL_CRST);
        for _ in 0..10_000 {
            if (self.read_reg32(REG_GCTL) & GCTL_CRST) != 0 {
                break;
            }
            core::hint::spin_loop();
        }

        // Настраиваем BDL таблицу под кольцевой PCM буфер
        let pcm_phys = self.pcm_buffer.physical() as u64;
        self.bdl[0].addr = pcm_phys;
        self.bdl[0].length = (PCM_BUFFER_SIZE / 2) as u32;
        self.bdl[0].ioc = 1;

        self.bdl[1].addr = pcm_phys + (PCM_BUFFER_SIZE / 2) as u64;
        self.bdl[1].length = (PCM_BUFFER_SIZE / 2) as u32;
        self.bdl[1].ioc = 1;

        let bdl_phys = self.bdl.physical() as u64;
        let s_off = self.out_stream_offset;

        // Останавливаем стрим
        self.write_reg8(s_off + SD_CTL, 0);

        // Задаем адрес BDL таблицы
        self.write_reg32(s_off + SD_BDLPL, bdl_phys as u32);
        self.write_reg32(s_off + SD_BDLPU, (bdl_phys >> 32) as u32);

        // Длина циклического буфера и индекс последнего валидного дескриптора
        self.write_reg32(s_off + SD_CBL, PCM_BUFFER_SIZE as u32);
        self.write_reg16(s_off + SD_LVI, 1);

        // Формат потока: 48 кГц, 16 бит, 2 канала (стерео)
        // Бит 14: base rate 48 кГц (0), биты 6:4: 16-бит (001b), биты 3:0: 2 канала (0001b)
        self.write_reg16(s_off + SD_FMT, (1 << 4) | 1);

        Ok(())
    }
}

impl AudioDevice for IntelHdaDevice {
    fn device_name(&self) -> &'static str {
        "Intel High Definition Audio"
    }

    fn play_pcm(&mut self, samples: &[u8]) -> Result<(), AudioDeviceError> {
        let len = samples.len().min(PCM_BUFFER_SIZE);
        if len == 0 {
            return Ok(());
        }

        self.pcm_buffer[..len].copy_from_slice(&samples[..len]);

        let s_off = self.out_stream_offset;
        // Номер стрима 1 в битах 23:20 + флаги RUN и IOCE
        let ctl = (1 << 20) | SD_CTL_IOCE | SD_CTL_RUN;
        self.write_reg32(s_off + SD_CTL, ctl);

        Ok(())
    }

    fn set_volume(&mut self, volume_percent: u8) -> Result<(), AudioDeviceError> {
        self.volume = volume_percent.min(100);
        Ok(())
    }

    fn is_busy(&self) -> bool {
        let s_off = self.out_stream_offset;
        let ctl = self.read_reg32(s_off + SD_CTL);
        (ctl & SD_CTL_RUN) != 0
    }
}

/// Поиск и регистрация аудиоконтроллеров Intel HDA (Class 0x04, Subclass 0x03)
pub fn probe_pci(dev: &PciDevice, dm: &mut DeviceManager) -> bool {
    if dev.class_code == 0x04 && dev.subclass == 0x03 {
        match IntelHdaDevice::new(dev) {
            Ok(hda) => {
                serial_println!(
                    "hda: registered audio controller (Intel High Definition Audio, {:04x}:{:04x})",
                    dev.vendor_id, dev.device_id
                );
                dm.register_audio_device(Arc::new(Mutex::new(hda)));
                return true;
            }
            Err(e) => {
                serial_println!("hda: init failed for {}: {}", dev.address, e);
            }
        }
    }
    false
}
