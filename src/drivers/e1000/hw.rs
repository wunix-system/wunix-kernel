// Описания регистров, структур дескрипторов и битовых масок Intel 82540EM/82545EM Gigabit Ethernet.

pub const INTEL_PCI_VENDOR_ID: u16 = 0x8086;

// Поддерживаемые микросхемы семейства Intel PRO/1000, e1000e и igb
pub const E1000_DEV_82540EM: u16 = 0x100E; // VirtualBox & QEMU default NIC
pub const E1000_DEV_82545EM: u16 = 0x100F; // VMware ESXi / Workstation compatible NIC
pub const E1000_DEV_82543GC: u16 = 0x1004; // Классический серверный адаптер Intel Gigabit
pub const E1000_DEV_82571EB: u16 = 0x105E; // Двухпортовый PCIe контроллер
pub const E1000_DEV_82572EI: u16 = 0x107D; // Серверный PCIe адаптер
pub const E1000_DEV_82573L: u16 = 0x109A;  // Мобильный PCIe гигабитный чип
pub const E1000_DEV_82574L: u16 = 0x10D3;  // PCIe Gigabit контроллер
pub const E1000_DEV_82579LM: u16 = 0x1502; // Корпоративный гигабитный PHY
pub const E1000_DEV_82579V: u16 = 0x1503;  // Потребительский чипсетный PHY
pub const E1000_DEV_I217LM: u16 = 0x153A;  // Haswell Gigabit
pub const E1000_DEV_I217V: u16 = 0x153B;
pub const E1000_DEV_I218V: u16 = 0x15A1;   // Broadwell Gigabit
pub const E1000_DEV_I219LM: u16 = 0x156F;  // Skylake/KabyLake Gigabit
pub const E1000_DEV_I219V: u16 = 0x1570;
pub const E1000_DEV_I219V2: u16 = 0x15B8;
pub const E1000_DEV_82575EB: u16 = 0x10A7; // Семейство igb: многоочередной серверный адаптер
pub const E1000_DEV_82576: u16 = 0x10C9;   // igb PCIe Gen2
pub const E1000_DEV_I350: u16 = 0x1521;    // igb Quad-Port Server
pub const E1000_DEV_I210: u16 = 0x1533;    // igb промышленный чип
pub const E1000_DEV_I211: u16 = 0x1539;    // igb потребительский чип PCIe
pub const E1000_DEV_I225_LM: u16 = 0x15F2; // Intel 2.5GbE I225-LM
pub const E1000_DEV_I225_V: u16 = 0x15F3;  // Intel 2.5GbE I225-V
pub const E1000_DEV_I226_V: u16 = 0x125B;  // Intel 2.5GbE I226-V
pub const E1000_DEV_I226_LM: u16 = 0x125C; // Intel 2.5GbE I226-LM

// Смещения регистров MMIO Intel e1000
pub const REG_CTRL: usize = 0x0000;
pub const REG_EERD: usize = 0x0014;
pub const REG_ICR: usize = 0x00C0;
pub const REG_IMC: usize = 0x00D8;
pub const REG_RCTL: usize = 0x0100;
pub const REG_TCTL: usize = 0x0400;
pub const REG_RDBAL: usize = 0x2800;
pub const REG_RDBAH: usize = 0x2804;
pub const REG_RDLEN: usize = 0x2808;
pub const REG_RDH: usize = 0x2810;
pub const REG_RDT: usize = 0x2818;
pub const REG_TDBAL: usize = 0x3800;
pub const REG_TDBAH: usize = 0x3804;
pub const REG_TDLEN: usize = 0x3808;
pub const REG_TDH: usize = 0x3810;
pub const REG_TDT: usize = 0x3818;
pub const REG_MTA: usize = 0x5200;
pub const REG_RAL: usize = 0x5400;
pub const REG_RAH: usize = 0x5404;

// Флаги управления приемом (RCTL)
pub const RCTL_EN: u32 = 1 << 1;        // Включение приемника
pub const RCTL_SBP: u32 = 1 << 2;       // Сохранение поврежденных пакетов
pub const RCTL_UPE: u32 = 1 << 3;       // Прием одноадресных пакетов (Promiscuous)
pub const RCTL_MPE: u32 = 1 << 4;       // Прием многоадресных пакетов
pub const RCTL_BAM: u32 = 1 << 15;      // Прием широковещательных пакетов (Broadcast)
pub const RCTL_BSIZE_2048: u32 = 0;     // Размер приемного буфера 2048 байт
pub const RCTL_SECRC: u32 = 1 << 26;    // Аппаратное отсечение контрольной суммы CRC

// Флаги управления передачей (TCTL)
pub const TCTL_EN: u32 = 1 << 1;        // Включение передатчика
pub const TCTL_PSP: u32 = 1 << 3;       // Автодополнение коротких пакетов до 64 байт
pub const TCTL_CT_SHIFT: u32 = 4;       // Collision Threshold
pub const TCTL_COLD_SHIFT: u32 = 12;    // Collision Distance

// Флаги дескрипторов
pub const RXD_STAT_DD: u8 = 1 << 0;     // Descriptor Done (пакет записан хостом)
pub const TXD_CMD_EOP: u8 = 1 << 0;     // Конец сетевого пакета
pub const TXD_CMD_IFCS: u8 = 1 << 1;    // Вставить аппаратную контрольную сумму FCS
pub const TXD_CMD_RS: u8 = 1 << 3;      // Сообщить статус завершения (Report Status)
pub const TXD_STAT_DD: u8 = 1 << 0;     // Завершение передачи дескриптора

pub const NUM_RX_DESCRIPTORS: usize = 16;
pub const NUM_TX_DESCRIPTORS: usize = 8;
pub const BUFFER_SIZE: usize = 2048;

pub const RX_BUFFERS_OFFSET: usize = 4096;
pub const TX_RING_OFFSET: usize = 512;
pub const TX_BUFFERS_OFFSET: usize = 36864;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct E1000RxDesc {
    pub addr: u64,
    pub length: u16,
    pub checksum: u16,
    pub status: u8,
    pub errors: u8,
    pub special: u16,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct E1000TxDesc {
    pub addr: u64,
    pub length: u16,
    pub cso: u8,
    pub cmd: u8,
    pub status: u8,
    pub css: u8,
    pub special: u16,
}
