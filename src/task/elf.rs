use x86_64::structures::paging::PageTableFlags;
use x86_64::VirtAddr;

use crate::memory::UserAddressSpace;

pub const ELF_MAGIC: [u8; 4] = [0x7F, b'E', b'L', b'F'];
pub const ELFCLASS64: u8 = 2;
pub const ELFDATA2LSB: u8 = 1;
pub const EV_CURRENT: u8 = 1;

pub const ET_EXEC: u16 = 2;
pub const ET_DYN: u16 = 3;

pub const EM_X86_64: u16 = 62;

pub const PT_LOAD: u32 = 1;
pub const PT_DYNAMIC: u32 = 2;
pub const PT_INTERP: u32 = 3;
pub const PT_NOTE: u32 = 4;
pub const PT_SHLIB: u32 = 5;
pub const PT_PHDR: u32 = 6;
pub const PT_TLS: u32 = 7;

pub const PF_X: u32 = 1 << 0;
pub const PF_W: u32 = 1 << 1;
pub const PF_R: u32 = 1 << 2;

// Auxiliary Vector идентификаторы согласно стандарту Linux x86_64 ABI
pub const AT_NULL: u64 = 0;
pub const AT_IGNORE: u64 = 1;
pub const AT_EXECFD: u64 = 2;
pub const AT_PHDR: u64 = 3;
pub const AT_PHENT: u64 = 4;
pub const AT_PHNUM: u64 = 5;
pub const AT_PAGESZ: u64 = 6;
pub const AT_BASE: u64 = 7;
pub const AT_FLAGS: u64 = 8;
pub const AT_ENTRY: u64 = 9;
pub const AT_NOTELF: u64 = 10;
pub const AT_UID: u64 = 11;
pub const AT_EUID: u64 = 12;
pub const AT_GID: u64 = 13;
pub const AT_EGID: u64 = 14;
pub const AT_CLKTCK: u64 = 17;
pub const AT_SECURE: u64 = 23;
pub const AT_RANDOM: u64 = 25;

// Вершина пользовательского стека (на границе канонического 48-битного диапазона виртуальной памяти)
pub const USER_STACK_TOP: u64 = 0x0000_7FFF_FFFF_0000;
// Размер стека: 2 МБ (512 физических страниц 4KiB для поддержки многопоточного рантайма Go)
pub const USER_STACK_SIZE: usize = 0x200000;

