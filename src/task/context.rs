use core::arch::global_asm;

// Callee-saved регистры согласно System V AMD64 ABI.
// Все остальные регистры (rax, rcx, rdx, rsi, rdi, r8-r11) являются caller-saved
// и компилятор сохраняет их на стеке вызывающей функции до вызова context_switch.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct Context {
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub rbx: u64,
    pub rbp: u64,
}

global_asm!(
    r#"
    .global context_switch
    context_switch:
        push rbp
        push rbx
        push r12
        push r13
        push r14
        push r15
        mov [rdi], rsp      /* Сохраняем rsp текущего потока в *prev_rsp */
        mov rsp, rsi        /* Загружаем rsp следующего потока из next_rsp */
        pop r15
        pop r14
        pop r13
        pop r12
        pop rbx
        pop rbp
        ret                 /* Возврат в точку возобновления следующего потока */
    "#
);

unsafe extern "C" {
    pub fn context_switch(prev_rsp: *mut *mut u8, next_rsp: *const u8);
}
