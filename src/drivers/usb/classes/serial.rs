use alloc::collections::VecDeque;
use alloc::sync::Arc;
use spin::Mutex;

use crate::drivers::usb::host::UsbHostController;
use crate::i18n::t;

pub const USB_SERIAL_VENDOR_FTDI: u16 = 0x0403;
pub const USB_SERIAL_VENDOR_CH340: u16 = 0x1A86;
pub const USB_SERIAL_VENDOR_CP2102: u16 = 0x10C4;
pub const USB_SERIAL_VENDOR_PL2303: u16 = 0x067B;

const SERIAL_BUFFER_CAPACITY: usize = 512;

pub struct UsbSerialPort {
    pub slot_id: u8,
    pub vendor_id: u16,
    pub product_id: u16,
    pub baud_rate: u32,
    ep_in: u8,
    ep_out: u8,
    host: Option<Arc<Mutex<dyn UsbHostController>>>,
    rx_queue: VecDeque<u8>,
    tx_queue: VecDeque<u8>,
}

impl UsbSerialPort {
    pub fn new(slot_id: u8, vid: u16, pid: u16) -> Self {
        let chip_name = match vid {
            USB_SERIAL_VENDOR_FTDI => "FTDI FT232 USB-UART",
            USB_SERIAL_VENDOR_CH340 => "WCH CH340 USB-UART",
            USB_SERIAL_VENDOR_CP2102 => "Silicon Labs CP2102 USB-UART",
            USB_SERIAL_VENDOR_PL2303 => "Prolific PL2303 USB-UART",
            _ => "Generic USB-Serial",
        };

        crate::klog!(
            "usb_serial: {} {} ({}: {}, {:#06x}:{:#06x})",
            t("зарегистрирован преобразователь", "registered converter"),
            chip_name,
            t("слот", "slot"),
            slot_id,
            vid,
            pid
        );

        Self {
            slot_id,
            vendor_id: vid,
            product_id: pid,
            baud_rate: 115200,
            ep_in: 0,
            ep_out: 0,
            host: None,
            rx_queue: VecDeque::with_capacity(SERIAL_BUFFER_CAPACITY),
            tx_queue: VecDeque::with_capacity(SERIAL_BUFFER_CAPACITY),
        }
    }

    pub fn with_host(
        host: Arc<Mutex<dyn UsbHostController>>,
        slot_id: u8,
        vid: u16,
        pid: u16,
        ep_in: u8,
        ep_out: u8,
    ) -> Self {
        let mut port = Self::new(slot_id, vid, pid);
        port.host = Some(host);
        port.ep_in = ep_in;
        port.ep_out = ep_out;
        port
    }

    pub fn set_baud_rate(&mut self, rate: u32) {
        self.baud_rate = rate;
        if let Some(ref host) = self.host {
            // CDC ACM SET_LINE_CODING (0x20): dwDTERate, bCharFormat=0, bParityType=0, bDataBits=8
            let mut line_coding = [0u8; 7];
            line_coding[0..4].copy_from_slice(&rate.to_le_bytes());
            line_coding[6] = 8;
            let _ = host
                .lock()
                .control_transfer(self.slot_id, 0x21, 0x20, 0, 0, &mut line_coding);
        }
        crate::klog!(
            "usb_serial: {}: {} {}",
            t("скорость порта", "baud rate set to"),
            rate,
            t("бод", "bps")
        );
    }

    pub fn write_byte(&mut self, b: u8) {
        if let Some(ref host) = self.host {
            if self.ep_out != 0 {
                let mut byte = [b];
                if host
                    .lock()
                    .bulk_transfer(self.slot_id, self.ep_out, &mut byte, 20)
                    .is_ok()
                {
                    return;
                }
            }
        }
        if self.tx_queue.len() < SERIAL_BUFFER_CAPACITY {
            self.tx_queue.push_back(b);
        }
    }

    pub fn read_byte(&mut self) -> Option<u8> {
        if let Some(b) = self.rx_queue.pop_front() {
            return Some(b);
        }
        if let Some(ref host) = self.host {
            if self.ep_in != 0 {
                let mut buf = [0u8; 64];
                if let Ok(n) = host
                    .lock()
                    .bulk_transfer(self.slot_id, self.ep_in, &mut buf, 1)
                {
                    for &byte in &buf[..n] {
                        if self.rx_queue.len() < SERIAL_BUFFER_CAPACITY {
                            self.rx_queue.push_back(byte);
                        }
                    }
                    return self.rx_queue.pop_front();
                }
            }
        }
        None
    }

    pub fn push_rx_byte(&mut self, b: u8) {
        if self.rx_queue.len() < SERIAL_BUFFER_CAPACITY {
            self.rx_queue.push_back(b);
        }
    }

    pub fn pop_tx_byte(&mut self) -> Option<u8> {
        self.tx_queue.pop_front()
    }
}
