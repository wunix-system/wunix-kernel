use spin::Mutex;
use x86_64::instructions::interrupts;
use x86_64::instructions::port::Port;

use super::PciAddress;

// Порты механизма конфигурации PCI Mechanism #1
const PCI_CONFIG_ADDRESS: u16 = 0x0CF8;
const PCI_CONFIG_DATA: u16 = 0x0CFC;

// Защита от гонок при доступе к портам 0xCF8/0xCFC.
// Запись адреса и чтение/запись данных должны быть строго атомарными,
// иначе прерывание или другой поток собьют выбранный регистр.
static PCI_LOCK: Mutex<()> = Mutex::new(());

/// Вычисляет 32-битное значение для порта CONFIG_ADDRESS (0xCF8).
///
/// Бит 31: Enable (1)
/// Биты 23..16: Номер шины (0..255)
/// Биты 15..11: Номер устройства (0..31)
/// Биты 10..8:  Номер функции (0..7)
/// Биты 7..2:   Смещение регистра (выровнено по 4 байтам)
#[inline]
pub const fn make_config_address(addr: PciAddress, offset: u8) -> u32 {
    0x8000_0000
        | ((addr.bus as u32) << 16)
        | (((addr.device & 0x1F) as u32) << 11)
        | (((addr.function & 0x07) as u32) << 8)
        | ((offset & 0xFC) as u32)
}

/// Читает 32-битное двойное слово из пространства конфигурации PCI.
pub fn read_u32(addr: PciAddress, offset: u8) -> u32 {
    interrupts::without_interrupts(|| {
        let _guard = PCI_LOCK.lock();
        unsafe {
            let mut addr_port = Port::<u32>::new(PCI_CONFIG_ADDRESS);
            let mut data_port = Port::<u32>::new(PCI_CONFIG_DATA);

            addr_port.write(make_config_address(addr, offset));
            data_port.read()
        }
    })
}

/// Читает 16-битное слово из пространства конфигурации PCI.
///
/// Читает 32 бита и сдвигает результат в зависимости от выравнивания смещения,
/// так как некоторые чипсеты не поддерживают 16-битный I/O доступ к порту 0xCFC.
pub fn read_u16(addr: PciAddress, offset: u8) -> u16 {
    let dword = read_u32(addr, offset);
    let shift = (offset & 2) * 8;
    ((dword >> shift) & 0xFFFF) as u16
}

/// Читает 8-битный байт из пространства конфигурации PCI.
pub fn read_u8(addr: PciAddress, offset: u8) -> u8 {
    let dword = read_u32(addr, offset);
    let shift = (offset & 3) * 8;
    ((dword >> shift) & 0xFF) as u8
}

/// Записывает 32-битное двойное слово в пространство конфигурации PCI.
pub fn write_u32(addr: PciAddress, offset: u8, val: u32) {
    interrupts::without_interrupts(|| {
        let _guard = PCI_LOCK.lock();
        unsafe {
            let mut addr_port = Port::<u32>::new(PCI_CONFIG_ADDRESS);
            let mut data_port = Port::<u32>::new(PCI_CONFIG_DATA);

            addr_port.write(make_config_address(addr, offset));
            data_port.write(val);
        }
    })
}

/// Записывает 16-битное слово в пространство конфигурации PCI через цикл Read-Modify-Write.
pub fn write_u16(addr: PciAddress, offset: u8, val: u16) {
    interrupts::without_interrupts(|| {
        let _guard = PCI_LOCK.lock();
        unsafe {
            let mut addr_port = Port::<u32>::new(PCI_CONFIG_ADDRESS);
            let mut data_port = Port::<u32>::new(PCI_CONFIG_DATA);

            let reg_addr = make_config_address(addr, offset);
            addr_port.write(reg_addr);
            let current = data_port.read();

            let shift = (offset & 2) * 8;
            let mask = !(0xFFFF_u32 << shift);
            let updated = (current & mask) | ((val as u32) << shift);

            addr_port.write(reg_addr);
            data_port.write(updated);
        }
    })
}

/// Записывает 8-битный байт в пространство конфигурации PCI через цикл Read-Modify-Write.
pub fn write_u8(addr: PciAddress, offset: u8, val: u8) {
    interrupts::without_interrupts(|| {
        let _guard = PCI_LOCK.lock();
        unsafe {
            let mut addr_port = Port::<u32>::new(PCI_CONFIG_ADDRESS);
            let mut data_port = Port::<u32>::new(PCI_CONFIG_DATA);

            let reg_addr = make_config_address(addr, offset);
            addr_port.write(reg_addr);
            let current = data_port.read();

            let shift = (offset & 3) * 8;
            let mask = !(0xFF_u32 << shift);
            let updated = (current & mask) | ((val as u32) << shift);

            addr_port.write(reg_addr);
            data_port.write(updated);
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_make_config_address_calculation() {
        let addr = PciAddress::new(0, 1, 0);
        let val = make_config_address(addr, 0x00);
        // Enable bit (0x80000000) | bus 0 << 16 | dev 1 << 11 (0x800) | fn 0 << 8 | reg 0
        assert_eq!(val, 0x8000_0800);

        let addr2 = PciAddress::new(2, 4, 3);
        let val2 = make_config_address(addr2, 0x1C);
        // Enable bit | 2 << 16 | 4 << 11 (0x2000) | 3 << 8 (0x300) | 0x1C
        assert_eq!(val2, 0x8002_231C);
    }

    #[test_case]
    fn test_offset_alignment_masking() {
        let addr = PciAddress::new(0, 0, 0);
        // Невыровненные смещения 0x01, 0x02, 0x03 должны выравниваться по 4 байтам на 0x00
        assert_eq!(make_config_address(addr, 0x01), 0x8000_0000);
        assert_eq!(make_config_address(addr, 0x02), 0x8000_0000);
        assert_eq!(make_config_address(addr, 0x03), 0x8000_0000);
        assert_eq!(make_config_address(addr, 0x06), 0x8000_0004);
    }
}
