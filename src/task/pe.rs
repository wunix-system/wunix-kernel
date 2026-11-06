use x86_64::structures::paging::PageTableFlags;
use x86_64::VirtAddr;

use crate::memory::UserAddressSpace;
use crate::task::elf::{ElfError, ElfLoadInfo, USER_STACK_SIZE, USER_STACK_TOP};

pub const DOS_MAGIC: u16 = 0x5A4D; // 'MZ'
pub const PE_SIGNATURE: u32 = 0x0000_4550; // 'PE\0\0'
pub const MACHINE_AMD64: u16 = 0x8664;
pub const PE32_PLUS_MAGIC: u16 = 0x020B;

pub const IMAGE_SCN_MEM_EXECUTE: u32 = 0x2000_0000;
pub const IMAGE_SCN_MEM_READ: u32 = 0x4000_0000;
pub const IMAGE_SCN_MEM_WRITE: u32 = 0x8000_0000;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct DosHeader {
    pub e_magic: u16,
    pub e_cblp: u16,
    pub e_cp: u16,
    pub e_crlc: u16,
    pub e_cparhdr: u16,
    pub e_minalloc: u16,
    pub e_maxalloc: u16,
    pub e_ss: u16,
    pub e_sp: u16,
    pub e_csum: u16,
    pub e_ip: u16,
    pub e_cs: u16,
    pub e_lfarlc: u16,
    pub e_ovno: u16,
    pub e_res: [u16; 4],
    pub e_oemid: u16,
    pub e_oeminfo: u16,
    pub e_res2: [u16; 10],
    pub e_lfanew: i32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct CoffHeader {
    pub machine: u16,
    pub number_of_sections: u16,
    pub time_date_stamp: u32,
    pub pointer_to_symbol_table: u32,
    pub number_of_symbols: u32,
    pub size_of_optional_header: u16,
    pub characteristics: u16,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct OptionalHeader64 {
    pub magic: u16,
    pub major_linker_version: u8,
    pub minor_linker_version: u8,
    pub size_of_code: u32,
    pub size_of_initialized_data: u32,
    pub size_of_uninitialized_data: u32,
    pub address_of_entry_point: u32,
    pub base_of_code: u32,
    pub image_base: u64,
    pub section_alignment: u32,
    pub file_alignment: u32,
    pub major_os_version: u16,
    pub minor_os_version: u16,
    pub major_image_version: u16,
    pub minor_image_version: u16,
    pub major_subsystem_version: u16,
    pub minor_subsystem_version: u16,
    pub win32_version_value: u32,
    pub size_of_image: u32,
    pub size_of_headers: u32,
    pub check_sum: u32,
    pub subsystem: u16,
    pub dll_characteristics: u16,
    pub size_of_stack_reserve: u64,
    pub size_of_stack_commit: u64,
    pub size_of_heap_reserve: u64,
    pub size_of_heap_commit: u64,
    pub loader_flags: u32,
    pub number_of_rva_and_sizes: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SectionHeader {
    pub name: [u8; 8],
    pub virtual_size: u32,
    pub virtual_address: u32,
    pub size_of_raw_data: u32,
    pub pointer_to_raw_data: u32,
    pub pointer_to_relocations: u32,
    pub pointer_to_linenumbers: u32,
    pub number_of_relocations: u16,
    pub number_of_linenumbers: u16,
    pub characteristics: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct ImageDataDirectory {
    pub virtual_address: u32,
    pub size: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct ImageImportDescriptor {
    pub original_first_thunk: u32,
    pub time_date_stamp: u32,
    pub forwarder_chain: u32,
    pub name: u32,
    pub first_thunk: u32,
}

pub const IMAGE_DIRECTORY_ENTRY_EXPORT: usize = 0;
pub const IMAGE_DIRECTORY_ENTRY_IMPORT: usize = 1;

fn rva_to_offset(rva: u32, sections: &[SectionHeader]) -> Option<usize> {
    for sec in sections {
        let sec_vaddr = sec.virtual_address;
        let sec_vsize = if sec.virtual_size != 0 {
            sec.virtual_size
        } else {
            sec.size_of_raw_data
        };
        if rva >= sec_vaddr && rva < sec_vaddr + sec_vsize {
            let offset_in_sec = rva - sec_vaddr;
            return Some((sec.pointer_to_raw_data + offset_in_sec) as usize);
        }
    }
    None
}

fn resolve_pe_imports(
    bytes: &[u8],
    opt: &OptionalHeader64,
    import_rva: u32,
    sections: &[SectionHeader],
    user_space: &mut UserAddressSpace,
) {
    let mut desc_offset = match rva_to_offset(import_rva, sections) {
        Some(off) => off,
        None => return,
    };

    let desc_size = core::mem::size_of::<ImageImportDescriptor>();
    while desc_offset + desc_size <= bytes.len() {
        let desc = unsafe {
            core::ptr::read_unaligned(bytes.as_ptr().add(desc_offset) as *const ImageImportDescriptor)
        };
        if desc.first_thunk == 0 && desc.name == 0 {
            break;
        }

        let thunk_rva = if desc.original_first_thunk != 0 {
            desc.original_first_thunk
        } else {
            desc.first_thunk
        };

        let mut thunk_offset = match rva_to_offset(thunk_rva, sections) {
            Some(off) => off,
            None => {
                desc_offset += desc_size;
                continue;
            }
        };

        let mut iat_idx = 0;
        while thunk_offset + 8 <= bytes.len() {
            let thunk_val = unsafe {
                core::ptr::read_unaligned(bytes.as_ptr().add(thunk_offset) as *const u64)
            };
            if thunk_val == 0 {
                break;
            }

            let is_ordinal = (thunk_val & 0x8000_0000_0000_0000) != 0;
            if !is_ordinal {
                let name_rva = (thunk_val & 0x7FFF_FFFF) as u32;
                if let Some(name_offset) = rva_to_offset(name_rva, sections) {
                    let str_offset = name_offset + 2;
                    let mut end = str_offset;
                    while end < bytes.len() && bytes[end] != 0 && (end - str_offset) < 64 {
                        end += 1;
                    }
                    if let Ok(func_name) = core::str::from_utf8(&bytes[str_offset..end]) {
                        if let Some(target_vaddr) = crate::task::syslib::resolve_symbol_vaddr(func_name) {
                            let iat_entry_vaddr = opt.image_base + desc.first_thunk as u64 + (iat_idx * 8);
                            let _ = user_space.write_user_bytes(iat_entry_vaddr, &target_vaddr.to_le_bytes());
                        }
                    }
                }
            }

            thunk_offset += 8;
            iat_idx += 1;
        }

        desc_offset += desc_size;
    }
}

pub fn is_pe(bytes: &[u8]) -> bool {
    if bytes.len() < core::mem::size_of::<DosHeader>() {
        return false;
    }
    let dos = unsafe { core::ptr::read_unaligned(bytes.as_ptr() as *const DosHeader) };
    if dos.e_magic != DOS_MAGIC || dos.e_lfanew <= 0 {
        return false;
    }
    let pe_offset = dos.e_lfanew as usize;
    if pe_offset + 4 > bytes.len() {
        return false;
    }
    let sig = unsafe { core::ptr::read_unaligned(bytes.as_ptr().add(pe_offset) as *const u32) };
    sig == PE_SIGNATURE
}

pub fn load_pe(bytes: &[u8]) -> Result<(UserAddressSpace, ElfLoadInfo), ElfError> {
    if !is_pe(bytes) {
        return Err(ElfError::InvalidMagic);
    }
    let dos = unsafe { core::ptr::read_unaligned(bytes.as_ptr() as *const DosHeader) };
    let pe_offset = dos.e_lfanew as usize;

    let coff_offset = pe_offset + 4;
    if coff_offset + core::mem::size_of::<CoffHeader>() > bytes.len() {
        return Err(ElfError::TooSmall);
    }
    let coff = unsafe { core::ptr::read_unaligned(bytes.as_ptr().add(coff_offset) as *const CoffHeader) };
    if coff.machine != MACHINE_AMD64 {
        return Err(ElfError::UnsupportedMachine);
    }

    let opt_offset = coff_offset + core::mem::size_of::<CoffHeader>();
    if opt_offset + core::mem::size_of::<OptionalHeader64>() > bytes.len() {
        return Err(ElfError::TooSmall);
    }
    let opt = unsafe { core::ptr::read_unaligned(bytes.as_ptr().add(opt_offset) as *const OptionalHeader64) };
    if opt.magic != PE32_PLUS_MAGIC {
        return Err(ElfError::Not64Bit);
    }

    let mut user_space = UserAddressSpace::new().map_err(|_| ElfError::OutOfMemory)?;

    let sections_offset = opt_offset + coff.size_of_optional_header as usize;
    let section_size = core::mem::size_of::<SectionHeader>();
    let mut sections = alloc::vec::Vec::with_capacity(coff.number_of_sections as usize);

    for i in 0..(coff.number_of_sections as usize) {
        let s_offset = sections_offset + i * section_size;
        if s_offset + section_size > bytes.len() {
            return Err(ElfError::InvalidProgramHeaders);
        }
        let sec = unsafe { core::ptr::read_unaligned(bytes.as_ptr().add(s_offset) as *const SectionHeader) };
        sections.push(sec);

        let vaddr = opt.image_base + sec.virtual_address as u64;
        let mem_size = if sec.virtual_size != 0 {
            sec.virtual_size as u64
        } else {
            sec.size_of_raw_data as u64
        };

        if vaddr >= 0x0000_8000_0000_0000 || (vaddr + mem_size) > 0x0000_8000_0000_0000 {
            return Err(ElfError::MappingFailed);
        }

        let mut flags = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
        if (sec.characteristics & IMAGE_SCN_MEM_WRITE) != 0 {
            flags |= PageTableFlags::WRITABLE;
        }

        let raw_offset = sec.pointer_to_raw_data as usize;
        let raw_size = (sec.size_of_raw_data as usize).min(mem_size as usize);
        let slice = if raw_offset + raw_size <= bytes.len() {
            &bytes[raw_offset..raw_offset + raw_size]
        } else {
            &[]
        };

        user_space
            .load_segment(vaddr, mem_size, slice, flags)
            .map_err(|_| ElfError::MappingFailed)?;
    }

    // Отображаем страницу стандартных системных библиотек Wunix (Syslib / vDSO)
    crate::task::syslib::map_syslib_to_user_space(&mut user_space)
        .map_err(|_| ElfError::MappingFailed)?;

    // Разрешаем таблицу импортов IAT через in-kernel системную библиотеку
    let data_dirs_offset = opt_offset + core::mem::size_of::<OptionalHeader64>();
    let dir_size = core::mem::size_of::<ImageDataDirectory>();
    if opt.number_of_rva_and_sizes > (IMAGE_DIRECTORY_ENTRY_IMPORT as u32) {
        let import_dir_offset = data_dirs_offset + IMAGE_DIRECTORY_ENTRY_IMPORT * dir_size;
        if import_dir_offset + dir_size <= bytes.len() {
            let import_dir = unsafe {
                core::ptr::read_unaligned(bytes.as_ptr().add(import_dir_offset) as *const ImageDataDirectory)
            };
            if import_dir.virtual_address != 0 && import_dir.size != 0 {
                resolve_pe_imports(bytes, &opt, import_dir.virtual_address, &sections, &mut user_space);
            }
        }
    }

    // Выделяем пользовательский стек 2 МиБ на границе USER_STACK_TOP
    let stack_bottom = VirtAddr::new(USER_STACK_TOP - USER_STACK_SIZE as u64);
    user_space
        .allocate_and_map_range(
            stack_bottom,
            USER_STACK_SIZE,
            PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE,
        )
        .map_err(|_| ElfError::MappingFailed)?;

    let entry_point = VirtAddr::new(opt.image_base + opt.address_of_entry_point as u64);

    let max_vaddr = opt.image_base + opt.size_of_image as u64;

    Ok((
        user_space,
        ElfLoadInfo {
            entry_point,
            phdr_addr: 0,
            phent: 0,
            phnum: 0,
            max_vaddr,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_pe_magic_detection() {
        assert!(!is_pe(&[]));
        assert!(!is_pe(&[0x7f, b'E', b'L', b'F']));

        let mut fake_pe = alloc::vec![0u8; 128];
        fake_pe[0] = 0x4D; // 'M'
        fake_pe[1] = 0x5A; // 'Z'
        fake_pe[0x3C] = 0x40; // e_lfanew = 0x40
        fake_pe[0x40] = b'P';
        fake_pe[0x41] = b'E';
        fake_pe[0x42] = 0;
        fake_pe[0x43] = 0;

        assert!(is_pe(&fake_pe));
    }
}
