use alloc::vec::Vec;

use super::config;
use super::{BarType, PciAddress, PciBar, PciClass, PciDevice};

/// Опрашивает конкретный адрес BDF (Bus:Device.Function).
/// Возвращает `Some(PciDevice)`, если на шине присутствует реальное устройство.
pub fn probe_device(addr: PciAddress) -> Option<PciDevice> {
    let vendor_id = config::read_u16(addr, 0x00);
    // 0xFFFF означает пустой слот / отсутствие ответа устройства
    if vendor_id == 0xFFFF || vendor_id == 0x0000 {
        return None;
    }

    let device_id = config::read_u16(addr, 0x02);
    let command = config::read_u16(addr, 0x04);
    let status = config::read_u16(addr, 0x06);
    let revision_id = config::read_u8(addr, 0x08);
    let prog_if = config::read_u8(addr, 0x09);
    let subclass = config::read_u8(addr, 0x0A);
    let class_code = config::read_u8(addr, 0x0B);
    let header_type = config::read_u8(addr, 0x0E);

    let mut bars: [Option<PciBar>; 6] = [None, None, None, None, None, None];

    // BAR-регистры присутствуют только у стандартных устройств (Header Type 0x00)
    if (header_type & 0x7F) == 0x00 {
        let mut bar_idx = 0;
        while bar_idx < 6 {
            let offset = 0x10 + (bar_idx as u8) * 4;
            let original = config::read_u32(addr, offset);

            // Записываем все единицы для определения запрашиваемого устройством размера диапазона
            config::write_u32(addr, offset, 0xFFFF_FFFF);
            let size_mask = config::read_u32(addr, offset);
            config::write_u32(addr, offset, original);

            if size_mask == 0 || size_mask == 0xFFFF_FFFF {
                bar_idx += 1;
                continue;
            }

            let is_io = (original & 0x01) == 1;

            if is_io {
                let address = (original & !0x3) as u64;
                // Маска I/O BAR: биты 1..0 зарезервированы
                let size = (!((size_mask & 0xFFFF_FFFC) as u64)) + 1;
                bars[bar_idx] = Some(PciBar {
                    index: bar_idx as u8,
                    bar_type: BarType::Io,
                    address,
                    size,
                    prefetchable: false,
                });
                bar_idx += 1;
            } else {
                let mem_type = (original >> 1) & 0x03;
                let prefetchable = (original & (1 << 3)) != 0;

                if mem_type == 0x02 && bar_idx < 5 {
                    // 64-битный MMIO BAR занимает два последовательных слота: bar_idx и bar_idx + 1
                    let next_offset = offset + 4;
                    let original_high = config::read_u32(addr, next_offset);

                    config::write_u32(addr, next_offset, 0xFFFF_FFFF);
                    let size_mask_high = config::read_u32(addr, next_offset);
                    config::write_u32(addr, next_offset, original_high);

                    let address = ((original_high as u64) << 32) | ((original & !0xF) as u64);
                    let full_mask = ((size_mask_high as u64) << 32) | ((size_mask & !0xF) as u64);
                    let size = (!full_mask) + 1;

                    bars[bar_idx] = Some(PciBar {
                        index: bar_idx as u8,
                        bar_type: BarType::Memory64,
                        address,
                        size,
                        prefetchable,
                    });
                    // Пропускаем следующий слот, так как он занят старшей половиной 64-битного адреса
                    bar_idx += 2;
                } else {
                    // Стандартный 32-битный MMIO BAR
                    let address = (original & !0xF) as u64;
                    let size = (!((size_mask & !0xF) as u64)) + 1;

                    bars[bar_idx] = Some(PciBar {
                        index: bar_idx as u8,
                        bar_type: BarType::Memory32,
                        address,
                        size,
                        prefetchable,
                    });
                    bar_idx += 1;
                }
            }
        }
    }

    let subsystem_vendor_id = config::read_u16(addr, 0x2C);
    let subsystem_id = config::read_u16(addr, 0x2E);
    let interrupt_line = config::read_u8(addr, 0x3C);
    let interrupt_pin = config::read_u8(addr, 0x3D);

    Some(PciDevice {
        address: addr,
        vendor_id,
        device_id,
        command,
        status,
        revision_id,
        prog_if,
        subclass,
        class_code,
        header_type,
        subsystem_vendor_id,
        subsystem_id,
        interrupt_line,
        interrupt_pin,
        bars,
    })
}

/// Выполняет рекурсивное сканирование шины PCI, начиная с заданной шины `bus`.
/// Обнаруживает PCI-to-PCI мосты и переходит на их вторичные шины (secondary buses).
pub fn scan_bus(bus: u8, devices: &mut Vec<PciDevice>, visited_buses: &mut [bool; 256]) {
    if visited_buses[bus as usize] {
        return;
    }
    visited_buses[bus as usize] = true;

    for dev in 0..32 {
        let func0_addr = PciAddress::new(bus, dev, 0);
        let func0 = match probe_device(func0_addr) {
            Some(dev) => dev,
            None => continue,
        };

        // Бит 7 в header_type указывает на многофункциональное устройство
        let is_multi_function = (func0.header_type & 0x80) != 0;
        let is_bridge = func0.class_code == PciClass::BRIDGE
            && func0.subclass == PciClass::SUBCLASS_PCI_BRIDGE;

        devices.push(func0);

        if is_bridge {
            // У PCI-to-PCI моста номер вторичной шины хранится в регистре 0x19
            let secondary_bus = config::read_u8(func0_addr, 0x19);
            if secondary_bus != 0 && secondary_bus != bus {
                scan_bus(secondary_bus, devices, visited_buses);
            }
        }

        // Если устройство многофункциональное, проверяем функции 1..7
        if is_multi_function {
            for func in 1..8 {
                let func_addr = PciAddress::new(bus, dev, func);
                if let Some(func_dev) = probe_device(func_addr) {
                    let is_secondary_bridge = func_dev.class_code == PciClass::BRIDGE
                        && func_dev.subclass == PciClass::SUBCLASS_PCI_BRIDGE;

                    devices.push(func_dev);

                    if is_secondary_bridge {
                        let secondary_bus = config::read_u8(func_addr, 0x19);
                        if secondary_bus != 0 && secondary_bus != bus {
                            scan_bus(secondary_bus, devices, visited_buses);
                        }
                    }
                }
            }
        }
    }
}

/// Сканирует всю топологию PCI:
/// Начинает с корневой шины 0 (и дополнительных хост-шин при наличии нескольких PCI host controllers),
/// рекурсивно спускаясь по PCI-to-PCI мостам.
pub fn scan_all_busses() -> Vec<PciDevice> {
    let mut devices = Vec::new();
    let mut visited_buses = [false; 256];

    // Проверяем корневую шину 0
    let host_controller = probe_device(PciAddress::new(0, 0, 0));
    if let Some(host) = host_controller {
        // Если хост-мост многофункциональный, функции 0..7 могут быть отдельными PCI-шинами
        if (host.header_type & 0x80) != 0 {
            for func in 0..8 {
                let addr = PciAddress::new(0, 0, func);
                if config::read_u16(addr, 0x00) != 0xFFFF {
                    scan_bus(func, &mut devices, &mut visited_buses);
                }
            }
        } else {
            scan_bus(0, &mut devices, &mut visited_buses);
        }
    } else {
        scan_bus(0, &mut devices, &mut visited_buses);
    }

    devices
}
