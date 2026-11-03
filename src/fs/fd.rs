use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use spin::RwLock;

use crate::fs::handle::{FileHandle, StdinHandle, StdoutStderrHandle};

static GLOBAL_FD_TABLE: RwLock<BTreeMap<u64, Arc<dyn FileHandle>>> =
    RwLock::new(BTreeMap::new());

pub fn init_stdio() {
    let mut table = GLOBAL_FD_TABLE.write();
    table.insert(0, Arc::new(StdinHandle));
    table.insert(1, Arc::new(StdoutStderrHandle));
    table.insert(2, Arc::new(StdoutStderrHandle));
}

pub fn get_file_handle(fd: u64) -> Option<Arc<dyn FileHandle>> {
    GLOBAL_FD_TABLE.read().get(&fd).cloned()
}

pub fn insert_file_handle(handle: Arc<dyn FileHandle>) -> u64 {
    let mut table = GLOBAL_FD_TABLE.write();
    let mut candidate = 3u64;
    while table.contains_key(&candidate) {
        candidate += 1;
    }
    table.insert(candidate, handle);
    candidate
}

pub fn insert_file_handle_at(fd: u64, handle: Arc<dyn FileHandle>) {
    GLOBAL_FD_TABLE.write().insert(fd, handle);
}

pub fn remove_file_handle(fd: u64) -> Option<Arc<dyn FileHandle>> {
    if fd < 3 {
        return None;
    }
    let handle = GLOBAL_FD_TABLE.write().remove(&fd);
    if let Some(ref h) = handle {
        h.close();
    }
    handle
}
