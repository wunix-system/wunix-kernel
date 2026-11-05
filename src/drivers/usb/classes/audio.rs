use alloc::sync::Arc;
use spin::Mutex;
use crate::drivers::manager::DeviceManager;
use crate::drivers::traits::{AudioDevice, AudioDeviceError};

pub const USB_CLASS_AUDIO: u8 = 0x01;
pub const AUDIO_SUBCLASS_CONTROL: u8 = 0x01;
pub const AUDIO_SUBCLASS_STREAMING: u8 = 0x02;

pub struct UsbAudioDevice {
    slot_id: u8,
    isochronous_ep: u8,
    volume: u8,
    sample_rate: u32,
    channels: u8,
}

impl UsbAudioDevice {
    pub fn new(slot_id: u8, isochronous_ep: u8) -> Self {
        Self {
            slot_id,
            isochronous_ep,
            volume: 80,
            sample_rate: 48000,
            channels: 2,
        }
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
        // В реальном контроллере xHCI/EHCI данные передаются через Isochronous Transfer Ring
        crate::klog!(
            "usb_audio: streaming {} bytes PCM ({}Hz, {}ch) to slot {} ep {}",
            samples.len(),
            self.sample_rate,
            self.channels,
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
        false
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
