#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(wunix_kernel::test_runner)]
#![reexport_test_harness_main = "test_main"]

extern crate alloc;

use core::panic::PanicInfo;
use limine::request::{
    BootloaderInfoRequest, ExecutableFileRequest, FirmwareTypeRequest, FramebufferRequest,
    HhdmRequest, MemmapRequest, ModulesRequest as ModuleRequest,
};
use limine::BaseRevision;
use wunix_kernel::i18n::t;
use wunix_kernel::{klog, serial_println};

// Сообщаем загрузчику Limine, что ядро поддерживает базовую ревизию протокола
#[used]
#[unsafe(no_mangle)]
pub static BASE_REVISION: BaseRevision = BaseRevision::new();

// Запрашиваем информацию о загрузчике
#[used]
#[unsafe(no_mangle)]
pub static BOOTLOADER_INFO_REQUEST: BootloaderInfoRequest = BootloaderInfoRequest::new();

// Запрашиваем информацию о загруженном файле ядра (включая параметры cmdline)
#[used]
#[unsafe(no_mangle)]
pub static EXECUTABLE_FILE_REQUEST: ExecutableFileRequest = ExecutableFileRequest::new();

// Запрашиваем тип прошивки платформы (BIOS/UEFI)
#[used]
#[unsafe(no_mangle)]
pub static FIRMWARE_TYPE_REQUEST: FirmwareTypeRequest = FirmwareTypeRequest::new();

// Запрашиваем у Limine смещение Higher-Half Direct Map (HHDM)
#[used]
#[unsafe(no_mangle)]
pub static HHDM_REQUEST: HhdmRequest = HhdmRequest::new();

// Запрашиваем у Limine карту физической памяти
#[used]
#[unsafe(no_mangle)]
pub static MEMORY_MAP_REQUEST: MemmapRequest = MemmapRequest::new();

// Запрашиваем у Limine графический фреймбуфер
#[used]
#[unsafe(no_mangle)]
pub static FRAMEBUFFER_REQUEST: FramebufferRequest = FramebufferRequest::new();

