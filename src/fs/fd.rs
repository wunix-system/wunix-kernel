use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use spin::RwLock;

use crate::fs::handle::{FileHandle, StdinHandle, StdoutStderrHandle};

pub const FD_CLOEXEC: u32 = 1;

#[derive(Clone)]
pub struct FdEntry {
    pub handle: Arc<dyn FileHandle>,
    pub flags: u32,
}

pub type FdMap = BTreeMap<u64, FdEntry>;

static PROCESS_FD_TABLES: RwLock<BTreeMap<u64, Arc<RwLock<FdMap>>>> =
    RwLock::new(BTreeMap::new());

fn default_stdio_map() -> FdMap {
    let mut map = BTreeMap::new();
    map.insert(0, FdEntry {
        handle: Arc::new(StdinHandle) as Arc<dyn FileHandle>,
        flags: 0,
    });
    map.insert(1, FdEntry {
        handle: Arc::new(StdoutStderrHandle) as Arc<dyn FileHandle>,
        flags: 0,
    });
    map.insert(2, FdEntry {
        handle: Arc::new(StdoutStderrHandle) as Arc<dyn FileHandle>,
        flags: 0,
    });
    map
}

pub fn init_stdio() {
    let mut tables = PROCESS_FD_TABLES.write();
    let map = default_stdio_map();
    tables.insert(1, Arc::new(RwLock::new(map)));
}

pub fn clone_fd_table(source_tgid: u64, target_tgid: u64) {
    let mut tables = PROCESS_FD_TABLES.write();
    let new_map = if let Some(src) = tables.get(&source_tgid) {
        src.read().clone()
    } else {
        default_stdio_map()
    };
    tables.insert(target_tgid, Arc::new(RwLock::new(new_map)));
}

pub fn share_fd_table(source_tgid: u64, target_tgid: u64) {
    let mut tables = PROCESS_FD_TABLES.write();
    if let Some(src) = tables.get(&source_tgid) {
        let shared = Arc::clone(src);
        tables.insert(target_tgid, shared);
    } else {
        let map = Arc::new(RwLock::new(default_stdio_map()));
        tables.insert(source_tgid, Arc::clone(&map));
        tables.insert(target_tgid, map);
    }
}

pub fn remove_process_fd_table(tgid: u64) {
    if let Some(table) = PROCESS_FD_TABLES.write().remove(&tgid) {
        table.write().clear();
    }
}

pub fn close_cloexec_fds_for(tgid: u64) {
    let tables = PROCESS_FD_TABLES.read();
    if let Some(table) = tables.get(&tgid) {
        table.write().retain(|_, entry| (entry.flags & FD_CLOEXEC) == 0);
    }
}

pub fn close_cloexec_fds() {
    close_cloexec_fds_for(crate::task::current_thread_tgid());
}

fn get_current_process_fd_table() -> Arc<RwLock<FdMap>> {
    let tgid = crate::task::current_thread_tgid();
    {
        let tables = PROCESS_FD_TABLES.read();
        if let Some(table) = tables.get(&tgid) {
            return Arc::clone(table);
        }
    }

    let mut tables = PROCESS_FD_TABLES.write();
    if let Some(table) = tables.get(&tgid) {
        return Arc::clone(table);
    }

    let parent_tgid = crate::task::current_thread_parent_tgid();
    let new_map = if parent_tgid != tgid && parent_tgid != 0 {
        if let Some(parent_table) = tables.get(&parent_tgid) {
            parent_table.read().clone()
        } else {
            default_stdio_map()
        }
    } else {
        default_stdio_map()
    };

    let arc = Arc::new(RwLock::new(new_map));
    tables.insert(tgid, Arc::clone(&arc));
    arc
}

pub fn get_file_handle(fd: u64) -> Option<Arc<dyn FileHandle>> {
    let table = get_current_process_fd_table();
    table.read().get(&fd).map(|e| Arc::clone(&e.handle))
}

pub fn get_fd_flags(fd: u64) -> Option<u32> {
    let table = get_current_process_fd_table();
    table.read().get(&fd).map(|e| e.flags)
}

pub fn set_fd_flags(fd: u64, flags: u32) -> bool {
    let table = get_current_process_fd_table();
    let mut map = table.write();
    if let Some(entry) = map.get_mut(&fd) {
        entry.flags = flags;
        true
    } else {
        false
    }
}

pub fn insert_file_handle(handle: Arc<dyn FileHandle>) -> u64 {
    insert_file_handle_flags(handle, 3, 0)
}

pub fn insert_file_handle_from(handle: Arc<dyn FileHandle>, min_fd: u64) -> u64 {
    insert_file_handle_flags(handle, min_fd, 0)
}

pub fn insert_file_handle_flags(handle: Arc<dyn FileHandle>, min_fd: u64, flags: u32) -> u64 {
    let table = get_current_process_fd_table();
    let mut map = table.write();
    let mut candidate = min_fd;
    while map.contains_key(&candidate) {
        candidate += 1;
    }
    map.insert(candidate, FdEntry { handle, flags });
    candidate
}

pub fn insert_file_handle_at(fd: u64, handle: Arc<dyn FileHandle>) {
    insert_file_handle_at_flags(fd, handle, 0);
}

pub fn insert_file_handle_at_flags(fd: u64, handle: Arc<dyn FileHandle>, flags: u32) {
    let table = get_current_process_fd_table();
    table.write().insert(fd, FdEntry { handle, flags });
}

pub fn remove_file_handle(fd: u64) -> Option<Arc<dyn FileHandle>> {
    let table = get_current_process_fd_table();
    table.write().remove(&fd).map(|e| e.handle)
}
