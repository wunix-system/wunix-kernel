use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

use crate::fs::registry::{
    parse_path_or_nt, reg_create_key, reg_delete_value, reg_enum_keys,
    reg_query_value, reg_set_value, Hive, RegError, RegType, RegValue,
};

pub const STATUS_SUCCESS: u64 = 0x0000_0000;
pub const STATUS_UNSUCCESSFUL: u64 = 0xC000_0001;
pub const STATUS_INVALID_PARAMETER: u64 = 0xC000_000D;
pub const STATUS_ACCESS_DENIED: u64 = 0xC000_0022;
pub const STATUS_BUFFER_TOO_SMALL: u64 = 0xC000_0023;
pub const STATUS_OBJECT_NAME_NOT_FOUND: u64 = 0xC000_0034;

static NEXT_REG_HANDLE: AtomicU64 = AtomicU64::new(0x1000);
static REG_HANDLE_TABLE: Mutex<BTreeMap<u64, String>> = Mutex::new(BTreeMap::new());

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct UnicodeString {
    pub length: u16,
    pub maximum_length: u16,
    pub pad: u32,
    pub buffer: u64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ObjectAttributes {
    pub length: u32,
    pub pad1: u32,
    pub root_directory: u64,
    pub object_name: u64,
    pub attributes: u32,
    pub pad2: u32,
    pub security_descriptor: u64,
    pub security_quality_of_service: u64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct KeyValuePartialInformation {
    pub title_index: u32,
    pub val_type: u32,
    pub data_length: u32,
}

pub fn read_unicode_string(str_ptr: u64) -> Result<String, u64> {
    if str_ptr == 0 || str_ptr >= 0x0000_8000_0000_0000 {
        return Err(STATUS_INVALID_PARAMETER);
    }
    let u_str = unsafe { core::ptr::read_unaligned(str_ptr as *const UnicodeString) };
    if u_str.length == 0 || u_str.buffer == 0 {
        return Ok(String::new());
    }

    let num_wchars = (u_str.length / 2) as usize;
    if num_wchars > 512 {
        return Err(STATUS_INVALID_PARAMETER);
    }

    let mut chars = Vec::with_capacity(num_wchars);
    for i in 0..num_wchars {
        let wchar_addr = u_str.buffer + (i * 2) as u64;
        if wchar_addr >= 0x0000_8000_0000_0000 {
            return Err(STATUS_INVALID_PARAMETER);
        }
        let w = unsafe { core::ptr::read_unaligned(wchar_addr as *const u16) };
        chars.push(w);
    }

    let decoded = core::char::decode_utf16(chars.into_iter())
        .map(|r| r.unwrap_or('\u{FFFD}'))
        .collect();
    Ok(decoded)
}

pub fn nt_open_key(
    key_handle_out_ptr: u64,
    _desired_access: u64,
    object_attributes_ptr: u64,
) -> u64 {
    if key_handle_out_ptr == 0 || object_attributes_ptr == 0 {
        return STATUS_INVALID_PARAMETER;
    }

    let obj_attr = unsafe {
        core::ptr::read_unaligned(object_attributes_ptr as *const ObjectAttributes)
    };
    let path_str = match read_unicode_string(obj_attr.object_name) {
        Ok(s) => s,
        Err(e) => return e,
    };

    let vfs_path = match parse_path_or_nt(&path_str) {
        Ok(p) => p,
        Err(_) => return STATUS_OBJECT_NAME_NOT_FOUND,
    };

    let node = match crate::fs::resolve_path(&vfs_path, true) {
        Ok(n) => n,
        Err(_) => return STATUS_OBJECT_NAME_NOT_FOUND,
    };

    if node.metadata().itype != crate::fs::InodeType::Directory {
        return STATUS_OBJECT_NAME_NOT_FOUND;
    }

    let handle = NEXT_REG_HANDLE.fetch_add(4, Ordering::Relaxed);
    REG_HANDLE_TABLE.lock().insert(handle, vfs_path);

    unsafe {
        core::ptr::write_unaligned(key_handle_out_ptr as *mut u64, handle);
    }
    STATUS_SUCCESS
}

pub fn nt_create_key(
    key_handle_out_ptr: u64,
    _desired_access: u64,
    object_attributes_ptr: u64,
    _title_index: u64,
    _class_ptr: u64,
    _create_options: u64,
    _disposition_ptr: u64,
) -> u64 {
    if key_handle_out_ptr == 0 || object_attributes_ptr == 0 {
        return STATUS_INVALID_PARAMETER;
    }

    let obj_attr = unsafe {
        core::ptr::read_unaligned(object_attributes_ptr as *const ObjectAttributes)
    };
    let path_str = match read_unicode_string(obj_attr.object_name) {
        Ok(s) => s,
        Err(e) => return e,
    };

    let vfs_path = match parse_path_or_nt(&path_str) {
        Ok(p) => p,
        Err(_) => {
            if let Some((hive, sub)) = Hive::from_prefix(&path_str) {
                match reg_create_key(hive, sub) {
                    Ok(p) => p,
                    Err(_) => return STATUS_UNSUCCESSFUL,
                }
            } else {
                return STATUS_INVALID_PARAMETER;
            }
        }
    };

    let _ = crate::fs::mkdir_p(&vfs_path);

    let handle = NEXT_REG_HANDLE.fetch_add(4, Ordering::Relaxed);
    REG_HANDLE_TABLE.lock().insert(handle, vfs_path);

    unsafe {
        core::ptr::write_unaligned(key_handle_out_ptr as *mut u64, handle);
    }
    STATUS_SUCCESS
}

pub fn nt_query_value_key(
    key_handle: u64,
    value_name_ptr: u64,
    _key_value_info_class: u64,
    key_value_info_ptr: u64,
    length: u64,
    result_length_ptr: u64,
) -> u64 {
    let key_path = {
        let table = REG_HANDLE_TABLE.lock();
        match table.get(&key_handle) {
            Some(p) => p.clone(),
            None => return STATUS_INVALID_PARAMETER,
        }
    };

    let val_name = match read_unicode_string(value_name_ptr) {
        Ok(s) => s,
        Err(e) => return e,
    };

    let reg_val = match reg_query_value(&key_path, &val_name) {
        Ok(v) => v,
        Err(RegError::ValueNotFound) => return STATUS_OBJECT_NAME_NOT_FOUND,
        Err(_) => return STATUS_UNSUCCESSFUL,
    };

    let raw_data = reg_val.serialize();
    let payload = if raw_data.len() >= 4 {
        &raw_data[4..]
    } else {
        &[]
    };

    let header_size = core::mem::size_of::<KeyValuePartialInformation>() as u64;
    let total_required = header_size + payload.len() as u64;

    if result_length_ptr != 0 {
        unsafe {
            core::ptr::write_unaligned(result_length_ptr as *mut u32, total_required as u32);
        }
    }

    if length < header_size {
        return STATUS_BUFFER_TOO_SMALL;
    }

    let info = KeyValuePartialInformation {
        title_index: 0,
        val_type: reg_val.reg_type() as u32,
        data_length: payload.len() as u32,
    };

    unsafe {
        core::ptr::write_unaligned(key_value_info_ptr as *mut KeyValuePartialInformation, info);
    }

    let copy_data_len = ((length - header_size) as usize).min(payload.len());
    if copy_data_len > 0 {
        unsafe {
            let dst = (key_value_info_ptr + header_size) as *mut u8;
            core::ptr::copy_nonoverlapping(payload.as_ptr(), dst, copy_data_len);
        }
    }

    if length < total_required {
        STATUS_BUFFER_TOO_SMALL
    } else {
        STATUS_SUCCESS
    }
}

pub fn nt_set_value_key(
    key_handle: u64,
    value_name_ptr: u64,
    _title_index: u64,
    val_type: u64,
    data_ptr: u64,
    data_length: u64,
) -> u64 {
    let key_path = {
        let table = REG_HANDLE_TABLE.lock();
        match table.get(&key_handle) {
            Some(p) => p.clone(),
            None => return STATUS_INVALID_PARAMETER,
        }
    };

    let val_name = match read_unicode_string(value_name_ptr) {
        Ok(s) => s,
        Err(e) => return e,
    };

    let payload = if data_length > 0 && data_ptr != 0 {
        unsafe { core::slice::from_raw_parts(data_ptr as *const u8, data_length as usize) }
    } else {
        &[]
    };

    let reg_type = match RegType::from_u32(val_type as u32) {
        Some(t) => t,
        None => return STATUS_INVALID_PARAMETER,
    };

    let mut full_bytes = Vec::new();
    full_bytes.extend_from_slice(&(reg_type as u32).to_le_bytes());
    full_bytes.extend_from_slice(payload);

    let reg_value = match RegValue::deserialize(&full_bytes) {
        Ok(v) => v,
        Err(_) => RegValue::Binary(payload.to_vec()),
    };

    match reg_set_value(&key_path, &val_name, &reg_value) {
        Ok(()) => STATUS_SUCCESS,
        Err(_) => STATUS_UNSUCCESSFUL,
    }
}

pub fn nt_enumerate_key(
    key_handle: u64,
    index: u64,
    _key_information_class: u64,
    key_information_ptr: u64,
    length: u64,
    result_length_ptr: u64,
) -> u64 {
    let key_path = {
        let table = REG_HANDLE_TABLE.lock();
        match table.get(&key_handle) {
            Some(p) => p.clone(),
            None => return STATUS_INVALID_PARAMETER,
        }
    };

    let subkeys = match reg_enum_keys(&key_path) {
        Ok(k) => k,
        Err(_) => return STATUS_UNSUCCESSFUL,
    };

    if (index as usize) >= subkeys.len() {
        return STATUS_OBJECT_NAME_NOT_FOUND;
    }

    let name = &subkeys[index as usize];
    let name_utf16: Vec<u16> = name.encode_utf16().collect();
    let name_bytes_len = (name_utf16.len() * 2) as u32;

    let total_required = 16 + name_bytes_len as u64;
    if result_length_ptr != 0 {
        unsafe {
            core::ptr::write_unaligned(result_length_ptr as *mut u32, total_required as u32);
        }
    }

    if length < 16 {
        return STATUS_BUFFER_TOO_SMALL;
    }

    // Записываем заголовок: [LastWriteTime: 8 bytes][TitleIndex: 4 bytes][NameLength: 4 bytes]
    unsafe {
        let base = key_information_ptr as *mut u8;
        core::ptr::write_bytes(base, 0, 16);
        core::ptr::write_unaligned(base.add(12) as *mut u32, name_bytes_len);

        let copy_len = ((length - 16) as usize).min(name_bytes_len as usize);
        if copy_len > 0 {
            core::ptr::copy_nonoverlapping(name_utf16.as_ptr() as *const u8, base.add(16), copy_len);
        }
    }

    if length < total_required {
        STATUS_BUFFER_TOO_SMALL
    } else {
        STATUS_SUCCESS
    }
}

pub fn nt_delete_value_key(key_handle: u64, value_name_ptr: u64) -> u64 {
    let key_path = {
        let table = REG_HANDLE_TABLE.lock();
        match table.get(&key_handle) {
            Some(p) => p.clone(),
            None => return STATUS_INVALID_PARAMETER,
        }
    };

    let val_name = match read_unicode_string(value_name_ptr) {
        Ok(s) => s,
        Err(e) => return e,
    };

    match reg_delete_value(&key_path, &val_name) {
        Ok(()) => STATUS_SUCCESS,
        Err(RegError::ValueNotFound) => STATUS_OBJECT_NAME_NOT_FOUND,
        Err(_) => STATUS_UNSUCCESSFUL,
    }
}

pub fn nt_close_reg_handle(handle: u64) -> bool {
    REG_HANDLE_TABLE.lock().remove(&handle).is_some()
}
