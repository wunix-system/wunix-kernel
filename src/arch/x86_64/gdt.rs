use spin::Once;
use x86_64::structures::gdt::{Descriptor, GlobalDescriptorTable, SegmentSelector};
use x86_64::structures::tss::TaskStateSegment;
use x86_64::VirtAddr;

pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;
const IST_STACK_SIZE: usize = 4096 * 5;
const KERNEL_SYSCALL_STACK_SIZE: usize = 4096 * 8; // 32 КиБ — стек системных вызовов ядра (syscall)
const KERNEL_INTERRUPT_STACK_SIZE: usize = 4096 * 8; // 32 КиБ — стек аппаратных прерываний из Ring 3 (TSS.rsp0)

#[repr(align(16))]
struct AlignedStack<const N: usize>([u8; N]);

static mut DOUBLE_FAULT_STACK: AlignedStack<IST_STACK_SIZE> = AlignedStack([0; IST_STACK_SIZE]);
static mut KERNEL_SYSCALL_STACK: AlignedStack<KERNEL_SYSCALL_STACK_SIZE> = AlignedStack([0; KERNEL_SYSCALL_STACK_SIZE]);
static mut KERNEL_INTERRUPT_STACK: AlignedStack<KERNEL_INTERRUPT_STACK_SIZE> = AlignedStack([0; KERNEL_INTERRUPT_STACK_SIZE]);

static TSS: Once<TaskStateSegment> = Once::new();

pub fn get_kernel_syscall_stack_top() -> u64 {
    let stack_start = unsafe { VirtAddr::from_ptr(&raw const KERNEL_SYSCALL_STACK.0) };
    stack_start.as_u64() + KERNEL_SYSCALL_STACK_SIZE as u64
}

pub fn get_kernel_interrupt_stack_top() -> u64 {
    let stack_start = unsafe { VirtAddr::from_ptr(&raw const KERNEL_INTERRUPT_STACK.0) };
    stack_start.as_u64() + KERNEL_INTERRUPT_STACK_SIZE as u64
}

/// Обновляет адрес RSP0 в дескрипторе TSS при переключении контекста потоков.
/// Аппаратные прерывания CPU при работе в Ring 3 используют этот адрес для стека ядра.
pub fn set_tss_rsp0(rsp0: u64) {
    let tss_ptr = get_tss() as *const TaskStateSegment as *mut TaskStateSegment;
    unsafe {
        (*tss_ptr).privilege_stack_table[0] = VirtAddr::new(rsp0);
    }
}

fn get_tss() -> &'static TaskStateSegment {
    TSS.call_once(|| {
        let mut tss = TaskStateSegment::new();

        tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] = {
            let stack_start = unsafe { VirtAddr::from_ptr(&raw const DOUBLE_FAULT_STACK.0) };
            // x86 стек растет вниз, поэтому в IST кладём верхнюю границу буфера
            stack_start + IST_STACK_SIZE as u64
        };

        // rsp0 — стек, на который процессор аппаратно переключается при переходе Ring 3 → Ring 0
        // (срабатывание аппаратного прерывания или исключения в пользовательском режиме).
        // Обязан быть выделен в отдельный буфер KERNEL_INTERRUPT_STACK, полностью изолированный
        // от KERNEL_SYSCALL_STACK. В противном случае тики таймера PIT или прерывания клавиатуры в Ring 3
        // начнут выталкивать фреймы на вершину KERNEL_SYSCALL_STACK и безвозвратно затрут
        // сохраненный контекст родительского процесса (/bin/init), ожидающего завершения шелла (/bin/wsh).
        tss.privilege_stack_table[0] = VirtAddr::new(get_kernel_interrupt_stack_top());

        tss
    })
}

#[derive(Debug, Clone, Copy)]
pub struct Selectors {
    pub code_selector: SegmentSelector,
    pub data_selector: SegmentSelector,
    pub tss_selector: SegmentSelector,
    pub user_code_selector: SegmentSelector,
    pub user_data_selector: SegmentSelector,
}

static GDT: Once<(GlobalDescriptorTable, Selectors)> = Once::new();

/// Раскладка GDT строго зафиксирована под требования инструкций syscall/sysret:
///
///   0x00  Null
///   0x08  Kernel Code (Ring 0, 64-bit)
///   0x10  Kernel Data (Ring 0)
///   0x18  User Data   (Ring 3)          ← sysretq грузит SS = STAR[63:48] + 8
///   0x20  User Code   (Ring 3, 64-bit)  ← sysretq грузит CS = STAR[63:48] + 16
///   0x28  TSS         (16 байт, два слота)
///
/// Порядок User Data перед User Code — обязательное условие для sysretq.
/// При STAR[63:48] = 0x10 процессор вычисляет:
///   CS = 0x10 + 16 = 0x20, с установленным RPL=3 → 0x23
///   SS = 0x10 + 8  = 0x18, с установленным RPL=3 → 0x1B
pub fn init() -> Selectors {
    let (gdt, selectors) = GDT.call_once(|| {
        let mut gdt = GlobalDescriptorTable::new();
        let code_selector = gdt.append(Descriptor::kernel_code_segment());     // 0x08
        let data_selector = gdt.append(Descriptor::kernel_data_segment());     // 0x10
        let user_data_selector = gdt.append(Descriptor::user_data_segment());  // 0x18
        let user_code_selector = gdt.append(Descriptor::user_code_segment());  // 0x20
        let tss_selector = gdt.append(Descriptor::tss_segment(get_tss()));     // 0x28

        (gdt, Selectors {
            code_selector,
            data_selector,
            tss_selector,
            user_code_selector,
            user_data_selector,
        })
    });

    gdt.load();

    unsafe {
        use x86_64::instructions::segmentation::{CS, DS, ES, SS, Segment};
        use x86_64::instructions::tables::load_tss;

        // Перезагружаем селекторы сегментов: CS для кода, DS/ES/SS для данных и стека ядра.
        // Если оставить в SS старый селектор загрузчика Limine (0x30), инструкция iretq
        // при первом же возврате из обработчика прерывания выбросит General Protection Fault (#GP),
        // так как селектор 0x30 выходит за границы нашего GDT.
        CS::set_reg(selectors.code_selector);
        DS::set_reg(selectors.data_selector);
        ES::set_reg(selectors.data_selector);
        SS::set_reg(selectors.data_selector);
        load_tss(selectors.tss_selector);
    }

    *selectors
}
