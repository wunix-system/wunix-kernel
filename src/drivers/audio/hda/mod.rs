pub mod bdl;
pub mod regs;

use alloc::sync::Arc;
use spin::Mutex;

use crate::drivers::dma::Dma;
use crate::drivers::manager::DeviceManager;
use crate::drivers::pci::PciDevice;
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
    corb: Dma<[u32; 256]>,
    rirb: Dma<[u64; 256]>,
    corb_wp: u8,
    rirb_rp: u8,
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

    fn read_reg16(&self, offset: usize) -> u16 {
        unsafe { core::ptr::read_volatile(self.mmio_base.add(offset) as *const u16) }
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
        let out_stream_offset = stream_offset(in_streams);

        let mut dev = Self {
            mmio_base,
            out_stream_offset,
            bdl: Dma::zeroed()?,
            pcm_buffer: Dma::zeroed()?,
            corb: Dma::zeroed()?,
            rirb: Dma::zeroed()?,
            corb_wp: 0,
            rirb_rp: 0,
            volume: 80,
        };

        dev.init_hw()?;
        Ok(dev)
    }

    fn send_verb(&mut self, nid: u8, verb: u32, payload: u32) -> u32 {
        let cmd = ((nid as u32) << 20) | ((verb & 0xFFF) << 8) | (payload & 0xFF);
        self.corb_wp = self.corb_wp.wrapping_add(1);
        let wp = self.corb_wp as usize;
        self.corb[wp] = cmd;
        self.write_reg16(REG_CORBWP, wp as u16);

        // Ожидаем ответа по RIRB
        for _ in 0..10_000 {
            let rirb_wp = (self.read_reg16(REG_RIRBWP) & 0xFF) as u8;
            if rirb_wp != self.rirb_rp {
                self.rirb_rp = self.rirb_rp.wrapping_add(1);
                let resp = self.rirb[self.rirb_rp as usize];
                return resp as u32;
            }
            core::hint::spin_loop();
        }
        0
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

        // 1. Настройка кольца команд CORB
        self.write_reg8(REG_CORBCTL, 0);
        self.write_reg8(REG_CORBSIZE, 2); // 256 записей
        let corb_phys = self.corb.physical() as u64;
        self.write_reg32(REG_CORBLBASE, corb_phys as u32);
        self.write_reg32(REG_CORBUBASE, (corb_phys >> 32) as u32);
        self.write_reg16(REG_CORBRP, 1 << 15);
        self.write_reg16(REG_CORBRP, 0);
        self.write_reg16(REG_CORBWP, 0);
        self.write_reg8(REG_CORBCTL, 2); // Запуск DMA CORB

        // 2. Настройка кольца ответов RIRB
        self.write_reg8(REG_RIRBCTL, 0);
        self.write_reg8(REG_RIRBSIZE, 2); // 256 записей
        let rirb_phys = self.rirb.physical() as u64;
        self.write_reg32(REG_RIRBLBASE, rirb_phys as u32);
        self.write_reg32(REG_RIRBUBASE, (rirb_phys >> 32) as u32);
        self.write_reg16(REG_RIRBWP, 1 << 15);
        self.write_reg8(REG_RIRBCTL, 2); // Запуск DMA RIRB

        // 3. Инициализация кодека через CORB глаголы (Verbs):
        // Перевод Audio Function Group (Node 1) в активное состояние питания D0
        self.send_verb(1, 0x705, 0x00);

        // Инициализация ЦАП (DAC) и выходных пинов (Node 2..6)
        for nid in 2..=8 {
            self.send_verb(nid, 0x705, 0x00); // Power state D0
            self.send_verb(nid, 0x706, 0x10); // Converter Stream 1, Channel 0
            self.send_verb(nid, 0x707, 0xC0); // Pin Widget Control: HP_ENABLE | OUT_ENABLE
            self.send_verb(nid, 0x390, 0x00); // Unmute output amplifier
            self.send_verb(nid, 0x3B0, 0x00);
        }

        // 4. Настраиваем BDL таблицу под кольцевой PCM буфер
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

pub fn probe_pci(dev: &mut PciDevice, dm: &mut DeviceManager) -> bool {
    if dev.class_code == 0x04 && dev.subclass == 0x03 {
        match IntelHdaDevice::new(dev) {
            Ok(hda) => {
                dev.driver_name = Some("snd_hda");
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
