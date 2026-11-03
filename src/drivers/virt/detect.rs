use core::arch::x86_64::__cpuid;
use core::fmt;

/// Известные ядру гипервизоры и платформы виртуализации.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HypervisorType {
    /// Реальное физическое «железо» (Bare Metal)
    BareMetal,
    /// Linux KVM (Kernel-based Virtual Machine) / Proxmox VE
    Kvm,
    /// Программный эмулятор QEMU TCG
    Qemu,
    /// Oracle VM VirtualBox
    VirtualBox,
    /// VMware Workstation, Fusion, Player или ESXi
    VMware,
    /// Microsoft Hyper-V / Azure VM
    HyperV,
    /// Xen Paravirtualization / HVM
    Xen,
    /// FreeBSD Bhyve
    Bhyve,
    /// Неизвестная виртуальная машина со специфичной сигнатурой
    Unknown([u8; 12]),
}

impl HypervisorType {
    /// Возвращает текстовое системное имя гипервизора для логов и procfs.
    pub const fn name(&self) -> &'static str {
        match self {
            Self::BareMetal => "none",
            Self::Kvm => "kvm",
            Self::Qemu => "qemu",
            Self::VirtualBox => "virtualbox",
            Self::VMware => "vmware",
            Self::HyperV => "hyperv",
            Self::Xen => "xen",
            Self::Bhyve => "bhyve",
            Self::Unknown(_) => "unknown_vm",
        }
    }

    /// Понятное человеку наименование платформы с учетом локализации.
    pub const fn display_name(&self, lang_ru: bool) -> &'static str {
        if lang_ru {
            match self {
                Self::BareMetal => "Физическое оборудование (Bare Metal)",
                Self::Kvm => "KVM / Proxmox VE",
                Self::Qemu => "QEMU TCG Эмулятор",
                Self::VirtualBox => "Oracle VM VirtualBox",
                Self::VMware => "VMware Workstation / ESXi",
                Self::HyperV => "Microsoft Hyper-V / Azure",
                Self::Xen => "Xen Hypervisor",
                Self::Bhyve => "FreeBSD Bhyve",
                Self::Unknown(_) => "Неизвестный гипервизор",
            }
        } else {
            match self {
                Self::BareMetal => "Physical Hardware (Bare Metal)",
                Self::Kvm => "KVM / Proxmox VE",
                Self::Qemu => "QEMU TCG Emulator",
                Self::VirtualBox => "Oracle VM VirtualBox",
                Self::VMware => "VMware Workstation / ESXi",
                Self::HyperV => "Microsoft Hyper-V / Azure",
                Self::Xen => "Xen Hypervisor",
                Self::Bhyve => "FreeBSD Bhyve",
                Self::Unknown(_) => "Unknown Hypervisor",
            }
        }
    }

    /// Проверяет, запущено ли ядро в любой виртуальной среде.
    pub const fn is_virtual(&self) -> bool {
        !matches!(self, Self::BareMetal)
    }
}

impl fmt::Display for HypervisorType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}

/// Распознает тип гипервизора по 12-байтной сигнатуре CPUID leaf 0x4000_0000.
pub fn parse_signature(sig: &[u8; 12]) -> HypervisorType {
    // KVM возвращает "KVMKVMKVM\0\0\0"
    if sig.starts_with(b"KVMKVMKVM") {
        return HypervisorType::Kvm;
    }
    // QEMU в режиме чистого программного TCG эмулятора без KVM
    if sig == b"TCGTCGTCGTCG" {
        return HypervisorType::Qemu;
    }
    // Oracle VM VirtualBox
    if sig == b"VBoxVBoxVBox" {
        return HypervisorType::VirtualBox;
    }
    // VMware (Workstation, Player, ESXi)
    if sig == b"VMwareVMware" {
        return HypervisorType::VMware;
    }
    // Microsoft Hyper-V (Windows Server, Azure, Win10/11)
    if sig == b"Microsoft Hv" {
        return HypervisorType::HyperV;
    }
    // Xen Paravirtualization
    if sig == b"XenVMMXenVMM" {
        return HypervisorType::Xen;
    }
    // FreeBSD Bhyve
    if sig == b"bhyve bhyve " {
        return HypervisorType::Bhyve;
    }

    HypervisorType::Unknown(*sig)
}

/// Выполняет аппаратный опрос CPUID для выявления запущенного гипервизора.
pub fn detect_hypervisor() -> HypervisorType {
    let leaf1 = __cpuid(1);
    let hypervisor_present = (leaf1.ecx & (1 << 31)) != 0;

    if !hypervisor_present {
        return HypervisorType::BareMetal;
    }

    let leaf_vm = __cpuid(0x4000_0000);
    let mut sig = [0u8; 12];
    sig[0..4].copy_from_slice(&leaf_vm.ebx.to_le_bytes());
    sig[4..8].copy_from_slice(&leaf_vm.ecx.to_le_bytes());
    sig[8..12].copy_from_slice(&leaf_vm.edx.to_le_bytes());

    parse_signature(&sig)
}