// Запрашиваем у Limine загруженные модули ядра (initramfs)
#[used]
#[unsafe(no_mangle)]
pub static MODULE_REQUEST: ModuleRequest = ModuleRequest::new();

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    // 1. Инициализация COM1 для консольного вывода
    wunix_kernel::drivers::serial::init();

    // Чтение параметров командной строки ядра (cmdline) от Limine (например, lang=ru / lang=en)
    if let Some(file_response) = EXECUTABLE_FILE_REQUEST.response() {
        let cmdline = file_response.executable_file().cmdline();
        wunix_kernel::i18n::init_from_cmdline(cmdline);
    }

    // 2. Инициализация графической консоли высокого разрешения через Limine Framebuffer
    let (fb_width, fb_height, fb_bpp) = if let Some(fb_response) = FRAMEBUFFER_REQUEST.response() {
        if let Some(fb) = fb_response.framebuffers().first() {
            wunix_kernel::drivers::display::init(fb);
            (fb.width, fb.height, fb.bpp)
        } else {
            (0, 0, 0)
        }
    } else {
        (0, 0, 0)
    };

    // Стартовый чистый лог загрузки Wunix OS
    klog!(
        "{}",
        t(
            "Загрузка Wunix OS (x86_64)...",
            "Booting Wunix OS (x86_64)..."
        )
    );

    // 3. Валидация протокола загрузчика Limine
    let _rev = if let Some(rev) = BASE_REVISION.actual_revision() {
        rev
    } else {
        klog!("panic: limine base revision tag not recognized");
        wunix_kernel::halt_loop();
    };

    // 4. Получение смещения прямого отображения физической памяти (HHDM)
    let hhdm_offset = if let Some(hhdm) = HHDM_REQUEST.response() {
        hhdm.offset
    } else {
        klog!("panic: limine hhdm response missing");
        wunix_kernel::halt_loop();
    };

    // 5. Получение карты физической памяти и вычисление статистики
    let memmap_entries = if let Some(memmap) = MEMORY_MAP_REQUEST.response() {
        memmap.entries()
    } else {
        klog!("panic: limine memmap response missing");
        wunix_kernel::halt_loop();
    };

    let mem_stats = wunix_kernel::memory::collect_memory_stats(memmap_entries);
    let cpu_info = wunix_kernel::arch::x86_64::CpuInfo::current();

    klog!(
        "  {}: {} [{}, 64-бит]",
        t("Процессор", "CPU"),
        cpu_info.brand(),
        cpu_info.vendor()
    );
    klog!(
        "  {}: {} МБ доступно / {} МБ RAM (куча ядра: {} МБ)",
        t("Память", "Memory"),
        mem_stats.usable_mb(),
        mem_stats.ram_mb(),
        wunix_kernel::memory::heap::HEAP_SIZE / (1024 * 1024)
    );

    // 6. Инициализация платформенных подсистем CPU (GDT, TSS, IDT, PIC8259, Syscall MSR)
    let _selectors = wunix_kernel::init();
    klog!(
        "  {}: GDT, IDT (256 векторов), таймер PIT 8254 @ 100 Гц",
        t("Прерывания", "Interrupts")
    );

    // 7. Инициализация виртуальной памяти и кучи
    wunix_kernel::memory::init(memmap_entries, hhdm_offset);

    // 8. Инициализация таймера и устройств ввода
    wunix_kernel::arch::x86_64::timer::init(100);
    wunix_kernel::drivers::keyboard::init();

    if fb_width > 0 {
        klog!(
            "  {}: Limine FB {}x{}, {} bpp, клавиатура PS/2",
            t("Консоль", "Console"),
            fb_width,
            fb_height,
            fb_bpp
        );
    } else {
        klog!(
            "  {}: COM1 (115200 8N1), клавиатура PS/2",
            t("Консоль", "Console")
        );
    }

    // Инициализация шины PCI и VirtIO
    wunix_kernel::drivers::init();
    let pci_count = wunix_kernel::drivers::with_device_manager(|dm| dm.pci_devices().len()).unwrap_or(0);
    klog!(
        "  {}: обнаружено устройств: {}",
        t("Шина PCI", "PCI bus"),
        pci_count
    );

    // 9. Инициализация виртуальной файловой системы (VFS) и загрузка initramfs
    let initramfs_data = if let Some(module_resp) = MODULE_REQUEST.response() {
        let modules = module_resp.modules();
        if let Some(first_module) = modules.first() {
            let data = first_module.data();
            let size_kb = (data.len() + 1023) / 1024;
            klog!(
                "  {}: распаковка архива initramfs ({} КБ)...",
                t("Файловая система", "Filesystem"),
                size_kb
            );
            Some(data)
        } else {
            None
        }
    } else {
        None
    };

    wunix_kernel::fs::init(initramfs_data);

    // Фиксация параметров командной строки загрузчика в /proc/cmdline
    if let Some(file_response) = EXECUTABLE_FILE_REQUEST.response() {
        let cmdline = file_response.executable_file().cmdline();
        let _ = wunix_kernel::fs::ops::write_file_ramfs("/proc/cmdline", cmdline.as_bytes());
    }

    // Инициализация сетевого стека ядра smoltcp поверх зарегистрированного сетевого адаптера
    if let Some(net_dev) = wunix_kernel::drivers::get_net_device(0) {
        let driver_name = net_dev.lock().driver_name();
        wunix_kernel::net::init(net_dev);
        klog!(
            "  {}: {} (DHCP/smoltcp)",
            t("Сетевой адаптер", "Network adapter"),
            driver_name
        );
    }

    // 10. Инициализация многозадачности ядра
    wunix_kernel::task::init();
    klog!(
        "  {}: планировщик запущен (квант 50 мс)",
        t("Многозадачность", "Multitasking")
    );

    #[cfg(test)]
    test_main();

    klog!(
        "{}",
        t(
            "Запуск пространства пользователя (PID 1: /bin/init)...",
            "Starting userspace (PID 1: /bin/init)..."
        )
    );
    klog!();

    let init_binary = wunix_kernel::fs::read_file("/bin/init")
        .or_else(|_| wunix_kernel::fs::read_file("/bin/wsh"))
        .expect("FATAL: Failed to find /bin/init or /bin/wsh in VFS");

    let (mut user_space, elf_info) = wunix_kernel::task::elf::load_elf(&init_binary)
        .expect("FATAL: Failed to load ELF binary for PID 1");

    let init_args = alloc::vec![alloc::string::String::from("/bin/init")];
    let init_env = alloc::vec![
        alloc::string::String::from("PATH=/bin:/usr/bin"),
        alloc::string::String::from("USER=root"),
        alloc::string::String::from("HOME=/root"),
        alloc::string::String::from("TERM=xterm-256color"),
    ];
    let (stack_top, argv_addr) = wunix_kernel::task::elf::setup_user_stack(
        &mut user_space,
        &init_args,
        &init_env,
        &elf_info,
    )
    .expect("FATAL: Failed to setup user stack for PID 1");

    user_space.activate();

    unsafe {
        wunix_kernel::arch::x86_64::syscall::jump_to_user_mode(
            elf_info.entry_point.as_u64(),
            stack_top,
            init_args.len() as u64,
            argv_addr,
        );
    }
}

#[cfg(not(test))]
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    serial_println!("panic: {}", info);
    wunix_kernel::println!("panic: {}", info);
    wunix_kernel::halt_loop();
}

#[cfg(test)]
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    wunix_kernel::test_panic_handler(info)
}
