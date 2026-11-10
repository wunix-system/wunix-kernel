use alloc::string::String;
use alloc::sync::Arc;
use spin::Mutex;
use super::registry::{read_unicode_string, ObjectAttributes, STATUS_INVALID_PARAMETER, STATUS_OBJECT_NAME_NOT_FOUND, STATUS_SUCCESS, STATUS_UNSUCCESSFUL};
use crate::fs::RegularFileHandle;

pub const FILE_OPENED: u64 = 1;
pub const FILE_CREATED: u64 = 2;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct FileStandardInformation {
    pub allocation_size: u64,
    pub end_of_file: u64,
    pub number_of_links: u32,
    pub delete_pending: u8,
    pub directory: u8,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ProcessBasicInformation {
    pub exit_status: i32,
    pub peb_base_address: u64,
    pub affinity_mask: u64,
    pub base_priority: i32,
    pub unique_process_id: u64,
    pub inherited_from_unique_process_id: u64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SystemBasicInformation {
    pub reserved: u32,
    pub timer_resolution: u32,
    pub page_size: u32,
    pub number_of_physical_pages: u32,
    pub lowest_physical_page: u32,
    pub highest_physical_page: u32,
    pub allocation_granularity: u32,
    pub minimum_user_mode_address: u64,
    pub maximum_user_mode_address: u64,
    pub active_processors_affinity_mask: u64,
    pub number_of_processors: u8,
}

pub fn convert_nt_path_to_vfs(nt_path: &str) -> String {
    let trimmed = nt_path.trim();
    let cleaned = if let Some(stripped) = trimmed.strip_prefix("\\??\\") {
        stripped
    } else if let Some(stripped) = trimmed.strip_prefix("\\DosDevices\\") {
        stripped
    } else {
        trimmed
    };
    crate::fs::path::translate_path(cleaned)
}

pub fn nt_open_file(
    file_handle_ptr: u64,
    desired_access: u64,
    obj_attr_ptr: u64,
    io_status_ptr: u64,
    _share_access: u64,
    _open_options: u64,
) -> u64 {
    nt_create_file(
        file_handle_ptr,
        desired_access,
        obj_attr_ptr,
        io_status_ptr,
        0,
        0,
        _share_access,
        1, // FILE_OPEN
        _open_options,
        0,
        0,
    )
}

pub fn nt_create_file(
    file_handle_ptr: u64,
    desired_access: u64,
    obj_attr_ptr: u64,
    io_status_ptr: u64,
    _alloc_size_ptr: u64,
    _file_attributes: u64,
    _share_access: u64,
    create_disposition: u64,
    _create_options: u64,
    _ea_buf: u64,
    _ea_len: u64,
) -> u64 {
    if file_handle_ptr == 0 || obj_attr_ptr == 0 {
        return STATUS_INVALID_PARAMETER;
    }

    let obj_attr = unsafe { *(obj_attr_ptr as *const ObjectAttributes) };
    if obj_attr.object_name == 0 {
        return STATUS_INVALID_PARAMETER;
    }

    let path_str = match read_unicode_string(obj_attr.object_name) {
        Ok(s) => s,
        Err(_) => return STATUS_INVALID_PARAMETER,
    };

    let vfs_path = convert_nt_path_to_vfs(&path_str);

    // Флаги открытия VFS
    let mut flags = 0u32;
    if (desired_access & 0x4000_0000) != 0 || (desired_access & 0x0002) != 0 {
        flags |= 0x2; // O_RDWR
    } else {
        flags |= 0x0; // O_RDONLY
    }

    // create_disposition: 1 = FILE_OPEN, 2 = FILE_CREATE, 3 = FILE_OPEN_IF, 4 = FILE_OVERWRITE
    if create_disposition == 2 || create_disposition == 3 || create_disposition == 4 {
        flags |= 0x40; // O_CREAT
    }
    if create_disposition == 4 {
        flags |= 0x200; // O_TRUNC
    }

    let node = match crate::fs::open(&vfs_path) {
        Ok(n) => {
            if (flags & 0x200) != 0 {
                let _ = n.truncate(0);
            }
            n
        }
        Err(_) => {
            if (flags & 0x40) != 0 {
                if crate::fs::write_file(&vfs_path, &[]).is_ok() {
                    match crate::fs::open(&vfs_path) {
                        Ok(n) => n,
                        Err(_) => return STATUS_OBJECT_NAME_NOT_FOUND,
                    }
                } else {
                    return STATUS_OBJECT_NAME_NOT_FOUND;
                }
            } else {
                return STATUS_OBJECT_NAME_NOT_FOUND;
            }
        }
    };

    let handle = Arc::new(RegularFileHandle {
        inode: node,
        offset: Mutex::new(0),
        flags: core::sync::atomic::AtomicU32::new(flags),
        path: Some(vfs_path),
    });

    let fd = crate::fs::insert_file_handle(handle);

    unsafe {
        core::ptr::write_unaligned(file_handle_ptr as *mut u64, fd);
        if io_status_ptr != 0 {
            core::ptr::write_unaligned(io_status_ptr as *mut u64, STATUS_SUCCESS);
            core::ptr::write_unaligned((io_status_ptr + 8) as *mut u64, FILE_OPENED);
        }
    }

    STATUS_SUCCESS
}

pub fn nt_query_information_file(
    file_handle: u64,
    io_status_ptr: u64,
    file_info_ptr: u64,
    length: u64,
    file_info_class: u64,
) -> u64 {
    if file_info_ptr == 0 {
        return STATUS_INVALID_PARAMETER;
    }

    let handle = match crate::fs::get_file_handle(file_handle) {
        Some(h) => h,
        None => return STATUS_INVALID_PARAMETER,
    };

    // Класс 5: FileStandardInformation
    if file_info_class == 5 {
        if length < core::mem::size_of::<FileStandardInformation>() as u64 {
            return STATUS_INVALID_PARAMETER;
        }

        let size = handle.as_inode().map(|i| i.metadata().size as u64).unwrap_or(0);
        let info = FileStandardInformation {
            allocation_size: (size + 4095) & !4095,
            end_of_file: size,
            number_of_links: 1,
            delete_pending: 0,
            directory: 0,
        };

        unsafe {
            core::ptr::write_unaligned(file_info_ptr as *mut FileStandardInformation, info);
            if io_status_ptr != 0 {
                core::ptr::write_unaligned(io_status_ptr as *mut u64, STATUS_SUCCESS);
                core::ptr::write_unaligned((io_status_ptr + 8) as *mut u64, core::mem::size_of::<FileStandardInformation>() as u64);
            }
        }
        return STATUS_SUCCESS;
    }

    STATUS_UNSUCCESSFUL
}

pub fn nt_query_information_process(
    _process_handle: u64,
    process_info_class: u64,
    process_info_ptr: u64,
    process_info_len: u64,
    return_len_ptr: u64,
) -> u64 {
    if process_info_ptr == 0 {
        return STATUS_INVALID_PARAMETER;
    }

    // Класс 0: ProcessBasicInformation
    if process_info_class == 0 {
        if process_info_len < core::mem::size_of::<ProcessBasicInformation>() as u64 {
            return STATUS_INVALID_PARAMETER;
        }

        let cur_pid = crate::task::current_thread_tgid();
        let parent_pid = crate::task::current_thread_parent_tgid();

        let pbi = ProcessBasicInformation {
            exit_status: 0,
            peb_base_address: crate::task::syslib::SYSLIB_INFO_VADDR,
            affinity_mask: 1,
            base_priority: 8,
            unique_process_id: cur_pid,
            inherited_from_unique_process_id: parent_pid,
        };

        unsafe {
            core::ptr::write_unaligned(process_info_ptr as *mut ProcessBasicInformation, pbi);
            if return_len_ptr != 0 {
                core::ptr::write_unaligned(return_len_ptr as *mut u32, core::mem::size_of::<ProcessBasicInformation>() as u32);
            }
        }
        return STATUS_SUCCESS;
    }

    STATUS_UNSUCCESSFUL
}

pub fn nt_query_system_information(
    system_info_class: u64,
    system_info_ptr: u64,
    system_info_len: u64,
    return_len_ptr: u64,
) -> u64 {
    if system_info_ptr == 0 {
        return STATUS_INVALID_PARAMETER;
    }

    // Класс 0: SystemBasicInformation
    if system_info_class == 0 {
        if system_info_len < core::mem::size_of::<SystemBasicInformation>() as u64 {
            return STATUS_INVALID_PARAMETER;
        }

        let sbi = SystemBasicInformation {
            reserved: 0,
            timer_resolution: 100000,
            page_size: 4096,
            number_of_physical_pages: 131072, // 512 MiB RAM
            lowest_physical_page: 1,
            highest_physical_page: 131072,
            allocation_granularity: 65536,
            minimum_user_mode_address: 0x10000,
            maximum_user_mode_address: 0x0000_7FFF_FFFF_FFFF,
            active_processors_affinity_mask: 1,
            number_of_processors: 1,
        };

        unsafe {
            core::ptr::write_unaligned(system_info_ptr as *mut SystemBasicInformation, sbi);
            if return_len_ptr != 0 {
                core::ptr::write_unaligned(return_len_ptr as *mut u32, core::mem::size_of::<SystemBasicInformation>() as u32);
            }
        }
        return STATUS_SUCCESS;
    }

    STATUS_UNSUCCESSFUL
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_nt_path_conversion() {
        assert_eq!(
            convert_nt_path_to_vfs("\\??\\C:\\Windows\\notepad.exe"),
            "/win/drives/c/Windows/notepad.exe"
        );
        assert_eq!(
            convert_nt_path_to_vfs("\\DosDevices\\D:\\Data\\file.txt"),
            "/win/drives/d/Data/file.txt"
        );
        assert_eq!(
            convert_nt_path_to_vfs("C:\\boot.ini"),
            "/win/drives/c/boot.ini"
        );
    }
}
