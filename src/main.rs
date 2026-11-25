#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(wunix_kernel::test_runner)]
#![reexport_test_harness_main = "test_main"]

extern crate alloc;

use core::panic::PanicInfo;
use limine::request::{
    BootloaderInfoRequest, ExecutableFileRequest, FirmwareTypeRequest, FramebufferRequest,
    HhdmRequest, MemmapRequest, ModulesRequest as ModuleRequest, RsdpRequest,
};
use limine::BaseRevision;
use wunix_kernel::i18n::t;
use wunix_kernel::{kboot, klog, serial_println};

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

// Запрашиваем у Limine указатель на таблицу ACPI RSDP
#[used]
#[unsafe(no_mangle)]
pub static RSDP_REQUEST: RsdpRequest = RsdpRequest::new();

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

    // Чтение параметров командной строки ядра (например, lang=ru / lang=en)
    let cmdline = if let Some(file_response) = EXECUTABLE_FILE_REQUEST.response() {
        let cmd = file_response.executable_file().cmdline();
        wunix_kernel::i18n::init_from_cmdline(cmd);
        cmd
    } else {
        ""
    };

    // 2. Инициализация графической консоли высокого разрешения через линейный кадровый буфер
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

    let fw_label = match FIRMWARE_TYPE_REQUEST.response().map(|r| r.firmware_type) {
        Some(limine::firmware::FIRMWARE_TYPE_EFI64) => "UEFI 64-bit",
        Some(limine::firmware::FIRMWARE_TYPE_EFI32) => "UEFI 32-bit",
        Some(limine::firmware::FIRMWARE_TYPE_X86BIOS) => "BIOS",
        _ => "x86_64",
    };

    kboot!(
        "",
        "Wunix {} (GENERIC) #1: {} ({})",
        wunix_kernel::version::VERSION,
        wunix_kernel::version::ARCH,
        fw_label
    );

    if wunix_kernel::drivers::serial::is_present() {
        kboot!(
            "com0",
            "{}",
            t(
                "последовательный порт 16550 по адресу 0x3f8 (115200 8N1)",
                "UART 16550 at 0x3f8 (115200 8N1)"
            )
        );
    }
    if fb_width > 0 {
        let cols = (fb_width as usize).saturating_sub(24) / 8;
        let rows = (fb_height as usize).saturating_sub(20) / 16;
        kboot!(
            "fb0",
            "{} {}x{}, {} bpp ({} {}x{})",
            t("графический кадровый буфер", "linear framebuffer"),
            fb_width,
            fb_height,
            fb_bpp,
            t("консоль", "console"),
            cols,
            rows
        );
    }
    kboot!(
        "",
        "{}: \"{}\"",
        t("параметры загрузки", "boot flags"),
        cmdline
    );

    // 3. Валидация базовой ревизии протокола загрузки
    let _rev = if let Some(rev) = BASE_REVISION.actual_revision() {
        rev
    } else {
        klog!(
            "panic: {}",
            t(
                "неподдерживаемая ревизия протокола загрузки",
                "unsupported boot protocol revision"
            )
        );
        wunix_kernel::halt_loop();
    };

    // 4. Получение смещения прямого отображения физической памяти (HHDM)
    let hhdm_offset = if let Some(hhdm) = HHDM_REQUEST.response() {
        hhdm.offset
    } else {
        klog!(
            "panic: {}",
            t(
                "отсутствует смещение прямого отображения HHDM",
                "missing HHDM offset from bootloader"
            )
        );
        wunix_kernel::halt_loop();
    };

    // 5. Получение карты физической памяти и вычисление статистики
    let memmap_entries = if let Some(memmap) = MEMORY_MAP_REQUEST.response() {
        memmap.entries()
    } else {
        klog!(
            "panic: {}",
            t(
                "отсутствует карта физической памяти",
                "missing physical memory map from bootloader"
            )
        );
        wunix_kernel::halt_loop();
    };

    let mem_stats = wunix_kernel::memory::collect_memory_stats(memmap_entries);
    let cpu_info = wunix_kernel::arch::x86_64::CpuInfo::current();
    let rdrand_str = if wunix_kernel::arch::x86_64::cpu::has_rdrand() {
        t("да", "yes")
    } else {
        t("нет", "no")
    };

    kboot!(
        "cpu0",
        "{} ({}, 64-бит, RDRAND: {})",
        cpu_info.brand(),
        cpu_info.vendor(),
        rdrand_str
    );
    kboot!(
        "",
        "{}  = {} ({} {})",
        t("реальная память", "real memory"),
        mem_stats.ram_bytes,
        mem_stats.ram_mb(),
        t("МБ", "MiB")
    );
    kboot!(
        "",
        "{} = {} ({} {}, {}: {})",
        t("доступная память", "avail memory"),
        mem_stats.usable_bytes,
        mem_stats.usable_mb(),
        t("МБ", "MiB"),
        t("областей", "regions"),
        mem_stats.usable_regions
    );

    // 6. Инициализация платформенных подсистем CPU (FPU/SSE, GDT, TSS, IDT, PIC8259, Syscall MSR)
    let _selectors = wunix_kernel::init();

    // 7. Инициализация виртуальной памяти и кучи
    wunix_kernel::memory::init(memmap_entries, hhdm_offset);

    // Передаем адрес ACPI RSDP в подсистему питания и таблиц ACPI ядра
    if let Some(rsdp_response) = RSDP_REQUEST.response() {
        wunix_kernel::drivers::acpi::set_rsdp_address(rsdp_response.address as u64);
    }

    // 8. Инициализация таймера, аппаратных часов RTC и клавиатуры
    wunix_kernel::arch::x86_64::timer::init(100);
    kboot!(
        "pit0",
        "{}",
        t(
            "системный таймер 8254 @ 100 Гц (шаг 10 мс)",
            "8254 interval timer @ 100 Hz (10 ms tick)"
        )
    );

    let rtc_dt = wunix_kernel::drivers::rtc::read_datetime();
    kboot!(
        "rtc0",
        "{}: {:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
        t("часы CMOS MC146818", "CMOS MC146818 clock"),
        rtc_dt.year,
        rtc_dt.month,
        rtc_dt.day,
        rtc_dt.hour,
        rtc_dt.minute,
        rtc_dt.second
    );

    wunix_kernel::drivers::keyboard::init();
    kboot!(
        "pckbd0",
        "{}",
        t(
            "клавиатура PS/2 (Scancode Set 1, раскладки EN/RU)",
            "PS/2 keyboard (Scancode Set 1, EN/RU layouts)"
        )
    );

    // Инициализация шины PCI и драйверов оборудования (SATA/AHCI, ATA, VirtIO, VMware, VBox, Hyper-V)
    wunix_kernel::drivers::init();

    // 9. Инициализация виртуальной файловой системы (VFS) и загрузка initramfs
    let initramfs_data = MODULE_REQUEST
        .response()
        .and_then(|module_resp| module_resp.modules().first().map(|m| m.data()));

    wunix_kernel::fs::init(initramfs_data);

    // Фиксация параметров командной строки загрузчика в /proc/cmdline и монтирование root раздела (при mode=disk)
    if !cmdline.is_empty() {
        let _ = wunix_kernel::fs::ops::write_file_ramfs("/proc/cmdline", cmdline.as_bytes());

        for token in cmdline.split_whitespace() {
            if let Some(root_dev) = token.strip_prefix("root=") {
                kboot!(
                    "",
                    "{} {} {} /...",
                    t("монтирование корня", "mounting root"),
                    root_dev,
                    t("в", "on")
                );
                match wunix_kernel::fs::mount(root_dev, "/", "", 0) {
                    Ok(_) => {
                        let fs_name = wunix_kernel::fs::list_mounts()
                            .into_iter()
                            .find(|m| m.target == "/")
                            .map(|m| m.fstype)
                            .unwrap_or_else(|| alloc::string::String::from("auto"));
                        kboot!(
                            "",
                            "{} {} ({}) {}",
                            t("корень", "root"),
                            root_dev,
                            fs_name,
                            t("смонтирован в /", "mounted on /")
                        );
                    }
                    Err(e) => {
                        kboot!(
                            "",
                            "{} {} / : {:?}",
                            t("ошибка монтирования корня", "root mount failed on"),
                            root_dev,
                            e
                        );
                    }
                }
            }
        }
    }

    // Инициализация сетевого стека ядра smoltcp поверх зарегистрированного сетевого адаптера
    if let Some(net_dev) = wunix_kernel::drivers::get_net_device(0) {
        wunix_kernel::net::init(net_dev);
    } else {
        kboot!(
            t("сеть", "net"),
            "{}",
            t("сетевые адаптеры не обнаружены", "no network adapters detected")
        );
    }

    // 10. Инициализация многозадачности ядра
    wunix_kernel::task::init();
    kboot!(
        t("планировщик", "sched"),
        "{}",
        t(
            "вытесняющий Round-Robin (квант 50 мс)",
            "preemptive Round-Robin (50 ms quantum)"
        )
    );

    #[cfg(test)]
    test_main();

    let (init_path, init_binary) = wunix_kernel::fs::read_file("/bin/init")
        .map(|b| ("/bin/init", b))
        .or_else(|_| wunix_kernel::fs::read_file("/sbin/init").map(|b| ("/sbin/init", b)))
        .or_else(|_| wunix_kernel::fs::read_file("/bin/wsh").map(|b| ("/bin/wsh", b)))
        .expect("FATAL: Failed to find /bin/init, /sbin/init or /bin/wsh in VFS");

    let (mut user_space, elf_info) = wunix_kernel::task::elf::load_elf(&init_binary)
        .expect("FATAL: Failed to load ELF binary for PID 1");

    let init_kb = (init_binary.len() + 1023) / 1024;
    let brk_start = (elf_info.max_vaddr + 4095) & !4095;
    wunix_kernel::task::syscall::mem::reset_process_vma(brk_start);

    let init_args = alloc::vec![alloc::string::String::from(init_path)];
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

    kboot!(
        "",
        "{} {} (PID 1, {} {}, {}: {:#x}, {}: {:#x})...",
        t("запуск", "starting"),
        init_path,
        init_kb,
        t("КБ", "KiB"),
        t("вход", "entry"),
        elf_info.entry_point.as_u64(),
        t("стек", "stack"),
        stack_top
    );
    klog!();

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
