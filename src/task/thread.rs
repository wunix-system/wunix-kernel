use alloc::boxed::Box;
use core::sync::atomic::{AtomicU64, Ordering};

pub const STACK_SIZE: usize = 128 * 1024; // 128 КиБ стек ядра для каждого потока (защита от переполнения при FS I/O)

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ThreadId(pub u64);

impl ThreadId {
    pub fn new() -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        ThreadId(NEXT_ID.fetch_add(1, Ordering::Relaxed))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadState {
    Ready,
    Running,
    Blocked,
    Terminated,
}

pub struct Thread {
    pub id: ThreadId,
    pub tgid: u64,
    pub parent_tgid: u64,
    pub pgid: u64,
    pub is_main: bool,
    pub is_sync_child: bool,
    pub state: ThreadState,
    pub stack: Option<Box<[u8]>>,
    pub kernel_stack_top: u64,
    pub rsp: *mut u8,
    pub cr3: u64,
    pub fs_base: u64,
    pub clear_child_tid: u64,
    pub wait_deadline: Option<u64>,
    pub uid: u32,
    pub gid: u32,
    pub euid: u32,
    pub egid: u32,
    pub suid: u32,
    pub sgid: u32,
    pub umask: u32,
}

// Поток эксклюзивно владеет своим стеком и перемещается между контекстами планировщика
unsafe impl Send for Thread {}

impl Thread {
    pub fn new(id: ThreadId, entry_fn: fn()) -> Self {
        let mut stack = alloc::vec![0u8; STACK_SIZE].into_boxed_slice();

        // Стек в x86_64 растет вниз (от старших адресов к младшим)
        let stack_end = stack.as_mut_ptr() as usize + STACK_SIZE;
        // System V AMD64 ABI требует 16-байтного выравнивания границы стека
        let stack_top = stack_end & !0xF;

        // Формируем начальный стек-фрейм (64 байта):
        // При переключении context_switch снимает 6 регистров (48 байт) и делает ret (8 байт),
        // передавая управление на entry_fn.
        let rsp = unsafe {
            let ptr = (stack_top - 64) as *mut u64;
            ptr.add(0).write(0); // r15
            ptr.add(1).write(0); // r14
            ptr.add(2).write(0); // r13
            ptr.add(3).write(0); // r12
            ptr.add(4).write(0); // rbx
            ptr.add(5).write(0); // rbp
            ptr.add(6).write(entry_fn as *const () as usize as u64); // ret -> entry_fn
            ptr.add(7).write(thread_exit as *const () as usize as u64); // ret из entry_fn -> thread_exit
            ptr as *mut u8
        };

        Self {
            id,
            tgid: 1,
            parent_tgid: 1,
            pgid: 1,
            is_main: true,
            is_sync_child: false,
            state: ThreadState::Ready,
            stack: Some(stack),
            kernel_stack_top: stack_top as u64,
            rsp,
            cr3: 0,
            fs_base: 0,
            clear_child_tid: 0,
            wait_deadline: None,
            uid: 0,
            gid: 0,
            euid: 0,
            egid: 0,
            suid: 0,
            sgid: 0,
            umask: 0o022,
        }
    }

    pub fn new_clone(
        id: ThreadId,
        tgid: u64,
        parent_tgid: u64,
        pgid: u64,
        stack: Box<[u8]>,
        kernel_stack_top: u64,
        rsp: *mut u8,
        cr3: u64,
        fs_base: u64,
        clear_child_tid: u64,
        uid: u32,
        gid: u32,
        euid: u32,
        egid: u32,
        suid: u32,
        sgid: u32,
        umask: u32,
    ) -> Self {
        Self {
            id,
            tgid,
            parent_tgid,
            pgid,
            is_main: false,
            is_sync_child: false,
            state: ThreadState::Ready,
            stack: Some(stack),
            kernel_stack_top,
            rsp,
            cr3,
            fs_base,
            clear_child_tid,
            wait_deadline: None,
            uid,
            gid,
            euid,
            egid,
            suid,
            sgid,
            umask,
        }
    }

    pub fn new_main() -> Self {
        Self {
            id: ThreadId::new(),
            tgid: 1,
            parent_tgid: 1,
            pgid: 1,
            is_main: true,
            is_sync_child: false,
            state: ThreadState::Running,
            stack: None,
            kernel_stack_top: crate::arch::x86_64::gdt::get_kernel_syscall_stack_top(),
            rsp: core::ptr::null_mut(),
            cr3: 0,
            fs_base: 0,
            clear_child_tid: 0,
            wait_deadline: None,
            uid: 0,
            gid: 0,
            euid: 0,
            egid: 0,
            suid: 0,
            sgid: 0,
            umask: 0o022,
        }
    }
}

extern "C" fn thread_exit() -> ! {
    super::exit();
}
