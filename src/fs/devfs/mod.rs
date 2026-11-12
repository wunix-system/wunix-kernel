pub mod dir;
pub mod fb;
pub mod handle;
pub mod inode;
pub mod input;
pub mod loopdev;
pub mod mem;
pub mod rtc;
pub mod tty;
pub mod types;

use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use spin::{Mutex, RwLock};

pub use dir::DevDirInode;
pub use handle::DevFileHandle;
pub use inode::DevInode;
pub use types::*;

use crate::drivers::BlockDevice;
use crate::fs::ramfs::RamInode;
use crate::fs::vfs::{FileSystem, Inode};

static DEVFS_INSTANCE: RwLock<Option<Arc<DevFs>>> = RwLock::new(None);

/// Виртуальная файловая система устройств DevFS (монтируется в /dev).
/// Обеспечивает честную регистрацию символьных драйверов ядра, терминалов,
/// кадрового буфера дисплея, часов RTC и блочных устройств без строкового хардкода.
pub struct DevFs {
    root: Arc<DevDirInode>,
    input_dir: Arc<DevDirInode>,
    pts_dir: Arc<DevDirInode>,
    shm_dir: Arc<DevDirInode>,
}

impl DevFs {
    pub fn new() -> Self {
        let root = Arc::new(DevDirInode::new());
        let input_dir = Arc::new(DevDirInode::new());
        let pts_dir = Arc::new(DevDirInode::new());
        let shm_dir = Arc::new(DevDirInode::new());

        root.insert("input", input_dir.clone());
        root.insert("pts", pts_dir.clone());
        root.insert("shm", shm_dir.clone());

        Self {
            root,
            input_dir,
            pts_dir,
            shm_dir,
        }
    }

    pub fn root_dir(&self) -> Arc<DevDirInode> {
        self.root.clone()
    }

    pub fn shm_dir(&self) -> Arc<DevDirInode> {
        self.shm_dir.clone()
    }

    pub fn register_char(&self, path: &str, major: u32, minor: u32, op: Arc<dyn CharDeviceOp>, perms: u16) {
        let rdev = makedev(major, minor);
        let node = Arc::new(DevInode::new_char(rdev, perms, op));

        if let Some(sub) = path.strip_prefix("input/") {
            self.input_dir.insert(sub, node);
        } else if let Some(sub) = path.strip_prefix("pts/") {
            self.pts_dir.insert(sub, node);
        } else {
            self.root.insert(path, node);
        }
    }

    pub fn register_block(&self, name: &str, major: u32, minor: u32, dev: Arc<Mutex<dyn BlockDevice>>) {
        let rdev = makedev(major, minor);
        let node = Arc::new(DevInode::new_block(rdev, 0o660, dev));
        self.root.insert(name, node);
    }
}

impl FileSystem for DevFs {
    fn root(&self) -> Arc<dyn Inode> {
        self.root.clone()
    }

    fn name(&self) -> &'static str {
        "devfs"
    }
}

