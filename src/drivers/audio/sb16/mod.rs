use x86_64::instructions::port::{PortReadOnly, PortWriteOnly};

use crate::drivers::dma::Dma;
use crate::drivers::traits::{AudioDevice, AudioDeviceError};
use crate::serial_println;

const SB16_DEFAULT_BASE: u16 = 0x220;

const REG_MIXER_ADDR: u16 = 0x04;
const REG_MIXER_DATA: u16 = 0x05;
const REG_DSP_RESET: u16 = 0x06;
const REG_DSP_READ: u16 = 0x0A;
const REG_DSP_WRITE: u16 = 0x0C;
const REG_DSP_STATUS: u16 = 0x0E;

// Регистры контроллера DMA Intel 8237A (Master DMA для 16-битных каналов 4-7)
const DMA2_MASK_REG: u16 = 0xD4;
const DMA2_MODE_REG: u16 = 0xD6;
const DMA2_CLEAR_FF_REG: u16 = 0xD8;
const DMA2_CH5_ADDR_REG: u16 = 0xC4;
const DMA2_CH5_COUNT_REG: u16 = 0xC6;
const DMA_CH5_PAGE_REG: u16 = 0x8B;

/// Драйвер легендарной звуковой карты Sound Blaster 16 с поддержкой ISA DMA
pub struct Sb16Device {
    base: u16,
    version_major: u8,
    version_minor: u8,
    volume: u8,
    dma_buf: Dma<[u8; 4096]>,
}

unsafe impl Send for Sb16Device {}
unsafe impl Sync for Sb16Device {}

impl Sb16Device {
    fn write_cmd(&self, cmd: u8) {
        let mut port_status = PortReadOnly::<u8>::new(self.base + REG_DSP_WRITE);
        for _ in 0..10_000 {
            let status = unsafe { port_status.read() };
            if (status & 0x80) == 0 {
                break;
            }
            core::hint::spin_loop();
        }
        let mut port_data = PortWriteOnly::<u8>::new(self.base + REG_DSP_WRITE);
        unsafe { port_data.write(cmd) }
    }

    fn read_data(&self) -> Option<u8> {
        let mut port_status = PortReadOnly::<u8>::new(self.base + REG_DSP_STATUS);
        for _ in 0..10_000 {
            let status = unsafe { port_status.read() };
            if (status & 0x80) != 0 {
                let mut port_data = PortReadOnly::<u8>::new(self.base + REG_DSP_READ);
                return Some(unsafe { port_data.read() });
            }
            core::hint::spin_loop();
        }
        None
    }

    pub fn probe(base: u16) -> Option<Self> {
        // Сброс DSP
        let mut reset_port = PortWriteOnly::<u8>::new(base + REG_DSP_RESET);
        unsafe {
            reset_port.write(1);
            for _ in 0..100 { core::hint::spin_loop(); }
            reset_port.write(0);
        }

        // Ожидаем сигнатуру готовности 0xAA от DSP
        let mut read_status = PortReadOnly::<u8>::new(base + REG_DSP_STATUS);
        let mut ready = false;
        for _ in 0..10_000 {
            let s = unsafe { read_status.read() };
            if (s & 0x80) != 0 {
                let mut read_data = PortReadOnly::<u8>::new(base + REG_DSP_READ);
                if unsafe { read_data.read() } == 0xAA {
                    ready = true;
                    break;
                }
            }
            core::hint::spin_loop();
        }

        if !ready {
            return None;
        }

        let dma_buf: Dma<[u8; 4096]> = Dma::zeroed().ok()?;
        if dma_buf.physical() >= 0x0100_0000 {
            serial_println!(
                "sb16: DMA buffer at {:#x} exceeds 16MB ISA limit (Zone DMA required), disabling SB16",
                dma_buf.physical()
            );
            return None;
        }

        let mut dev = Self {
            base,
            version_major: 0,
            version_minor: 0,
            volume: 80,
            dma_buf,
        };

        // Запрашиваем версию DSP (команда 0xE1)
        dev.write_cmd(0xE1);
        dev.version_major = dev.read_data()?;
        dev.version_minor = dev.read_data()?;

        Some(dev)
    }

