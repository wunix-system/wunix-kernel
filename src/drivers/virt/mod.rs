pub mod detect;
pub mod hyperv;
pub mod vbox;
pub mod vmware;
pub mod xen;

pub use detect::{detect_hypervisor, HypervisorType};

use spin::RwLock;

static DETECTED_HYPERVISOR: RwLock<Option<HypervisorType>> = RwLock::new(None);

/// Инициализирует подсистему виртуализации:
/// 1. Детектирует текущую среду выполнения (CPUID).
/// 2. Кэширует результат для быстрого доступа из procfs и драйверов.
pub fn init() -> HypervisorType {
    let hv = detect_hypervisor();
    *DETECTED_HYPERVISOR.write() = Some(hv);
    hv
}

/// Возвращает текущий тип гипервизора, на котором работает ядро.
pub fn current_hypervisor() -> HypervisorType {
    let guard = DETECTED_HYPERVISOR.read();
    if let Some(hv) = *guard {
        hv
    } else {
        detect_hypervisor()
    }
}
