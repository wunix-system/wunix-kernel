use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use spin::RwLock;

use crate::fs::handle::{FileHandle, StdinHandle, StdoutStderrHandle};

pub type FdMap = BTreeMap<u64, Arc<dyn FileHandle>>;

static PROCESS_FD_TABLES: RwLock<BTreeMap<u64, Arc<RwLock<FdMap>>>> =
    RwLock::new(BTreeMap::new());

fn default_stdio_map() -> FdMap {
    let mut map = BTreeMap::new();
    map.insert(0, Arc::new(StdinHandle) as Arc<dyn FileHandle>);
    map.insert(1, Arc::new(StdoutStderrHandle) as Arc<dyn FileHandle>);
    map.insert(2, Arc::new(StdoutStderrHandle) as Arc<dyn FileHandle>);
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
        // Очищаем таблицу дескрипторов процесса. При освобождении ссылок Arc
        // для ресурсов, у которых счетчик ссылок достигает нуля, автоматически
        // срабатывает деструктор Drop (RAII), не затрагивая параллельные процессы.
        table.write().clear();
    }
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
    table.read().get(&fd).cloned()
}

pub fn insert_file_handle(handle: Arc<dyn FileHandle>) -> u64 {
    let table = get_current_process_fd_table();
    let mut map = table.write();
    let mut candidate = 3u64;
    while map.contains_key(&candidate) {
        candidate += 1;
    }
    map.insert(candidate, handle);
    candidate
}

pub fn insert_file_handle_at(fd: u64, handle: Arc<dyn FileHandle>) {
    let table = get_current_process_fd_table();
    table.write().insert(fd, handle);
}

pub fn remove_file_handle(fd: u64) -> Option<Arc<dyn FileHandle>> {
    let table = get_current_process_fd_table();
    table.write().remove(&fd)
}
