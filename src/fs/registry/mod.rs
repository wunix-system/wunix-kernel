pub mod populate;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use crate::fs::vfs::{InodeType, VfsError};

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegType {
    None = 0,
    Sz = 1,
    ExpandSz = 2,
    Binary = 3,
    DWord = 4,
    DWordBigEndian = 5,
    Link = 6,
    MultiSz = 7,
    ResourceList = 8,
    FullResourceDescriptor = 9,
    ResourceRequirementsList = 10,
    QWord = 11,
}

impl RegType {
    pub fn from_u32(val: u32) -> Option<Self> {
        match val {
            0 => Some(Self::None),
            1 => Some(Self::Sz),
            2 => Some(Self::ExpandSz),
            3 => Some(Self::Binary),
            4 => Some(Self::DWord),
            5 => Some(Self::DWordBigEndian),
            6 => Some(Self::Link),
            7 => Some(Self::MultiSz),
            8 => Some(Self::ResourceList),
            9 => Some(Self::FullResourceDescriptor),
            10 => Some(Self::ResourceRequirementsList),
            11 => Some(Self::QWord),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegValue {
    None,
    Sz(String),
    ExpandSz(String),
    Binary(Vec<u8>),
    DWord(u32),
    DWordBigEndian(u32),
    MultiSz(Vec<String>),
    QWord(u64),
}

impl RegValue {
    pub fn reg_type(&self) -> RegType {
        match self {
            Self::None => RegType::None,
            Self::Sz(_) => RegType::Sz,
            Self::ExpandSz(_) => RegType::ExpandSz,
            Self::Binary(_) => RegType::Binary,
            Self::DWord(_) => RegType::DWord,
            Self::DWordBigEndian(_) => RegType::DWordBigEndian,
            Self::MultiSz(_) => RegType::MultiSz,
            Self::QWord(_) => RegType::QWord,
        }
    }

    pub fn serialize(&self) -> Vec<u8> {
        let type_id = self.reg_type() as u32;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&type_id.to_le_bytes());

        match self {
            Self::None => {}
            Self::Sz(s) | Self::ExpandSz(s) => {
                bytes.extend_from_slice(s.as_bytes());
            }
            Self::Binary(b) => {
                bytes.extend_from_slice(b);
            }
            Self::DWord(val) => {
                bytes.extend_from_slice(&val.to_le_bytes());
            }
            Self::DWordBigEndian(val) => {
                bytes.extend_from_slice(&val.to_be_bytes());
            }
            Self::MultiSz(strings) => {
                for s in strings {
                    bytes.extend_from_slice(s.as_bytes());
                    bytes.push(0);
                }
                bytes.push(0);
            }
            Self::QWord(val) => {
                bytes.extend_from_slice(&val.to_le_bytes());
            }
        }
        bytes
    }

    pub fn deserialize(raw: &[u8]) -> Result<Self, RegError> {
        if raw.len() < 4 {
            return Err(RegError::InvalidData);
        }
        let type_u32 = u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]);
        let reg_type = RegType::from_u32(type_u32).ok_or(RegError::InvalidData)?;
        let payload = &raw[4..];

        match reg_type {
            RegType::None => Ok(Self::None),
            RegType::Sz => {
                let s = core::str::from_utf8(payload).map_err(|_| RegError::InvalidData)?;
                Ok(Self::Sz(String::from(s.trim_end_matches('\0'))))
            }
            RegType::ExpandSz => {
                let s = core::str::from_utf8(payload).map_err(|_| RegError::InvalidData)?;
                Ok(Self::ExpandSz(String::from(s.trim_end_matches('\0'))))
            }
            RegType::Binary => Ok(Self::Binary(payload.to_vec())),
            RegType::DWord => {
                if payload.len() < 4 {
                    return Err(RegError::InvalidData);
                }
                let val = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
                Ok(Self::DWord(val))
            }
            RegType::DWordBigEndian => {
                if payload.len() < 4 {
                    return Err(RegError::InvalidData);
                }
                let val = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
                Ok(Self::DWordBigEndian(val))
            }
            RegType::MultiSz => {
                let mut strings = Vec::new();
                let mut start = 0;
                for (i, &b) in payload.iter().enumerate() {
                    if b == 0 {
                        if i > start {
                            if let Ok(s) = core::str::from_utf8(&payload[start..i]) {
                                strings.push(String::from(s));
                            }
                        }
                        start = i + 1;
                    }
                }
                Ok(Self::MultiSz(strings))
            }
            RegType::QWord => {
                if payload.len() < 8 {
                    return Err(RegError::InvalidData);
                }
                let val = u64::from_le_bytes([
                    payload[0], payload[1], payload[2], payload[3],
                    payload[4], payload[5], payload[6], payload[7],
                ]);
                Ok(Self::QWord(val))
            }
            _ => Ok(Self::Binary(payload.to_vec())),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hive {
    LocalMachine,
    CurrentUser,
    Users,
    ClassesRoot,
    CurrentConfig,
}

impl Hive {
    pub fn root_path(&self) -> &'static str {
        match self {
            Self::LocalMachine => "/win/registry/HKLM",
            Self::CurrentUser => "/win/registry/HKCU",
            Self::Users => "/win/registry/HKU",
            Self::ClassesRoot => "/win/registry/HKCR",
            Self::CurrentConfig => "/win/registry/HKCC",
        }
    }

    pub fn from_prefix(prefix: &str) -> Option<(Self, &str)> {
        let p = prefix.trim_start_matches('\\').trim_start_matches('/');
        if let Some(rest) = p.strip_prefix("HKEY_LOCAL_MACHINE") {
            Some((Self::LocalMachine, rest.trim_start_matches('\\').trim_start_matches('/')))
        } else if let Some(rest) = p.strip_prefix("HKLM") {
            Some((Self::LocalMachine, rest.trim_start_matches('\\').trim_start_matches('/')))
        } else if let Some(rest) = p.strip_prefix("HKEY_CURRENT_USER") {
            Some((Self::CurrentUser, rest.trim_start_matches('\\').trim_start_matches('/')))
        } else if let Some(rest) = p.strip_prefix("HKCU") {
            Some((Self::CurrentUser, rest.trim_start_matches('\\').trim_start_matches('/')))
        } else if let Some(rest) = p.strip_prefix("HKEY_USERS") {
            Some((Self::Users, rest.trim_start_matches('\\').trim_start_matches('/')))
        } else if let Some(rest) = p.strip_prefix("HKU") {
            Some((Self::Users, rest.trim_start_matches('\\').trim_start_matches('/')))
        } else if let Some(rest) = p.strip_prefix("HKEY_CLASSES_ROOT") {
            Some((Self::ClassesRoot, rest.trim_start_matches('\\').trim_start_matches('/')))
        } else if let Some(rest) = p.strip_prefix("HKCR") {
            Some((Self::ClassesRoot, rest.trim_start_matches('\\').trim_start_matches('/')))
        } else if let Some(rest) = p.strip_prefix("HKEY_CURRENT_CONFIG") {
            Some((Self::CurrentConfig, rest.trim_start_matches('\\').trim_start_matches('/')))
        } else if let Some(rest) = p.strip_prefix("HKCC") {
            Some((Self::CurrentConfig, rest.trim_start_matches('\\').trim_start_matches('/')))
        } else if let Some(rest) = p.strip_prefix("Registry\\Machine") {
            Some((Self::LocalMachine, rest.trim_start_matches('\\').trim_start_matches('/')))
        } else if let Some(rest) = p.strip_prefix("Registry\\User") {
            Some((Self::Users, rest.trim_start_matches('\\').trim_start_matches('/')))
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegError {
    KeyNotFound,
    ValueNotFound,
    InvalidPath,
    InvalidData,
    AccessDenied,
    IoError,
}

impl From<VfsError> for RegError {
    fn from(err: VfsError) -> Self {
        match err {
            VfsError::NotFound => RegError::KeyNotFound,
            VfsError::PermissionDenied => RegError::AccessDenied,
            VfsError::InvalidPath => RegError::InvalidPath,
            _ => RegError::IoError,
        }
    }
}

pub fn normalize_key_path(hive: Hive, subkey: &str) -> String {
    let base = hive.root_path();
    let sanitized = subkey.replace('\\', "/");
    let trimmed = sanitized.trim_matches('/');
    if trimmed.is_empty() {
        String::from(base)
    } else {
        format!("{}/{}", base, trimmed)
    }
}

pub fn parse_path_or_nt(input: &str) -> Result<String, RegError> {
    let clean = input.trim();
    if let Some((hive, subkey)) = Hive::from_prefix(clean) {
        Ok(normalize_key_path(hive, subkey))
    } else if clean.starts_with("/win/registry") {
        Ok(clean.replace('\\', "/"))
    } else {
        Err(RegError::InvalidPath)
    }
}

pub fn reg_create_key(hive: Hive, subkey: &str) -> Result<String, RegError> {
    let full_path = normalize_key_path(hive, subkey);
    crate::fs::mkdir_p(&full_path)?;
    Ok(full_path)
}

pub fn reg_open_key(hive: Hive, subkey: &str) -> Result<String, RegError> {
    let full_path = normalize_key_path(hive, subkey);
    let node = crate::fs::resolve_path(&full_path, true)?;
    if node.metadata().itype != InodeType::Directory {
        return Err(RegError::KeyNotFound);
    }
    Ok(full_path)
}

fn value_file_name(val_name: &str) -> &str {
    if val_name.is_empty() || val_name == "@" {
        "__default__"
    } else {
        val_name
    }
}

pub fn reg_set_value(key_path: &str, value_name: &str, value: &RegValue) -> Result<(), RegError> {
    let v_name = value_file_name(value_name);
    let full_value_path = format!("{}/{}", key_path.trim_end_matches('/'), v_name);
    let serialized = value.serialize();
    crate::fs::write_file(&full_value_path, &serialized)?;
    Ok(())
}

pub fn reg_query_value(key_path: &str, value_name: &str) -> Result<RegValue, RegError> {
    let v_name = value_file_name(value_name);
    let full_value_path = format!("{}/{}", key_path.trim_end_matches('/'), v_name);
    let raw = crate::fs::read_file(&full_value_path).map_err(|e| match e {
        VfsError::NotFound => RegError::ValueNotFound,
        other => RegError::from(other),
    })?;
    RegValue::deserialize(&raw)
}

pub fn reg_delete_value(key_path: &str, value_name: &str) -> Result<(), RegError> {
    let v_name = value_file_name(value_name);
    let full_value_path = format!("{}/{}", key_path.trim_end_matches('/'), v_name);
    crate::fs::unlink(&full_value_path)?;
    Ok(())
}

pub fn reg_enum_keys(key_path: &str) -> Result<Vec<String>, RegError> {
    let node = crate::fs::resolve_path(key_path, true)?;
    let entries = node.readdir()?;
    let mut keys = Vec::new();
    for entry in entries {
        if entry.itype == InodeType::Directory && entry.name != "." && entry.name != ".." {
            keys.push(entry.name);
        }
    }
    Ok(keys)
}

pub fn reg_enum_values(key_path: &str) -> Result<Vec<(String, RegType)>, RegError> {
    let node = crate::fs::resolve_path(key_path, true)?;
    let entries = node.readdir()?;
    let mut values = Vec::new();
    for entry in entries {
        if entry.itype == InodeType::File {
            let v_path = format!("{}/{}", key_path.trim_end_matches('/'), entry.name);
            if let Ok(raw) = crate::fs::read_file(&v_path) {
                if raw.len() >= 4 {
                    let type_u32 = u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]);
                    if let Some(reg_type) = RegType::from_u32(type_u32) {
                        let display_name = if entry.name == "__default__" {
                            String::from("")
                        } else {
                            entry.name
                        };
                        values.push((display_name, reg_type));
                    }
                }
            }
        }
    }
    Ok(values)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_reg_value_serialization() {
        let val_sz = RegValue::Sz(String::from("Wunix OS"));
        let bytes_sz = val_sz.serialize();
        let deser_sz = RegValue::deserialize(&bytes_sz).unwrap();
        assert_eq!(val_sz, deser_sz);

        let val_dw = RegValue::DWord(0x12345678);
        let bytes_dw = val_dw.serialize();
        let deser_dw = RegValue::deserialize(&bytes_dw).unwrap();
        assert_eq!(val_dw, deser_dw);

        let val_qw = RegValue::QWord(0x0102030405060708);
        let bytes_qw = val_qw.serialize();
        let deser_qw = RegValue::deserialize(&bytes_qw).unwrap();
        assert_eq!(val_qw, deser_qw);

        let val_multi = RegValue::MultiSz(alloc::vec![String::from("item1"), String::from("item2")]);
        let bytes_multi = val_multi.serialize();
        let deser_multi = RegValue::deserialize(&bytes_multi).unwrap();
        assert_eq!(val_multi, deser_multi);
    }

    #[test_case]
    fn test_hive_prefix_parsing() {
        let (hive, sub) = Hive::from_prefix("HKLM\\Software\\Microsoft").unwrap();
        assert_eq!(hive, Hive::LocalMachine);
        assert_eq!(sub, "Software\\Microsoft");

        let (hive, sub) = Hive::from_prefix("Registry\\Machine\\SYSTEM").unwrap();
        assert_eq!(hive, Hive::LocalMachine);
        assert_eq!(sub, "SYSTEM");

        let (hive, sub) = Hive::from_prefix("HKCU/Environment").unwrap();
        assert_eq!(hive, Hive::CurrentUser);
        assert_eq!(sub, "Environment");
    }
}

