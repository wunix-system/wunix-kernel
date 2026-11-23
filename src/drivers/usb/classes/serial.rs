use alloc::collections::VecDeque;

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
            "usb_serial: registered {} converter on slot {} ({:#06x}:{:#06x})",
            chip_name, slot_id, vid, pid
        );

        Self {
            slot_id,
            vendor_id: vid,
            product_id: pid,
            baud_rate: 115200,
            rx_queue: VecDeque::with_capacity(SERIAL_BUFFER_CAPACITY),
            tx_queue: VecDeque::with_capacity(SERIAL_BUFFER_CAPACITY),
        }
    }

    pub fn set_baud_rate(&mut self, rate: u32) {
        self.baud_rate = rate;
        crate::klog!("usb_serial: set baud rate to {} bps", rate);
    }

    /// Помещает байт в очередь отправки через Bulk OUT
    pub fn write_byte(&mut self, b: u8) {
        if self.tx_queue.len() < SERIAL_BUFFER_CAPACITY {
            self.tx_queue.push_back(b);
        }
    }

    /// Считывает байт из очереди приема Bulk IN
    pub fn read_byte(&mut self) -> Option<u8> {
        self.rx_queue.pop_front()
    }

    /// Помещает принятый от USB контроллера байт в очередь приема
    pub fn push_rx_byte(&mut self, b: u8) {
        if self.rx_queue.len() < SERIAL_BUFFER_CAPACITY {
            self.rx_queue.push_back(b);
        }
    }

    /// Извлекает байт для отправки в аппаратный Bulk OUT эндпоинт
    pub fn pop_tx_byte(&mut self) -> Option<u8> {
        self.tx_queue.pop_front()
    }
}