/// Инициализирует файловую систему устройств DevFS и заполняет стандартные узлы /dev
pub fn init() -> Arc<DevFs> {
    let devfs = Arc::new(DevFs::new());

    // 1. Устройства памяти и генераторы данных (Major 1)
    devfs.register_char("null", MEM_MAJOR, 3, Arc::new(mem::NullDev), 0o666);
    devfs.register_char("zero", MEM_MAJOR, 5, Arc::new(mem::ZeroDev), 0o666);
    devfs.register_char("full", MEM_MAJOR, 7, Arc::new(mem::FullDev), 0o666);
    devfs.register_char("random", MEM_MAJOR, 8, Arc::new(mem::RandomDev), 0o666);
    devfs.register_char("urandom", MEM_MAJOR, 9, Arc::new(mem::RandomDev), 0o666);
    devfs.register_char("mem", MEM_MAJOR, 1, Arc::new(mem::MemDev), 0o600);
    devfs.register_char("kmem", MEM_MAJOR, 2, Arc::new(mem::KmemDev), 0o600);
    devfs.register_char("port", MEM_MAJOR, 4, Arc::new(mem::PortDev), 0o600);
    devfs.register_char("kmsg", MEM_MAJOR, 11, Arc::new(mem::KmsgDev), 0o644);

    // 2. Терминалы и консоли (Major 4 и 5)
    devfs.register_char("tty", TTYAUX_MAJOR, 0, Arc::new(tty::TtyDev), 0o666);
    devfs.register_char("console", TTYAUX_MAJOR, 1, Arc::new(tty::ConsoleDev), 0o620);
    devfs.register_char("ptmx", TTYAUX_MAJOR, 2, Arc::new(tty::PtmxDev), 0o666);
    devfs.register_char("tty0", TTY_MAJOR, 0, Arc::new(tty::ConsoleDev), 0o620);
    for i in 1..=4 {
        let name = format!("tty{}", i);
        devfs.register_char(&name, TTY_MAJOR, i as u32, Arc::new(tty::VirtualConsoleDev { index: i }), 0o620);
    }
    devfs.register_char("ttyS0", TTY_MAJOR, 64, Arc::new(tty::TtySDev { base_port: 0x3F8 }), 0o660);
    devfs.register_char("ttyS1", TTY_MAJOR, 65, Arc::new(tty::TtySDev { base_port: 0x2F8 }), 0o660);

    // 3. Кадровый буфер дисплея (Major 29)
    devfs.register_char("fb0", FB_MAJOR, 0, Arc::new(fb::FbDev), 0o660);

    // 4. Часы реального времени CMOS RTC (Major 254)
    devfs.register_char("rtc0", RTC_MAJOR, 0, Arc::new(rtc::RtcDev), 0o660);
    devfs.root.insert("rtc", Arc::new(RamInode::new_symlink("rtc0")));

    // 5. Устройства ввода (Major 13)
    devfs.register_char("input/mice", INPUT_MAJOR, 63, Arc::new(input::MiceDev), 0o660);
    devfs.register_char("input/mouse0", INPUT_MAJOR, 32, Arc::new(input::MiceDev), 0o660);
    devfs.register_char("input/event0", INPUT_MAJOR, 64, Arc::new(input::EventDev), 0o660);

    // 6. Псевдотерминалы PTS
    devfs.pts_dir.insert("ptmx", Arc::new(RamInode::new_symlink("../ptmx")));

    // 7. Виртуальные блочные loop-устройства (Major 7) и ramdisk (Major 1)
    for i in 0..8 {
        let loop_name = format!("loop{}", i);
        let loop_dev = loopdev::create_loop_device(i);
        devfs.register_block(&loop_name, LOOP_MAJOR, i, loop_dev);
    }
    let ram_dev = loopdev::create_loop_device(100);
    devfs.register_block("ram0", MEM_MAJOR, 0, ram_dev);

    // 8. Стандартные символические ссылки POSIX
    devfs.root.insert("stdin", Arc::new(RamInode::new_symlink("/proc/self/fd/0")));
    devfs.root.insert("stdout", Arc::new(RamInode::new_symlink("/proc/self/fd/1")));
    devfs.root.insert("stderr", Arc::new(RamInode::new_symlink("/proc/self/fd/2")));
    devfs.root.insert("fd", Arc::new(RamInode::new_symlink("/proc/self/fd")));
    devfs.root.insert("core", Arc::new(RamInode::new_symlink("/proc/kcore")));

    *DEVFS_INSTANCE.write() = Some(devfs.clone());
    devfs
}

pub fn get_devfs() -> Option<Arc<DevFs>> {
    DEVFS_INSTANCE.read().clone()
}

