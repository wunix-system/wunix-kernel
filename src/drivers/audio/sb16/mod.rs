use x86_64::instructions::port::{PortReadOnly, PortWriteOnly};

use crate::drivers::traits::{AudioDevice, AudioDeviceError};
use crate::serial_println;

const SB16_DEFAULT_BASE: u16 = 0x220;

const REG_MIXER_ADDR: u16 = 0x04;
const REG_MIXER_DATA: u16 = 0x05;
const REG_DSP_RESET: u16 = 0x06;
const REG_DSP_READ: u16 = 0x0A;
const REG_DSP_WRITE: u16 = 0x0C;
const REG_DSP_STATUS: u16 = 0x0E;

/// Драйвер легендарной звуковой карты Sound Blaster 16 (ISA / QEMU эмуляция)
pub struct Sb16Device {
    base: u16,
    version_major: u8,
    version_minor: u8,
    volume: u8,
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

        let mut dev = Self {
            base,
            version_major: 0,
            version_minor: 0,
            volume: 80,
        };

        // Запрашиваем версию DSP (команда 0xE1)
        dev.write_cmd(0xE1);
        dev.version_major = dev.read_data()?;
        dev.version_minor = dev.read_data()?;

        Some(dev)
    }
}

impl AudioDevice for Sb16Device {
    fn device_name(&self) -> &'static str {
        "Sound Blaster 16"
    }

    fn play_pcm(&mut self, _samples: &[u8]) -> Result<(), AudioDeviceError> {
        // Задаем частоту дискретизации 44100 Гц (команда 0x41)
        self.write_cmd(0x41);
        self.write_cmd((44100u16 >> 8) as u8);
        self.write_cmd((44100u16 & 0xFF) as u8);
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
        false
    }
}

/// Инициализация звуковой карты Sound Blaster 16 по стандартному порту 0x220
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
