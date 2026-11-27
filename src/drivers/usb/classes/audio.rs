use alloc::collections::VecDeque;
use alloc::sync::Arc;
use spin::Mutex;

use crate::drivers::manager::DeviceManager;
use crate::drivers::traits::{AudioDevice, AudioDeviceError};
use crate::drivers::usb::host::UsbHostController;
use crate::i18n::t;

pub const USB_CLASS_AUDIO: u8 = 0x01;
pub const AUDIO_SUBCLASS_CONTROL: u8 = 0x01;
pub const AUDIO_SUBCLASS_STREAMING: u8 = 0x02;

const MAX_PCM_BUFFER: usize = 32768;

pub struct UsbAudioDevice {
    slot_id: u8,
    out_ep: u8,
    host: Option<Arc<Mutex<dyn UsbHostController>>>,
    volume: u8,
    sample_rate: u32,
    channels: u8,
    pcm_buffer: VecDeque<u8>,
}

impl UsbAudioDevice {
    pub fn new(slot_id: u8, out_ep: u8) -> Self {
        Self {
            slot_id,
            out_ep,
            host: None,
            volume: 80,
            sample_rate: 48000,
            channels: 2,
            pcm_buffer: VecDeque::with_capacity(MAX_PCM_BUFFER),
        }
    }

    pub fn with_host(
        host: Arc<Mutex<dyn UsbHostController>>,
        slot_id: u8,
        out_ep: u8,
    ) -> Self {
        let mut dev = Self::new(slot_id, out_ep);
        dev.host = Some(host);
        dev
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn channels(&self) -> u8 {
        self.channels
    }

    pub fn fetch_samples(&mut self, buf: &mut [u8]) -> usize {
        let count = buf.len().min(self.pcm_buffer.len());
        for b in buf.iter_mut().take(count) {
            if let Some(sample) = self.pcm_buffer.pop_front() {
                *b = sample;
            }
        }
        count
    }
}

impl AudioDevice for UsbAudioDevice {
    fn device_name(&self) -> &'static str {
        "usb-audio-uac"
    }

    fn play_pcm(&mut self, samples: &[u8]) -> Result<(), AudioDeviceError> {
        if samples.is_empty() {
            return Ok(());
        }

        // Если эндпоинт потоковой передачи Bulk OUT привязан, отправляем PCM напрямую на шину USB.
        if let Some(ref host) = self.host {
            if self.out_ep != 0 {
                let mut copy = samples.to_vec();
                if host
                    .lock()
                    .bulk_transfer(self.slot_id, self.out_ep, &mut copy, 100)
                    .is_ok()
                {
                    return Ok(());
                }
            }
        }

        let available = MAX_PCM_BUFFER.saturating_sub(self.pcm_buffer.len());
        let to_copy = samples.len().min(available);
        if to_copy == 0 {
            return Err(AudioDeviceError::NotReady);
        }

        for &byte in &samples[..to_copy] {
            self.pcm_buffer.push_back(byte);
        }
        Ok(())
    }

    fn set_volume(&mut self, volume_percent: u8) -> Result<(), AudioDeviceError> {
        self.volume = volume_percent.min(100);
        if let Some(ref host) = self.host {
            // UAC Set Current Volume request (0x21, SET_CUR=0x01) на control interface 0
            let vol_db: i16 = if self.volume == 0 {
                -32768
            } else {
                ((self.volume as i16 - 100) * 128).clamp(-12800, 0)
            };
            let mut payload = vol_db.to_le_bytes();
            let _ = host
                .lock()
                .control_transfer(self.slot_id, 0x21, 0x01, 0x0200, 0, &mut payload);
        }
        crate::klog!(
            "usb_audio: {}: {}%",
            t("установлена громкость", "volume set to"),
            self.volume
        );
        Ok(())
    }

    fn is_busy(&self) -> bool {
        !self.pcm_buffer.is_empty()
    }
}

pub fn probe_audio_device(slot_id: u8, out_ep: u8, dm: &mut DeviceManager) {
    let dev = UsbAudioDevice::new(slot_id, out_ep);
    crate::klog!(
        "usb_audio: {}",
        t(
            "зарегистрировано аудиоустройство USB Audio Class (UAC 1.0/2.0)",
            "registered USB Audio Class (UAC 1.0/2.0) audio device"
        )
    );
    dm.register_audio_device(Arc::new(Mutex::new(dev)));
}

pub fn probe_audio_device_with_host(
    host: Arc<Mutex<dyn UsbHostController>>,
    slot_id: u8,
    out_ep: u8,
    dm: &mut DeviceManager,
) {
    let dev = UsbAudioDevice::with_host(host, slot_id, out_ep);
    crate::klog!(
        "usb_audio: {}",
        t(
            "зарегистрировано аудиоустройство USB Audio Class (UAC 1.0/2.0)",
            "registered USB Audio Class (UAC 1.0/2.0) audio device"
        )
    );
    dm.register_audio_device(Arc::new(Mutex::new(dev)));
}
