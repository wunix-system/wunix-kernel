use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;
use uart_16550::SerialPort;
use x86_64::instructions::port::Port;

pub const COM1_BASE: u16 = 0x3F8;
const COM1_LINE_STATUS: u16 = COM1_BASE + 5;
const COM1_SCRATCH: u16 = COM1_BASE + 7;

static SERIAL_PRESENT: AtomicBool = AtomicBool::new(false);
pub static SERIAL1: Mutex<SerialPort> = Mutex::new(unsafe { SerialPort::new(COM1_BASE) });

// Проверка наличия UART контроллера 16550 на шине I/O.
// В VirtualBox (где COM-порт отключен по умолчанию) и на реальных ПК без COM1
// неподключенные порты ввода-вывода "плавают" в 0xFF.
// Если читать статус LSR (0x3FD) без проверки, он всегда вернет 0xFF, где бит 0 (Data Ready)
// равен 1, что вызывает вечный цикл чтения фантомных байтов 0xFF в обработчике таймера.
fn probe_com1() -> bool {
    unsafe {
        let mut scr = Port::<u8>::new(COM1_SCRATCH);
        scr.write(0x55);
        if scr.read() != 0x55 {
            return false;
        }
        scr.write(0xAA);
        if scr.read() != 0xAA {
            return false;
        }
        true
    }
}

pub fn init() {
    if probe_com1() {
        SERIAL_PRESENT.store(true, Ordering::Relaxed);
        SERIAL1.lock().init();
    } else {
        SERIAL_PRESENT.store(false, Ordering::Relaxed);
    }
}

pub fn is_present() -> bool {
    SERIAL_PRESENT.load(Ordering::Relaxed)
}

pub fn try_receive() -> Option<u8> {
    if !is_present() {
        return None;
    }

    x86_64::instructions::interrupts::without_interrupts(|| {
        // Дополнительная аппаратная защита: если LSR вернул 0xFF, шина в обрыве
        let mut lsr_port = Port::<u8>::new(COM1_LINE_STATUS);
        let lsr: u8 = unsafe { lsr_port.read() };
        if lsr == 0xFF || (lsr & 1) == 0 {
            return None;
        }
        SERIAL1.lock().try_receive().ok()
    })
}

#[macro_export]
macro_rules! serial_print {
    ($($arg:tt)*) => {
        $crate::drivers::serial::_print(format_args!($($arg)*));
    };
}

#[macro_export]
macro_rules! serial_println {
    () => ($crate::serial_print!("\n"));
    ($fmt:expr) => ($crate::serial_print!(concat!($fmt, "\n")));
    ($fmt:expr, $($arg:tt)*) => ($crate::serial_print!(
        concat!($fmt, "\n"), $($arg)*));
}

#[doc(hidden)]
pub fn _print(args: ::core::fmt::Arguments) {
    if !is_present() {
        return;
    }

    use core::fmt::Write;
    use x86_64::instructions::interrupts;

    // Выводим исключительно в COM1 (UART 16550), не засоряя графический экран консоли
    interrupts::without_interrupts(|| {
        if let Some(mut serial) = SERIAL1.try_lock() {
            let _ = serial.write_fmt(args);
        }
    });
}
