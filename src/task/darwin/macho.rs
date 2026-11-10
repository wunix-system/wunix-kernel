//! Загрузчик исполняемых файлов формата Mach-O (64-bit macOS / Darwin).

use x86_64::VirtAddr;
use x86_64::structures::paging::PageTableFlags;
use crate::memory::UserAddressSpace;
use crate::task::elf::{ElfError, ElfLoadInfo, USER_STACK_SIZE, USER_STACK_TOP};

pub const MH_MAGIC: u32 = 0xFEED_FACE;
pub const MH_CIGAM: u32 = 0xCEFA_EDFE;
pub const MH_MAGIC_64: u32 = 0xFEED_FACF;
pub const MH_CIGAM_64: u32 = 0xCFFA_EDFE;
pub const FAT_MAGIC: u32 = 0xCAFE_BABE;
pub const FAT_CIGAM: u32 = 0xBEBA_FECA;
pub const FAT_MAGIC_64: u32 = 0xCAFE_BABF;
pub const FAT_CIGAM_64: u32 = 0xBFBA_FECA;

pub const CPU_TYPE_X86_64: u32 = 0x0100_0007;

pub const LC_SEGMENT_64: u32 = 0x19;
pub const LC_UNIXTHREAD: u32 = 0x5;
pub const LC_MAIN: u32 = 0x8000_0028;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct MachHeader64 {
    pub magic: u32,
    pub cputype: u32,
    pub cpusubtype: u32,
    pub filetype: u32,
    pub ncmds: u32,
    pub sizeofcmds: u32,
    pub flags: u32,
    pub reserved: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct LoadCommand {
    pub cmd: u32,
    pub cmdsize: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SegmentCommand64 {
    pub cmd: u32,
    pub cmdsize: u32,
    pub segname: [u8; 16],
    pub vmaddr: u64,
    pub vmsize: u64,
    pub fileoff: u64,
    pub filesize: u64,
    pub maxprot: u32,
    pub initprot: u32,
    pub nsects: u32,
    pub flags: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct EntryPointCommand {
    pub cmd: u32,
    pub cmdsize: u32,
    pub entryoff: u64,
    pub stacksize: u64,
}

/// Проверяет, является ли бинарный файл Mach-O образом (включая Universal / Fat).
pub fn is_macho(bytes: &[u8]) -> bool {
    if bytes.len() < 4 {
        return false;
    }
    let magic = u32::from_ne_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    matches!(
        magic,
        MH_MAGIC | MH_CIGAM | MH_MAGIC_64 | MH_CIGAM_64 | FAT_MAGIC | FAT_CIGAM | FAT_MAGIC_64 | FAT_CIGAM_64
    )
}

/// Загружает 64-битный Mach-O бинарник в пользовательское адресное пространство.
pub fn load_macho(bytes: &[u8]) -> Result<(UserAddressSpace, ElfLoadInfo), ElfError> {
    if bytes.len() < core::mem::size_of::<MachHeader64>() {
        return Err(ElfError::TooSmall);
    }

    let header = unsafe { core::ptr::read_unaligned(bytes.as_ptr() as *const MachHeader64) };
    if header.magic != MH_MAGIC_64 {
        crate::serial_println!("[macho] unsupported magic: {:#x}", header.magic);
        return Err(ElfError::InvalidMagic);
    }

    if header.cputype != CPU_TYPE_X86_64 {
        crate::serial_println!("[macho] unsupported cputype: {:#x}", header.cputype);
        return Err(ElfError::UnsupportedMachine);
    }

    let mut user_space = UserAddressSpace::new().map_err(|_| ElfError::OutOfMemory)?;

    let mut offset = core::mem::size_of::<MachHeader64>();
    let mut entry_point = 0u64;
    let mut max_vaddr = 0u64;
    let mut text_base = 0u64;

    for _ in 0..header.ncmds {
        if offset + core::mem::size_of::<LoadCommand>() > bytes.len() {
            return Err(ElfError::TooSmall);
        }

        let cmd = unsafe { core::ptr::read_unaligned(bytes.as_ptr().add(offset) as *const LoadCommand) };
        if cmd.cmdsize < core::mem::size_of::<LoadCommand>() as u32 || offset + cmd.cmdsize as usize > bytes.len() {
            return Err(ElfError::TooSmall);
        }

        if cmd.cmd == LC_SEGMENT_64 {
            if cmd.cmdsize as usize >= core::mem::size_of::<SegmentCommand64>() {
                let seg = unsafe { core::ptr::read_unaligned(bytes.as_ptr().add(offset) as *const SegmentCommand64) };

                // Пропускаем страницу нулевого указателя (__PAGEZERO)
                let is_pagezero = &seg.segname[..10] == b"__PAGEZERO";
                if !is_pagezero && seg.vmsize > 0 {
                    if &seg.segname[..6] == b"__TEXT" {
                        text_base = seg.vmaddr;
                    }

                    let seg_end = seg.vmaddr.saturating_add(seg.vmsize);
                    if seg_end > max_vaddr {
                        max_vaddr = seg_end;
                    }

                    let file_start = seg.fileoff as usize;
                    let file_end = file_start + seg.filesize as usize;
                    let seg_data = if file_end <= bytes.len() && seg.filesize > 0 {
                        &bytes[file_start..file_end]
                    } else {
                        &[]
                    };

                    let mut flags = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
                    if (seg.initprot & 0x2) != 0 {
                        flags |= PageTableFlags::WRITABLE;
                    }
                    if (seg.initprot & 0x4) == 0 {
                        flags |= PageTableFlags::NO_EXECUTE;
                    }

                    user_space
                        .load_segment(seg.vmaddr, seg.vmsize, seg_data, flags)
                        .map_err(|_| ElfError::MappingFailed)?;
                }
            }
        } else if cmd.cmd == LC_MAIN {
            if cmd.cmdsize as usize >= core::mem::size_of::<EntryPointCommand>() {
                let ep = unsafe { core::ptr::read_unaligned(bytes.as_ptr().add(offset) as *const EntryPointCommand) };
                entry_point = text_base + ep.entryoff;
            }
        } else if cmd.cmd == LC_UNIXTHREAD {
            let thread_state_offset = offset + 16;
            if thread_state_offset + 168 <= bytes.len() {
                let rip_offset = thread_state_offset + 16 * 8; // __rip
                if rip_offset + 8 <= bytes.len() {
                    entry_point = unsafe { core::ptr::read_unaligned(bytes.as_ptr().add(rip_offset) as *const u64) };
                }
            }
        }

        offset += cmd.cmdsize as usize;
    }

    // Отображаем пользовательский стек 2 МиБ на USER_STACK_TOP
    let stack_bottom = VirtAddr::new(USER_STACK_TOP - USER_STACK_SIZE as u64);
    user_space
        .allocate_and_map_range(
            stack_bottom,
            USER_STACK_SIZE,
            PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE,
        )
        .map_err(|_| ElfError::MappingFailed)?;

    if entry_point == 0 {
        entry_point = text_base;
    }

    Ok((
        user_space,
        ElfLoadInfo {
            entry_point: VirtAddr::new(entry_point),
            prog_entry: entry_point,
            phdr_addr: 0,
            phent: 0,
            phnum: 0,
            max_vaddr,
            at_base: 0,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_macho_detection() {
        let macho_64_magic = MH_MAGIC_64.to_ne_bytes();
        assert!(is_macho(&macho_64_magic));

        let fat_magic = FAT_MAGIC.to_ne_bytes();
        assert!(is_macho(&fat_magic));

        let elf_magic = [0x7f, b'E', b'L', b'F'];
        assert!(!is_macho(&elf_magic));

        let pe_magic = [b'M', b'Z', 0, 0];
        assert!(!is_macho(&pe_magic));

        let short_data = [0xfe, 0xed];
        assert!(!is_macho(&short_data));
    }
}
