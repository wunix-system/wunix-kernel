pub mod block;
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
    clone_fd_table, get_file_handle, init_stdio, insert_file_handle, insert_file_handle_at,
    remove_file_handle, remove_process_fd_table, share_fd_table,
};
pub use handle::{
    FileHandle, RegularFileHandle, StdinHandle, StdoutStderrHandle, EPOLLERR, EPOLLET, EPOLLHUP,
    EPOLLIN, EPOLLOUT, EPOLLPRI, EPOLLRDHUP, O_CLOEXEC, O_CREAT, O_NONBLOCK, O_RDONLY, O_RDWR,
    O_WRONLY,
};
pub use mount::{list_mounts, mount, umount, MountEntry};
pub use ops::{
    check_permission, create_symlink, mkdir_p, open, read_file, resolve_path, root, unlink,
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

use crate::serial_println;

/// Инициализирует подсистему виртуальной файловой системы (VFS):
/// 1. Монтирует RamFS в качестве корня '/'.
/// 2. Создает гибридный скелет директорий Unix/NT.
/// 3. Распаковывает архив initramfs (если передан загрузчиком).
pub fn init(initramfs_data: Option<&[u8]>) {
    init_stdio();
    let ramfs = Arc::new(RamFs::new());
    *ops::ROOT_FS.write() = Some(ramfs);

    // Базовый скелет гибридной операционной системы
    const SKELETON_DIRS: &[&str] = &[
        "/bin",
        "/sbin",
        "/boot",
        "/dev",
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
        if let Err(e) = mkdir_p(dir) {
            serial_println!("vfs: failed to create skeleton dir '{}': {:?}", dir, e);
        }
    }

    // Создаем каталог для пользователей Windows:
    if let Err(e) = mkdir_p("/win/drives/c/Users") {
        serial_println!("vfs: failed to create /win/drives/c/Users: {:?}", e);
    }

    // Создаем стандартные псевдоустройства /dev
    let _ = write_file("/dev/null", &[]);
    let _ = write_file("/dev/zero", &[]);
    let _ = write_file("/dev/urandom", &[]);
    let _ = write_file("/dev/random", &[]);
    let _ = write_file("/dev/tty", &[]);
    let _ = write_file("/dev/tty0", &[]);
    let _ = write_file("/dev/console", &[]);

    // Если Limine передал initramfs — распаковываем его поверх корня
    let root_node = root().expect("vfs: root inode must exist");

    if let Some(bytes) = initramfs_data {
        match tar::unpack_tar(bytes, &root_node) {
            Ok(count) => {
                crate::klog!(
                    "  {}: RamFS смонтирована в / ({} файлов)",
                    crate::i18n::t("VFS", "VFS"),
                    count
                );
            }
            Err(e) => {
                crate::klog!(
                    "  {}: ошибка распаковки initramfs: {:?}",
                    crate::i18n::t("VFS", "VFS"),
                    e
                );
            }
        }
    } else {
        crate::klog!(
            "  {}: RamFS смонтирована в / (initramfs отсутствует)",
            crate::i18n::t("VFS", "VFS")
        );
    }

    // Сохраняем системный язык в /etc/locale для пользовательских программ
    let lang_str = match crate::i18n::current_language() {
        crate::i18n::Language::Russian => "LANG=ru\n",
        crate::i18n::Language::English => "LANG=en\n",
    };
    let _ = write_file("/etc/locale", lang_str.as_bytes());

    // Инициализируем блочные устройства (/dev/vda, /dev/vda1, ...)
    block::init();

    // Заполняем системные ветки реестра Windows (HKLM, HKCU, HKCR)
    registry::populate::init_system_registry();
}
