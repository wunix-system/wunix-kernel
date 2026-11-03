pub const VMWARE_BACKDOOR_PORT: u16 = 0x5658;
pub const VMWARE_MAGIC: u32 = 0x564D_5868;

pub const VMWARE_CMD_GET_VERSION: u32 = 10;
pub const VMWARE_CMD_GET_TIME: u32 = 23;
pub const VMWARE_CMD_VMMOUSE_DATA: u32 = 39;
pub const VMWARE_CMD_VMMOUSE_STATUS: u32 = 40;
pub const VMWARE_CMD_VMMOUSE_COMMAND: u32 = 41;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackdoorResult {
    pub eax: u32,
    pub ebx: u32,
    pub ecx: u32,
    pub edx: u32,
}

#[inline]
pub fn backdoor_call(cmd: u32, arg: u32) -> BackdoorResult {
    let mut eax = VMWARE_MAGIC;
    let mut temp_rbx = arg as u64;
    let mut ecx = cmd;
    let mut edx = VMWARE_BACKDOOR_PORT as u32;

    // LLVM резервирует регистр rbx внутри компилятора под базовые указатели/PIC,
    // поэтому напрямую передать "ebx" в asm! нельзя.
    // Используем атомарный обмен xchg rbx, {temp} до и после команды ввода in:
    // это сохраняет значение rbx хоста и передает наш параметр в виртуальный порт VMware.
    unsafe {
        core::arch::asm!(
            "xchg rbx, {temp}",
            "in eax, dx",
            "xchg rbx, {temp}",
            inlateout("eax") eax,
            temp = inout(reg) temp_rbx,
            inlateout("ecx") ecx,
            inlateout("edx") edx,
            options(nomem, nostack)
        );
    }

    BackdoorResult {
        eax,
        ebx: temp_rbx as u32,
        ecx,
        edx,
    }
}

pub fn is_vmware_present() -> bool {
    let res = backdoor_call(VMWARE_CMD_GET_VERSION, 0);
    res.eax == VMWARE_MAGIC || res.ebx == VMWARE_MAGIC
}

pub fn get_host_time_seconds() -> Option<u32> {
    if !is_vmware_present() {
        return None;
    }
    let res = backdoor_call(VMWARE_CMD_GET_TIME, 0);
    if res.eax != 0 && res.eax != 0xFFFF_FFFF {
        Some(res.eax)
    } else {
        None
    }
}