#[derive(Debug, Clone, Copy)]
pub struct ElfLoadInfo {
    pub entry_point: VirtAddr,
    pub prog_entry: u64,
    pub phdr_addr: u64,
    pub phent: u16,
    pub phnum: u16,
    pub max_vaddr: u64,
    pub at_base: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElfError {
    TooSmall,
    InvalidMagic,
    Not64Bit,
    NotLittleEndian,
    UnsupportedMachine,
    UnsupportedType,
    InvalidProgramHeaders,
    OutOfMemory,
    MappingFailed,
    InterpreterNotFound,
    InterpreterInvalid,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Elf64Header {
    pub e_ident: [u8; 16],
    pub e_type: u16,
    pub e_machine: u16,
    pub e_version: u32,
    pub e_entry: u64,
    pub e_phoff: u64,
    pub e_shoff: u64,
    pub e_flags: u32,
    pub e_ehsize: u16,
    pub e_phentsize: u16,
    pub e_phnum: u16,
    pub e_shentsize: u16,
    pub e_shnum: u16,
    pub e_shstrndx: u16,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Elf64ProgramHeader {
    pub p_type: u32,
    pub p_flags: u32,
    pub p_offset: u64,
    pub p_vaddr: u64,
    pub p_paddr: u64,
    pub p_filesz: u64,
    pub p_memsz: u64,
    pub p_align: u64,
}

/// Выполняет строгую проверку соответствия байтового среза спецификации ELF64 для архитектуры x86_64
pub fn validate_header(bytes: &[u8]) -> Result<Elf64Header, ElfError> {
    if bytes.len() < core::mem::size_of::<Elf64Header>() {
        return Err(ElfError::TooSmall);
    }

    // Читаем заголовок без предположений о выравнивании входного среза
    let header = unsafe { core::ptr::read_unaligned(bytes.as_ptr() as *const Elf64Header) };

    if header.e_ident[0..4] != ELF_MAGIC {
        return Err(ElfError::InvalidMagic);
    }
    if header.e_ident[4] != ELFCLASS64 {
        return Err(ElfError::Not64Bit);
    }
    if header.e_ident[5] != ELFDATA2LSB {
        return Err(ElfError::NotLittleEndian);
    }
    if header.e_machine != EM_X86_64 {
        return Err(ElfError::UnsupportedMachine);
    }
    if header.e_type != ET_EXEC && header.e_type != ET_DYN {
        return Err(ElfError::UnsupportedType);
    }

    let phentsize = header.e_phentsize as usize;
    if phentsize < core::mem::size_of::<Elf64ProgramHeader>() {
        return Err(ElfError::InvalidProgramHeaders);
    }

    Ok(header)
}

/// Загружает исполняемый ELF64 файл в новое изолированное адресное пространство процесса (Ring 3).
///
/// 1. Проверяет корректность формата ELF64.
/// 2. Для каждого загружаемого сегмента PT_LOAD выделяет и маппит физические фреймы.
/// 3. Выделяет и отображает стек пользователя на USER_STACK_TOP.
/// 4. Возвращает сконфигурированное адресное пространство и метаданные ELF (RIP, PHDR, PHNUM, PHENT).
pub fn load_elf(elf_bytes: &[u8]) -> Result<(UserAddressSpace, ElfLoadInfo), ElfError> {
    let header = validate_header(elf_bytes)?;

    let ph_offset = header.e_phoff as usize;
    let ph_num = header.e_phnum as usize;
    let ph_entry_size = header.e_phentsize as usize;

    let ph_total_bytes = ph_num
        .checked_mul(ph_entry_size)
        .ok_or(ElfError::InvalidProgramHeaders)?;

    let ph_end = ph_offset
        .checked_add(ph_total_bytes)
        .ok_or(ElfError::InvalidProgramHeaders)?;

    if ph_end > elf_bytes.len() {
        return Err(ElfError::InvalidProgramHeaders);
    }

    // Для позиционно-независимых исполняемых файлов (ET_DYN / PIE) со стартовым адресом 0
    // смещаем базовый адрес на 4 МиБ, чтобы нулевая страница оставалась защищенной от разыменования.
    let load_base = if header.e_type == ET_DYN {
        0x0000_0000_0040_0000u64
    } else {
        0u64
    };

    let mut user_space = UserAddressSpace::new().map_err(|_| ElfError::OutOfMemory)?;
    let mut phdr_addr = 0u64;
    let mut max_vaddr = 0u64;

    for i in 0..ph_num {
        let entry_offset = ph_offset + i * ph_entry_size;
        let ph = unsafe {
            core::ptr::read_unaligned(
                elf_bytes.as_ptr().add(entry_offset) as *const Elf64ProgramHeader
            )
        };

        if ph.p_type == PT_PHDR {
            phdr_addr = load_base + ph.p_vaddr;
        }

        if ph.p_type != PT_LOAD || ph.p_memsz == 0 {
            continue;
        }

        let file_start = ph.p_offset as usize;
        let file_len = ph.p_filesz as usize;
        let file_end = file_start
            .checked_add(file_len)
            .ok_or(ElfError::InvalidProgramHeaders)?;

        if file_end > elf_bytes.len() || ph.p_filesz > ph.p_memsz {
            return Err(ElfError::InvalidProgramHeaders);
        }

        let vaddr = load_base + ph.p_vaddr;

        // Защита границы Higher-Half: пользовательские сегменты не могут залезать в адресное пространство ядра
        if vaddr >= 0x0000_8000_0000_0000 || (vaddr + ph.p_memsz) > 0x0000_8000_0000_0000 {
            return Err(ElfError::MappingFailed);
        }

        let seg_end = vaddr + ph.p_memsz;
        if seg_end > max_vaddr {
            max_vaddr = seg_end;
        }

        let mut flags = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
        if (ph.p_flags & PF_W) != 0 {
            flags |= PageTableFlags::WRITABLE;
        }

        let file_slice = &elf_bytes[file_start..file_end];
        user_space
            .load_segment(vaddr, ph.p_memsz, file_slice, flags)
            .map_err(|_| ElfError::MappingFailed)?;
    }

    if phdr_addr == 0 {
        // Если PT_PHDR отсутствует явно, ищем сегмент PT_LOAD, содержащий таблицу заголовков (e_phoff)
        for i in 0..ph_num {
            let entry_offset = ph_offset + i * ph_entry_size;
            let ph = unsafe {
                core::ptr::read_unaligned(
                    elf_bytes.as_ptr().add(entry_offset) as *const Elf64ProgramHeader,
                )
            };
            if ph.p_type == PT_LOAD
                && header.e_phoff >= ph.p_offset
                && header.e_phoff < ph.p_offset + ph.p_filesz
            {
                phdr_addr = load_base + ph.p_vaddr + (header.e_phoff - ph.p_offset);
                break;
            }
        }
        if phdr_addr == 0 {
            phdr_addr = load_base + header.e_phoff;
        }
    }

    // Выделяем пользовательский стек 2 МБ для поддержки глубоких рекурсий рантайма Go
    let stack_bottom = VirtAddr::new(USER_STACK_TOP - USER_STACK_SIZE as u64);
    user_space
        .allocate_and_map_range(
            stack_bottom,
            USER_STACK_SIZE,
            PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE,
        )
        .map_err(|_| ElfError::MappingFailed)?;

    // Отображаем разделяемую библиотечную страницу Syslib (vDSO)
    crate::task::syslib::map_syslib_to_user_space(&mut user_space)
        .map_err(|_| ElfError::MappingFailed)?;

    // Ищем сегмент PT_INTERP: если исполняемый файл динамический,
    // необходимо загрузить в адресное пространство ELF-интерпретатор (ld.so)
    let mut interp_path: Option<alloc::string::String> = None;
    for i in 0..ph_num {
        let entry_offset = ph_offset + i * ph_entry_size;
        let ph = unsafe {
            core::ptr::read_unaligned(
                elf_bytes.as_ptr().add(entry_offset) as *const Elf64ProgramHeader,
            )
        };
        if ph.p_type == PT_INTERP {
            let start = ph.p_offset as usize;
            let end = (ph.p_offset + ph.p_filesz) as usize;
            if end <= elf_bytes.len() {
                let slice = &elf_bytes[start..end];
                let clean = if let Some(&0) = slice.last() {
                    &slice[..slice.len() - 1]
                } else {
                    slice
                };
                if let Ok(s) = core::str::from_utf8(clean) {
                    interp_path = Some(alloc::string::String::from(s));
                }
            }
            break;
        }
    }

    let prog_entry = load_base + header.e_entry;
    let mut entry_point = VirtAddr::new(prog_entry);
    let mut at_base = 0u64;

    if let Some(path) = interp_path {
        let interp_bytes = crate::fs::read_file(&path)
            .or_else(|_| {
                if let Some(rest) = path.strip_prefix("/lib64/") {
                    crate::fs::read_file(&alloc::format!("/lib/{}", rest))
                } else if let Some(rest) = path.strip_prefix("/lib/") {
                    crate::fs::read_file(&alloc::format!("/lib64/{}", rest))
                } else {
                    Err(crate::fs::VfsError::NotFound)
                }
            })
            .or_else(|_| {
                let base = path.rsplit('/').next().unwrap_or(&path);
                crate::fs::read_file(&alloc::format!("/lib/{}", base))
                    .or_else(|_| crate::fs::read_file(&alloc::format!("/lib64/{}", base)))
            })
            .map_err(|_| ElfError::InterpreterNotFound)?;

        let interp_hdr = validate_header(&interp_bytes).map_err(|_| ElfError::InterpreterInvalid)?;

        // Загружаем динамический компоновщик в изолированный высокий диапазон 0x7000_0000_0000
        let interp_base = if interp_hdr.e_type == ET_DYN {
            0x0000_7000_0000_0000u64
        } else {
            0u64
        };

        let interp_ph_off = interp_hdr.e_phoff as usize;
        let interp_ph_num = interp_hdr.e_phnum as usize;
        let interp_phent = interp_hdr.e_phentsize as usize;
        let interp_ph_total = interp_ph_num
            .checked_mul(interp_phent)
            .ok_or(ElfError::InterpreterInvalid)?;

        if interp_ph_off + interp_ph_total > interp_bytes.len() {
            return Err(ElfError::InterpreterInvalid);
        }

        for i in 0..interp_ph_num {
            let entry_off = interp_ph_off + i * interp_phent;
            let ph = unsafe {
                core::ptr::read_unaligned(
                    interp_bytes.as_ptr().add(entry_off) as *const Elf64ProgramHeader,
                )
            };

            if ph.p_type != PT_LOAD || ph.p_memsz == 0 {
                continue;
            }

            let f_start = ph.p_offset as usize;
            let f_len = ph.p_filesz as usize;
            let f_end = f_start
                .checked_add(f_len)
                .ok_or(ElfError::InterpreterInvalid)?;

            if f_end > interp_bytes.len() || ph.p_filesz > ph.p_memsz {
                return Err(ElfError::InterpreterInvalid);
            }

            let vaddr = interp_base + ph.p_vaddr;
            if vaddr >= 0x0000_8000_0000_0000 || (vaddr + ph.p_memsz) > 0x0000_8000_0000_0000 {
                return Err(ElfError::MappingFailed);
            }

            let mut flags = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
            if (ph.p_flags & PF_W) != 0 {
                flags |= PageTableFlags::WRITABLE;
            }

            let file_slice = &interp_bytes[f_start..f_end];
            user_space
                .load_segment(vaddr, ph.p_memsz, file_slice, flags)
                .map_err(|_| ElfError::MappingFailed)?;
        }

        entry_point = VirtAddr::new(interp_base + interp_hdr.e_entry);
        at_base = interp_base;
    }

    Ok((
        user_space,
        ElfLoadInfo {
            entry_point,
            prog_entry,
            phdr_addr,
            phent: header.e_phentsize,
            phnum: header.e_phnum,
            max_vaddr,
            at_base,
        },
    ))
}

/// Настраивает стек Ring 3 процесса в строгом соответствии со спецификацией System V AMD64 ABI:
///
///   [RSP]                         = argc (u64)
///   [RSP + 8 .. + 8*argc]         = argv[0], ..., argv[N-1] (указатели на строки)
///   [RSP + 8*(argc+1)]            = NULL (0u64 — конец argv)
///   [RSP + 8*(argc+2) ..]         = envp[0], ..., envp[M-1] (указатели на строки "KEY=VALUE")
///   [RSP + ...]                   = NULL (0u64 — конец envp)
///   [RSP + ...]                   = auxv[0]: (AT_PHDR = 3, phdr_addr)
///                                   auxv[1]: (AT_PHENT = 4, sizeof(Elf64_Phdr))
///                                   auxv[2]: (AT_PHNUM = 5, phnum)
///                                   auxv[3]: (AT_PAGESZ = 6, 4096)
///                                   auxv[4]: (AT_BASE = 7, interp_base)
///                                   auxv[5]: (AT_FLAGS = 8, 0)
///                                   auxv[6]: (AT_ENTRY = 9, prog_entry)
///                                   ...
///   [Ниже на стеке]               = Байты строк argv, envp и 16 байт для AT_RANDOM.
///
/// Выравнивание конечного адреса RSP строго кратно 16 байтам перед входом в точку запуска.
pub fn setup_user_stack(
    user_space: &mut UserAddressSpace,
    args: &[alloc::string::String],
    env: &[alloc::string::String],
    info: &ElfLoadInfo,
) -> Result<(u64, u64), &'static str> {
    let mut sp = USER_STACK_TOP;

    // 1. 16 байт аппаратной энтропии для AT_RANDOM (рантайм Go использует для инициализации сида хеш-таблиц)
    let mut rand_bytes = [0u8; 16];
    let tsc = unsafe { core::arch::x86_64::_rdtsc() };
    rand_bytes[0..8].copy_from_slice(&tsc.to_le_bytes());
    let ticks = crate::arch::x86_64::timer::ticks();
    rand_bytes[8..16].copy_from_slice(&(ticks ^ 0xdead_beef_c001_cafe).to_le_bytes());
    sp -= 16;
    user_space.write_user_bytes(sp, &rand_bytes)?;
    let random_buf_ptr = sp;

    // 2. Строки переменных окружения (envp)
    let mut env_ptrs = alloc::vec::Vec::new();
    for var in env.iter().rev() {
        let bytes = var.as_bytes();
        sp -= (bytes.len() + 1) as u64;
        user_space.write_user_bytes(sp, bytes)?;
        user_space.write_user_bytes(sp + bytes.len() as u64, &[0])?;
        env_ptrs.push(sp);
    }
    env_ptrs.reverse();

    // 3. Строки аргументов командной строки (argv)
    let mut arg_ptrs = alloc::vec::Vec::new();
    for arg in args.iter().rev() {
        let bytes = arg.as_bytes();
        sp -= (bytes.len() + 1) as u64;
        user_space.write_user_bytes(sp, bytes)?;
        user_space.write_user_bytes(sp + bytes.len() as u64, &[0])?;
        arg_ptrs.push(sp);
    }
    arg_ptrs.reverse();

    // 4. Векторы Auxiliary Vectors (auxv) согласно System V AMD64 ABI
    let auxv: [(u64, u64); 16] = [
        (AT_PHDR, info.phdr_addr),
        (AT_PHENT, info.phent as u64),
        (AT_PHNUM, info.phnum as u64),
        (AT_PAGESZ, 4096),
        (AT_BASE, info.at_base),
        (AT_FLAGS, 0),
        (AT_ENTRY, info.prog_entry),
        (AT_UID, 0),
        (AT_EUID, 0),
        (AT_GID, 0),
        (AT_EGID, 0),
        (AT_SECURE, 0),
        (AT_CLKTCK, 100),
        (AT_RANDOM, random_buf_ptr),
        (crate::task::syslib::AT_SYSINFO_EHDR, crate::task::syslib::SYSLIB_INFO_VADDR),
        (AT_NULL, 0),
    ];

    // 5. Размер таблицы указателей:
    //    argc (1) + argv (N) + NULL (1) + envp (M) + NULL (1) + auxv (8 пар = 16 слов)
    let total_entries = 1 + arg_ptrs.len() + 1 + env_ptrs.len() + 1 + auxv.len() * 2;
    let total_table_bytes = (total_entries * 8) as u64;

    // Выравниваем sp так, чтобы после вычитания total_table_bytes адрес [RSP] был строго кратен 16
    sp &= !7;
    if (sp - total_table_bytes) % 16 != 0 {
        sp -= 8;
    }

    let final_sp = sp - total_table_bytes;
    let mut cur = final_sp;

    // Записываем argc
    user_space.write_user_bytes(cur, &(args.len() as u64).to_ne_bytes())?;
    cur += 8;

    // Записываем указатели argv[0..N]
    let argv_addr = cur;
    for &ptr in &arg_ptrs {
        user_space.write_user_bytes(cur, &ptr.to_ne_bytes())?;
        cur += 8;
    }
    // Завершающий NULL для argv
    user_space.write_user_bytes(cur, &0u64.to_ne_bytes())?;
    cur += 8;

    // Записываем указатели envp[0..M]
    for &ptr in &env_ptrs {
        user_space.write_user_bytes(cur, &ptr.to_ne_bytes())?;
        cur += 8;
    }
    // Завершающий NULL для envp
    user_space.write_user_bytes(cur, &0u64.to_ne_bytes())?;
    cur += 8;

    // Записываем пары (tag, val) для auxv
    for &(tag, val) in &auxv {
        user_space.write_user_bytes(cur, &tag.to_ne_bytes())?;
        cur += 8;
        user_space.write_user_bytes(cur, &val.to_ne_bytes())?;
        cur += 8;
    }

    Ok((final_sp, argv_addr))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_elf_header_validation() {
        let mut mock_header = [0u8; 64];
        mock_header[0..4].copy_from_slice(&ELF_MAGIC);
        mock_header[4] = ELFCLASS64;
        mock_header[5] = ELFDATA2LSB;
        mock_header[6] = EV_CURRENT;
        // e_type = ET_EXEC (2)
        mock_header[16] = 2;
        // e_machine = EM_X86_64 (62)
        mock_header[18] = 62;
        // e_phentsize = 56
        mock_header[54] = 56;

        let res = validate_header(&mock_header);
        assert!(res.is_ok());
        let h = res.unwrap();
        assert_eq!(h.e_type, ET_EXEC);
        assert_eq!(h.e_machine, EM_X86_64);
    }

    #[test_case]
    fn test_elf_header_invalid_magic() {
        let bad_header = [0u8; 64];
        assert_eq!(validate_header(&bad_header), Err(ElfError::InvalidMagic));
    }
}
