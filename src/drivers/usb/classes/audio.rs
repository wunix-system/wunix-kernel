use alloc::collections::VecDeque;
use alloc::sync::Arc;
use spin::Mutex;
use crate::drivers::manager::DeviceManager;
use crate::drivers::traits::{AudioDevice, AudioDeviceError};

pub const USB_CLASS_AUDIO: u8 = 0x01;
pub const AUDIO_SUBCLASS_CONTROL: u8 = 0x01;
pub const AUDIO_SUBCLASS_STREAMING: u8 = 0x02;

const MAX_PCM_BUFFER: usize = 32768; // 32 КиБ буфер сэмплов

pub struct UsbAudioDevice {
    slot_id: u8,
    isochronous_ep: u8,
    volume: u8,
    sample_rate: u32,
    channels: u8,
    pcm_buffer: VecDeque<u8>,
}

impl UsbAudioDevice {
    pub fn new(slot_id: u8, isochronous_ep: u8) -> Self {
        Self {
            slot_id,
            isochronous_ep,
            volume: 80,
            sample_rate: 48000,
            channels: 2,
            pcm_buffer: VecDeque::with_capacity(MAX_PCM_BUFFER),
        }
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn channels(&self) -> u8 {
        self.channels
    }

    /// Считывает очередную порцию сэмплов для отправки в изохронный эндпоинт контроллера
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

        let available = MAX_PCM_BUFFER.saturating_sub(self.pcm_buffer.len());
        let to_copy = samples.len().min(available);
        if to_copy == 0 {
            return Err(AudioDeviceError::NotReady);
        }

        for &byte in &samples[..to_copy] {
            self.pcm_buffer.push_back(byte);
        }

        crate::klog!(
            "usb_audio: buffered {} bytes PCM (total in buffer: {} bytes) for slot {} ep {}",
            to_copy,
            self.pcm_buffer.len(),
            self.slot_id,
            self.isochronous_ep
        );

        Ok(())
    }

    fn set_volume(&mut self, volume_percent: u8) -> Result<(), AudioDeviceError> {
        self.volume = volume_percent.min(100);
        crate::klog!("usb_audio: set master volume to {}%", self.volume);
        Ok(())
    }

    fn is_busy(&self) -> bool {
        !self.pcm_buffer.is_empty()
    }
}

pub fn probe_audio_device(
    slot_id: u8,
    isochronous_ep: u8,
    dm: &mut DeviceManager,
) {
    let dev = UsbAudioDevice::new(slot_id, isochronous_ep);
    crate::klog!("usb_audio: registered USB Audio Class (UAC 1.0/2.0) headset/DAC");
    dm.register_audio_device(Arc::new(Mutex::new(dev)));
}
