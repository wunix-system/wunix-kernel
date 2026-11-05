use alloc::collections::VecDeque;

pub const USB_SERIAL_VENDOR_FTDI: u16 = 0x0403;
pub const USB_SERIAL_VENDOR_CH340: u16 = 0x1A86;
pub const USB_SERIAL_VENDOR_CP2102: u16 = 0x10C4;
pub const USB_SERIAL_VENDOR_PL2303: u16 = 0x067B;

pub struct UsbSerialPort {
    pub slot_id: u8,
    pub vendor_id: u16,
    pub product_id: u16,
    pub baud_rate: u32,
    rx_queue: VecDeque<u8>,
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
            "usb_serial: registered {} converter on slot {} ({:#06x}:{:#06x})",
            chip_name, slot_id, vid, pid
        );

        Self {
            slot_id,
            vendor_id: vid,
            product_id: pid,
            baud_rate: 115200,
            rx_queue: VecDeque::with_capacity(256),
        }
    }

    pub fn set_baud_rate(&mut self, rate: u32) {
        self.baud_rate = rate;
        crate::klog!("usb_serial: set baud rate to {} bps", rate);
    }

    pub fn write_byte(&mut self, _b: u8) {
        // Передача через Bulk OUT Endpoint в контроллере xHCI
    }

    pub fn read_byte(&mut self) -> Option<u8> {
        self.rx_queue.pop_front()
    }
}
