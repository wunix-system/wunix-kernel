#![no_std]
#![cfg_attr(test, no_main)]
#![feature(custom_test_frameworks)]
#![test_runner(crate::test_runner)]
#![reexport_test_harness_main = "test_main"]
#![feature(abi_x86_interrupt)]

extern crate alloc;

#[macro_export]
macro_rules! kprint {
    ($($arg:tt)*) => {
        $crate::serial_print!($($arg)*);
        $crate::print!($($arg)*);
    };
}

#[macro_export]
macro_rules! klog {
    () => {
        $crate::serial_println!();
        $crate::println!();
    };
    ($($arg:tt)*) => {
        $crate::serial_println!($($arg)*);
        $crate::println!($($arg)*);
    };
}

pub mod arch;
pub mod bootlog;
pub mod drivers;
pub mod fs;
pub mod i18n;
pub mod memory;
pub mod net;
pub mod task;
pub mod version;

use core::panic::PanicInfo;

pub trait Testable {
    fn run(&self);
}

impl<T: Fn()> Testable for T {
    fn run(&self) {
        serial_print!("{}...\t", core::any::type_name::<T>());
        self();
        serial_println!("[ok]");
    }
}

pub fn test_runner(tests: &[&dyn Testable]) {
    serial_println!("Running {} tests", tests.len());
    for test in tests {
        test.run();
    }
    exit_qemu(QemuExitCode::Success);
}

pub fn test_panic_handler(info: &PanicInfo) -> ! {
    serial_println!("[failed]\n");
    serial_println!("Error: {}\n", info);
    exit_qemu(QemuExitCode::Failed);
    halt_loop();
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum QemuExitCode {
    Success = 0x10,
    Failed = 0x11,
}

pub fn exit_qemu(exit_code: QemuExitCode) {
    use x86_64::instructions::port::Port;

    unsafe {
        let mut port = Port::new(0xf4);
        port.write(exit_code as u32);
    }
}

pub fn init() -> arch::x86_64::gdt::Selectors {
    drivers::serial::init();
    #[cfg(target_arch = "x86_64")]
    arch::x86_64::init()
}

pub fn halt_loop() -> ! {
    loop {
        x86_64::instructions::hlt();
    }
}

#[cfg(test)]
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    test_panic_handler(info)
}

#[cfg(test)]
#[used]
#[unsafe(no_mangle)]
pub static TEST_BASE_REVISION: limine::BaseRevision = limine::BaseRevision::new();

#[cfg(test)]
#[used]
#[unsafe(no_mangle)]
pub static TEST_HHDM_REQUEST: limine::request::HhdmRequest = limine::request::HhdmRequest::new();

#[cfg(test)]
#[used]
#[unsafe(no_mangle)]
pub static TEST_MEMMAP_REQUEST: limine::request::MemmapRequest =
    limine::request::MemmapRequest::new();

#[cfg(test)]
#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    init();
    if let (Some(hhdm), Some(mmap)) = (
        TEST_HHDM_REQUEST.response(),
        TEST_MEMMAP_REQUEST.response(),
    ) {
        if TEST_BASE_REVISION.actual_revision().is_some() {
            memory::init(mmap.entries(), hhdm.offset);
        } else {
            serial_println!("[ktest] WARN: TEST_BASE_REVISION actual_revision is None");
        }
    } else {
        serial_println!("[ktest] WARN: missing HHDM/MEMMAP response from Limine");
    }
    test_main();
    halt_loop();
}

#[test_case]
fn test_trivial_assertion() {
    assert_eq!(1, 1);
}