    /// Программирует контроллер Intel 8237A DMA для канала 5 (16-битный вывод)
    fn setup_dma5(&self, phys_addr: u32, word_count: u16) {
        unsafe {
            let mut mask = PortWriteOnly::<u8>::new(DMA2_MASK_REG);
            let mut clear_ff = PortWriteOnly::<u8>::new(DMA2_CLEAR_FF_REG);
            let mut mode = PortWriteOnly::<u8>::new(DMA2_MODE_REG);
            let mut addr = PortWriteOnly::<u8>::new(DMA2_CH5_ADDR_REG);
            let mut page = PortWriteOnly::<u8>::new(DMA_CH5_PAGE_REG);
            let mut count = PortWriteOnly::<u8>::new(DMA2_CH5_COUNT_REG);

            // Маскируем канал 5
            mask.write(5);
            clear_ff.write(0);
            // Single cycle mode, read transfer, channel 5 (0x48 | 1)
            mode.write(0x49);

            // Адрес в 16-битных словах
            let word_addr = (phys_addr >> 1) as u16;
            addr.write((word_addr & 0xFF) as u8);
            addr.write(((word_addr >> 8) & 0xFF) as u8);

            // Страничный регистр (биты 16..23 физического адреса)
            page.write(((phys_addr >> 16) & 0xFF) as u8);

            // Количество 16-битных слов минус 1
            let words_minus_one = word_count.saturating_sub(1);
            count.write((words_minus_one & 0xFF) as u8);
            count.write(((words_minus_one >> 8) & 0xFF) as u8);

            // Разрешаем канал 5
            mask.write(1);
        }
    }
}

impl AudioDevice for Sb16Device {
    fn device_name(&self) -> &'static str {
        "Sound Blaster 16"
    }

    fn play_pcm(&mut self, samples: &[u8]) -> Result<(), AudioDeviceError> {
        let len = samples.len().min(4096);
        if len == 0 {
            return Ok(());
        }

        self.dma_buf[..len].copy_from_slice(&samples[..len]);
        let words = (len / 2) as u16;
        let phys = self.dma_buf.physical() as u32;

        self.setup_dma5(phys, words);

        // Устанавливаем частоту 44100 Гц
        self.write_cmd(0x41);
        self.write_cmd((44100u16 >> 8) as u8);
        self.write_cmd((44100u16 & 0xFF) as u8);

        // Команда 0xB0: 16-битный PCM D/A, single-cycle
        self.write_cmd(0xB0);
        self.write_cmd(0x10); // 16-bit signed stereo
        let samples_minus_one = words.saturating_sub(1);
        self.write_cmd((samples_minus_one & 0xFF) as u8);
        self.write_cmd(((samples_minus_one >> 8) & 0xFF) as u8);

        Ok(())
    }

    fn set_volume(&mut self, volume_percent: u8) -> Result<(), AudioDeviceError> {
        self.volume = volume_percent.min(100);
        let mut addr_port = PortWriteOnly::<u8>::new(self.base + REG_MIXER_ADDR);
        let mut data_port = PortWriteOnly::<u8>::new(self.base + REG_MIXER_DATA);

        let vol_val = (self.volume * 31 / 100) << 3;
        unsafe {
            addr_port.write(0x22); // Master Volume
            data_port.write(vol_val);
        }
        Ok(())
    }

    fn is_busy(&self) -> bool {
        // Чтение портов 0x0E (8-бит) и 0x0F (16-бит) квитирует завершение воспроизведения в DSP SB16
        let mut status_16 = PortReadOnly::<u8>::new(self.base + 0x0F);
        let _ = unsafe { status_16.read() };
        let mut status_8 = PortReadOnly::<u8>::new(self.base + REG_DSP_STATUS);
        let _ = unsafe { status_8.read() };
        false
    }
}

pub fn init() -> Option<Sb16Device> {
    if let Some(sb) = Sb16Device::probe(SB16_DEFAULT_BASE) {
        serial_println!(
            "sb16: registered Sound Blaster 16 at port 0x{:03x} (DSP v{}.{})",
            sb.base, sb.version_major, sb.version_minor
        );
        return Some(sb);
    }
    None
}
