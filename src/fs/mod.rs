pub mod block;
pub mod devfs;
pub mod epoll;
pub mod eventfd;
pub mod ext4;
pub mod fat;
pub mod fd;
pub mod ufs;
pub mod handle;
pub mod mount;
pub mod ops;
pub mod path;
pub mod pipe;
pub mod procfs;
pub mod ramfs;
pub mod registry;
pub mod socket;
pub mod tar;
pub mod vfs;

use alloc::sync::Arc;

pub use epoll::EpollInstance;
pub use eventfd::EventFdHandle;
pub use fd::{
    clone_fd_table, close_cloexec_fds, close_cloexec_fds_for, get_fd_flags, get_file_handle,
    init_stdio, insert_file_handle, insert_file_handle_at, insert_file_handle_at_flags,
    insert_file_handle_flags, insert_file_handle_from, remove_file_handle, remove_process_fd_table,
    set_fd_flags, share_fd_table, FD_CLOEXEC,
};
pub use handle::{
    FileHandle, RegularFileHandle, StdinHandle, StdoutStderrHandle, EPOLLERR, EPOLLET, EPOLLHUP,
    EPOLLIN, EPOLLOUT, EPOLLPRI, EPOLLRDHUP, O_CLOEXEC, O_CREAT, O_NONBLOCK, O_RDONLY, O_RDWR,
    O_WRONLY,
};
pub use mount::{list_mounts, mount, umount, MountEntry};
pub use ops::{
    check_permission, create_symlink, mkdir_p, open, read_file, rename, resolve_path, root, unlink,
    write_file,
};
pub use path::{is_windows_subsystem, normalize_path, translate_path};
pub use pipe::{create_socketpair, PipeBuffer, PipeReadHandle, PipeWriteHandle, SocketPairHandle};
pub use procfs::{pci_device_name, pci_driver_name, pci_vendor_name, refresh_proc_files};
pub use ramfs::{RamFs, RamInode};
pub use registry::{
    reg_create_key, reg_delete_value, reg_enum_keys, reg_enum_values, reg_open_key,
    reg_query_value, reg_set_value, Hive, RegError, RegType, RegValue,
};
pub use socket::{create_socket, SockAddrIn, SocketFileHandle};
pub use tar::unpack_tar;
pub use vfs::{DirEntry, FileSystem, Inode, InodeType, Metadata, VfsError};

/// Инициализирует подсистему виртуальной файловой системы (VFS):
/// 1. Монтирует RamFS в качестве корня '/'.
/// 2. Создает гибридный скелет директорий Unix/NT.
/// 3. Распаковывает архив initramfs (если передан загрузчиком).
pub fn init(initramfs_data: Option<&[u8]>) {
    use crate::i18n::t;

    init_stdio();
    let ramfs = Arc::new(RamFs::new());
    *ops::ROOT_FS.write() = Some(ramfs.clone());
    crate::kboot!(
        "ramfs0",
        "{}",
        t("корневая файловая система памяти смонтирована в /", "root memory filesystem mounted on /")
    );

    // Инициализируем файловую систему устройств DevFS и монтируем в корень /dev
    let devfs_instance = devfs::init();
    let _ = ramfs.root().insert_child("dev", devfs_instance.root_dir());
    crate::kboot!(
        "devfs0",
        "{}",
        t("файловая система устройств смонтирована в /dev", "device filesystem mounted on /dev")
    );

    // Базовый скелет гибридной операционной системы
    const SKELETON_DIRS: &[&str] = &[
        "/bin",
        "/sbin",
        "/boot",
        "/etc",
        "/home",
        "/lib/modules",
        "/mnt",
        "/proc",
        "/proc/bus/pci",
        "/proc/driver",
        "/proc/net",
        "/sys",
        "/sys/bus/pci/devices",
        "/sys/class/graphics/fb0",
        "/sys/class/net",
        "/sys/class/net/lo",
        "/sys/class/net/eth0",
        "/tmp",
        "/usr/bin",
        "/var/log",
        "/win/drives/c/Windows/System32",
        "/win/drives/c/Program Files",
        "/win/registry",
    ];

    for dir in SKELETON_DIRS {
        let _ = mkdir_p(dir);
    }

    // Создаем каталог для пользователей Windows:
    let _ = mkdir_p("/win/drives/c/Users");
    crate::kboot!(
        "vfs",
        "{} ({} {})",
        t("создана структура системных каталогов /proc, /sys, /etc", "created system directory layout /proc, /sys, /etc"),
        SKELETON_DIRS.len() + 1,
        t("узлов", "entries")
    );

    // Если загрузчик передал initramfs — распаковываем его поверх корня
    let root_node = root().expect("vfs: root inode must exist");

    if let Some(bytes) = initramfs_data {
        let size_kb = (bytes.len() + 1023) / 1024;
        match tar::unpack_tar(bytes, &root_node) {
            Ok(count) => {
                crate::kboot!(
                    "initramfs",
                    "{} ({} {}, {} {})",
                    t("архив распакован в /", "archive unpacked to /"),
                    size_kb,
                    t("КБ", "KiB"),
                    count,
                    t("объектов", "entries")
                );
            }
            Err(e) => {
                crate::kboot!(
                    "initramfs",
                    "{}: {:?}",
                    t("ошибка распаковки", "unpack failed"),
                    e
                );
            }
        }
    } else {
        crate::kboot!(
            "initramfs",
            "{}",
            t(
                "образ не передан (прямая загрузка с дискового раздела)",
                "not provided (direct root partition boot)"
            )
        );
    }

    // Сохраняем системный язык в /etc/locale и /etc/environment для пользовательских программ
    let loc = crate::i18n::current_locale();
    let locale_buf = alloc::format!("LANG={}\nLC_ALL={}\n", loc, loc);
    let _ = write_file("/etc/locale", locale_buf.as_bytes());

    let mut env_buf = read_file("/etc/environment")
        .ok()
        .and_then(|b| alloc::string::String::from_utf8(b).ok())
        .unwrap_or_default();
    if !env_buf.contains("LANG=") {
        if !env_buf.is_empty() && !env_buf.ends_with('\n') {
            env_buf.push('\n');
        }
        env_buf.push_str(&alloc::format!("LANG={}\n", loc));
    }
    if !env_buf.contains("LC_ALL=") {
        if !env_buf.is_empty() && !env_buf.ends_with('\n') {
            env_buf.push('\n');
        }
        env_buf.push_str(&alloc::format!("LC_ALL={}\n", loc));
    }
    let _ = write_file("/etc/environment", env_buf.as_bytes());
    crate::kboot!(
        t("локализация", "locale"),
        "{} {}",
        t("активна системная локаль", "system locale"),
        loc
    );

    // Инициализируем блочные устройства (/dev/vda, /dev/vda1, ...)
    block::init();

    // Заполняем системные ветки реестра Windows (HKLM, HKCU, HKCR)
    registry::populate::init_system_registry();
    crate::kboot!(
        t("реестр", "registry"),
        "{}",
        t("инициализированы системные кусты HKLM, HKCU, HKCR", "initialized system hives HKLM, HKCU, HKCR")
    );
}
