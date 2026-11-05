pub mod bd;
pub mod regs;

use alloc::sync::Arc;
use spin::Mutex;
use x86_64::instructions::port::{PortReadOnly, PortWriteOnly};

use crate::drivers::manager::DeviceManager;
use crate::drivers::pci::PciDevice;
use crate::drivers::dma::Dma;
use crate::drivers::traits::{AudioDevice, AudioDeviceError};
use crate::serial_println;

use bd::{Ac97BufferDesc, BD_BUP, BD_IOC};
use regs::*;

const PCM_BUFFER_SIZE: usize = 16384;

/// Драйвер аудиоконтроллера AC'97 (Intel ICH AC'97 Audio)
pub struct Ac97Device {
    mixer_base: u16,
    bm_base: u16,
    bd_list: Dma<[Ac97BufferDesc; 32]>,
    pcm_buffer: Dma<[u8; PCM_BUFFER_SIZE]>,
    volume: u8,
}

unsafe impl Send for Ac97Device {}
unsafe impl Sync for Ac97Device {}

impl Ac97Device {
    fn mixer_write(&self, reg: u16, val: u16) {
        let mut port = PortWriteOnly::<u16>::new(self.mixer_base + reg);
        unsafe { port.write(val) }
    }

    fn bm_read8(&self, reg: u16) -> u8 {
        let mut port = PortReadOnly::<u8>::new(self.bm_base + reg);
        unsafe { port.read() }
    }

    fn bm_read16(&self, reg: u16) -> u16 {
        let mut port = PortReadOnly::<u16>::new(self.bm_base + reg);
        unsafe { port.read() }
    }

    fn bm_write8(&self, reg: u16, val: u8) {
        let mut port = PortWriteOnly::<u8>::new(self.bm_base + reg);
        unsafe { port.write(val) }
    }

    fn bm_write32(&self, reg: u16, val: u32) {
        let mut port = PortWriteOnly::<u32>::new(self.bm_base + reg);
        unsafe { port.write(val) }
    }

    pub fn new(pci: &PciDevice) -> Result<Self, &'static str> {
        pci.enable_io_space();
        pci.enable_bus_mastering();

        let bar0 = pci.bar(0).ok_or("ac97: BAR0 (NAMBAR) not found")?;
        let bar1 = pci.bar(1).ok_or("ac97: BAR1 (NABMBAR) not found")?;

        let mixer_base = bar0.address as u16;
        let bm_base = bar1.address as u16;

        let mut dev = Self {
            mixer_base,
            bm_base,
            bd_list: Dma::zeroed()?,
            pcm_buffer: Dma::zeroed()?,
            volume: 80,
        };

        dev.init_hw()?;
        Ok(dev)
    }

    fn init_hw(&mut self) -> Result<(), &'static str> {
        // Программный сброс микшера AC'97
        self.mixer_write(MIXER_RESET, 0x0001);

        // Сброс каналов воспроизведения Bus Master
        self.bm_write8(BM_PO_CR, PO_CR_RESET);
        for _ in 0..10_000 {
            if (self.bm_read8(BM_PO_CR) & PO_CR_RESET) == 0 {
                break;
            }
            core::hint::spin_loop();
        }

        // Выставляем громкость (0 = максимум громкости в AC'97 шкале затухания)
        self.mixer_write(MIXER_MASTER_VOL, 0x0000);
        self.mixer_write(MIXER_PCM_VOL, 0x0000);

        // Настраиваем дескриптор буфера воспроизведения
        let pcm_phys = self.pcm_buffer.physical() as u32;
        let samples_count = (PCM_BUFFER_SIZE / 2) as u16; // 16-бит сэмплы

        self.bd_list[0].addr = pcm_phys;
        self.bd_list[0].length = samples_count;
        self.bd_list[0].flags = BD_IOC | BD_BUP;

        // Задаем физический адрес BDL в контроллере
        self.bm_write32(BM_PO_BDBAR, self.bd_list.physical() as u32);

        // Задаем индекс последнего буфера LVI = 0
        self.bm_write8(BM_PO_LVI, 0);

        Ok(())
    }
}

impl AudioDevice for Ac97Device {
    fn device_name(&self) -> &'static str {
        "AC'97 Audio Controller"
    }

    fn play_pcm(&mut self, samples: &[u8]) -> Result<(), AudioDeviceError> {
        let len = samples.len().min(PCM_BUFFER_SIZE);
        if len == 0 {
            return Ok(());
        }

        self.pcm_buffer[..len].copy_from_slice(&samples[..len]);
        self.bd_list[0].length = (len / 2) as u16;

        // Запускаем Bus Master воспроизведение
        self.bm_write8(BM_PO_CR, PO_CR_RUN | PO_CR_IOCE);
        Ok(())
    }

    fn set_volume(&mut self, volume_percent: u8) -> Result<(), AudioDeviceError> {
        self.volume = volume_percent.min(100);
        // В AC'97 0x0000 = 0 dB (максимум), 0x1F1F = -46.5 dB (минимум)
        let atten = ((100 - self.volume) * 31 / 100) as u16;
        let val = (atten << 8) | atten;
        self.mixer_write(MIXER_MASTER_VOL, val);
        Ok(())
    }

    fn is_busy(&self) -> bool {
        let sr = self.bm_read16(BM_PO_SR);
        (sr & PO_SR_DCH) == 0
    }
}

/// Поиск и регистрация аудиоконтроллеров AC'97 (Class 0x04, Subclass 0x01)
pub fn probe_pci(dev: &PciDevice, dm: &mut DeviceManager) -> bool {
    if dev.class_code == 0x04 && dev.subclass == 0x01 {
        match Ac97Device::new(dev) {
            Ok(ac97) => {
                serial_println!(
                    "ac97: registered audio controller (AC'97 Audio, {:04x}:{:04x})",
                    dev.vendor_id, dev.device_id
                );
                dm.register_audio_device(Arc::new(Mutex::new(ac97)));
                return true;
            }
            Err(e) => {
                serial_println!("ac97: init failed for {}: {}", dev.address, e);
            }
        }
    }
    false
}