/// Регистрирует блочный диск ядра (/dev/vda, /dev/sda, разделы vda1 и т.д.) в дереве DevFS
pub fn register_block_device(name: &str, dev: Arc<Mutex<dyn BlockDevice>>) {
    if let Some(devfs) = get_devfs() {
        let clean = name.trim_start_matches("/dev/");
        let major = if clean.starts_with("vd") {
            VIRTBLK_MAJOR
        } else if clean.starts_with("sd") {
            SCSI_DISK_MAJOR
        } else if clean.starts_with("loop") {
            LOOP_MAJOR
        } else {
            VIRTBLK_MAJOR
        };

        // Вычисляем младший номер minor (например, "vda" -> 0, "vda1" -> 1)
        let minor = clean
            .chars()
            .rev()
            .take_while(|c| c.is_ascii_digit())
            .collect::<String>()
            .chars()
            .rev()
            .collect::<String>()
            .parse::<u32>()
            .unwrap_or(0);

        devfs.register_block(clean, major, minor, dev);
    }
}

/// Проверяет, является ли узел устройством (CharDevice или BlockDevice), и создает дескриптор открытого файла DevFileHandle
pub fn open_device_node(node: &Arc<dyn Inode>, flags: u32, path: Option<String>) -> Option<Arc<dyn crate::fs::handle::FileHandle>> {
    let meta = node.metadata();
    if meta.itype == crate::fs::InodeType::CharDevice || meta.itype == crate::fs::InodeType::BlockDevice {
        return Some(Arc::new(DevFileHandle::new(node.clone(), flags, path)));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_makedev_major_minor() {
        let dev = makedev(1, 3);
        assert_eq!(major(dev), 1);
        assert_eq!(minor(dev), 3);

        let dev2 = makedev(254, 64);
        assert_eq!(major(dev2), 254);
        assert_eq!(minor(dev2), 64);
    }

    #[test_case]
    fn test_devfs_standard_devices_lookup() {
        let devfs = init();
        let root = devfs.root_dir();

        // Проверяем наличие null, zero, full, urandom, tty, fb0, rtc0
        let null_node = root.lookup("null").expect("null exists");
        assert_eq!(null_node.metadata().itype, crate::fs::InodeType::CharDevice);
        assert_eq!(null_node.metadata().rdev, makedev(1, 3));

        let zero_node = root.lookup("zero").expect("zero exists");
        assert_eq!(zero_node.metadata().itype, crate::fs::InodeType::CharDevice);
        assert_eq!(zero_node.metadata().rdev, makedev(1, 5));

        let full_node = root.lookup("full").expect("full exists");
        assert_eq!(full_node.metadata().itype, crate::fs::InodeType::CharDevice);
        assert_eq!(full_node.metadata().rdev, makedev(1, 7));

        let fb0_node = root.lookup("fb0").expect("fb0 exists");
        assert_eq!(fb0_node.metadata().itype, crate::fs::InodeType::CharDevice);
        assert_eq!(fb0_node.metadata().rdev, makedev(29, 0));

        let rtc0_node = root.lookup("rtc0").expect("rtc0 exists");
        assert_eq!(rtc0_node.metadata().itype, crate::fs::InodeType::CharDevice);
    }

    #[test_case]
    fn test_devfs_mem_devices_behavior() {
        let devfs = init();
        let root = devfs.root_dir();

        // null: чтение всегда 0, запись подтверждается
        let null = root.lookup("null").unwrap();
        let mut buf = [0xAAu8; 16];
        assert_eq!(null.read_at(0, &mut buf).unwrap(), 0);
        assert_eq!(null.write_at(0, b"12345").unwrap(), 5);

        // zero: чтение заполняет нулями
        let zero = root.lookup("zero").unwrap();
        assert_eq!(zero.read_at(0, &mut buf).unwrap(), 16);
        assert_eq!(buf, [0u8; 16]);

        // full: чтение дает нули, запись возвращает ошибку переполнения
        let full = root.lookup("full").unwrap();
        assert_eq!(full.read_at(0, &mut buf).unwrap(), 16);
        assert!(full.write_at(0, b"test").is_err());
    }
}
