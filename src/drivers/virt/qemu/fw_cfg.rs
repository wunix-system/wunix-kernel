use x86_64::instructions::port::Port;

// Порты I/O для управления и передачи данных через шлюз QEMU fw_cfg
const FW_CFG_PORT_SEL: u16 = 0x0510;
const FW_CFG_PORT_DATA: u16 = 0x0511;

// Ключи стандартных записей конфигурации виртуальной машины
const FW_CFG_SIGNATURE: u16 = 0x0000;
const FW_CFG_RAM_SIZE: u16 = 0x0003;
const FW_CFG_NB_CPUS: u16 = 0x0005;

/// Драйвер синтетического интерфейса QEMU / KVM Firmware Configuration (fw_cfg).
/// Позволяет гостевой ОС напрямую запрашивать у гипервизора параметры виртуальной машины
/// (объем памяти, число виртуальных ядер, UUID хоста) без симуляции медленного BIOS.
pub struct QemuFwCfg {
    port_sel: Port<u16>,
    port_data: Port<u8>,
}

impl QemuFwCfg {
    pub const fn new() -> Self {
        Self {
            port_sel: Port::new(FW_CFG_PORT_SEL),
            port_data: Port::new(FW_CFG_PORT_DATA),
        }
    }

    /// Проверяет наличие fw_cfg: считывает 4-байтовую сигнатуру "QEMU" по селектору 0x0000.
    pub fn is_present(&mut self) -> bool {
        let mut sig = [0u8; 4];
        self.read_entry(FW_CFG_SIGNATURE, &mut sig);
        &sig == b"QEMU"
    }

    /// Читает сырую последовательность байт по указанному селектору fw_cfg.
    pub fn read_entry(&mut self, key: u16, buf: &mut [u8]) {
        unsafe {
            self.port_sel.write(key);
            for byte in buf.iter_mut() {
                *byte = self.port_data.read();
            }
        }
    }

    /// Возвращает физический объем RAM, выделенный виртуальной машине гипервизором.
    pub fn ram_size(&mut self) -> Option<u64> {
        if !self.is_present() {
            return None;
        }
        let mut bytes = [0u8; 8];
        self.read_entry(FW_CFG_RAM_SIZE, &mut bytes);
        Some(u64::from_le_bytes(bytes))
    }

    /// Возвращает количество доступных ядер виртуального процессора (SMP vCPUs).
    pub fn cpu_count(&mut self) -> Option<u16> {
        if !self.is_present() {
            return None;
        }
        let mut bytes = [0u8; 2];
        self.read_entry(FW_CFG_NB_CPUS, &mut bytes);
        Some(u16::from_le_bytes(bytes))
    }
}
