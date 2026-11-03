use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

use super::vfs::{Inode, InodeType, VfsError};

// Парсит восьмеричное число из текстового поля заголовка TAR (размер, права доступа).
// По стандарту POSIX ustar поле завершается нуль-терминатором или пробелом.
fn parse_octal(bytes: &[u8]) -> usize {
    let mut val = 0usize;
    let mut started = false;

    for &b in bytes {
        if (b'0'..=b'7').contains(&b) {
            val = val.saturating_mul(8).saturating_add((b - b'0') as usize);
            started = true;
        } else if b == 0 || b == b' ' {
            if started {
                break;
            }
        } else {
            break;
        }
    }

    val
}

// Извлекает ASCII-строку из фиксированного байтового буфера до первого нуль-байта
fn parse_str(bytes: &[u8]) -> &str {
    let len = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    core::str::from_utf8(&bytes[..len]).unwrap_or("")
}

// Создает цепочку вложенных директорий, если они еще не существуют
fn ensure_dir_path(mut current: Arc<dyn Inode>, segments: &[&str]) -> Result<Arc<dyn Inode>, VfsError> {
    for &seg in segments {
        if seg.is_empty() || seg == "." {
            continue;
        }
        let next = match current.lookup(seg) {
            Ok(node) => node,
            Err(VfsError::NotFound) => current.create(seg, InodeType::Directory)?,
            Err(err) => return Err(err),
        };
        current = next;
    }
    Ok(current)
}

// Распаковывает архив формата POSIX ustar поверх целевой директории VFS.
// Возвращает количество успешно созданных файлов и ссылок.
pub fn unpack_tar(tar_bytes: &[u8], target_dir: &Arc<dyn Inode>) -> Result<usize, VfsError> {
    let mut offset = 0usize;
    let mut files_unpacked = 0usize;

    while offset + 512 <= tar_bytes.len() {
        let header = &tar_bytes[offset..offset + 512];

        // Два подряд пустых 512-байтных блока по стандарту TAR означают конец архива
        if header.iter().all(|&b| b == 0) {
            break;
        }

        let raw_name = parse_str(&header[0..100]);
        let magic = parse_str(&header[257..263]);
        let raw_prefix = if magic.starts_with("ustar") {
            parse_str(&header[345..500])
        } else {
            ""
        };

        let full_path = if !raw_prefix.is_empty() {
            format!("{}/{}", raw_prefix, raw_name)
        } else {
            String::from(raw_name)
        };

        // Нормализуем путь внутри архива: отсекаем ведущие './' и слеши
        let trimmed_path = full_path
            .trim_start_matches('/')
            .trim_start_matches("./")
            .trim_end_matches('/');

        if trimmed_path.is_empty() {
            offset += 512;
            continue;
        }

        let mode = parse_octal(&header[100..108]) as u16;
        let uid = parse_octal(&header[108..116]) as u32;
        let gid = parse_octal(&header[116..124]) as u32;
        let size = parse_octal(&header[124..136]);
        let typeflag = header[156];
        let linkname = parse_str(&header[157..257]);

        let segments: Vec<&str> = trimmed_path.split('/').collect();
        let (dir_segments, leaf_name) = if segments.len() > 1 {
            (&segments[..segments.len() - 1], segments[segments.len() - 1])
        } else {
            (&segments[..0], segments[0])
        };

        let parent_dir = ensure_dir_path(target_dir.clone(), dir_segments)?;
        let data_offset = offset + 512;
        let data_blocks = (size + 511) / 512;

        match typeflag {
            // Директория
            b'5' => {
                if !leaf_name.is_empty() {
                    let dir_node = match parent_dir.lookup(leaf_name) {
                        Ok(node) => {
                            if node.metadata().itype != InodeType::Directory {
                                let _ = parent_dir.unlink(leaf_name);
                                parent_dir.create(leaf_name, InodeType::Directory)?
                            } else {
                                node
                            }
                        }
                        Err(VfsError::NotFound) => {
                            parent_dir.create(leaf_name, InodeType::Directory)?
                        }
                        Err(e) => return Err(e),
                    };
                    if mode != 0 {
                        let _ = dir_node.chmod(mode);
                    }
                    let _ = dir_node.chown(uid, gid);
                }
            }
            // Символическая ссылка
            b'2' => {
                let link_node = match parent_dir.lookup(leaf_name) {
                    Ok(node) => {
                        if node.metadata().itype != InodeType::Symlink {
                            let _ = parent_dir.unlink(leaf_name);
                            parent_dir.create(leaf_name, InodeType::Symlink)?
                        } else {
                            node
                        }
                    }
                    Err(VfsError::NotFound) => parent_dir.create(leaf_name, InodeType::Symlink)?,
                    Err(e) => return Err(e),
                };
                link_node.write_at(0, linkname.as_bytes())?;
                if mode != 0 {
                    let _ = link_node.chmod(mode);
                }
                let _ = link_node.chown(uid, gid);
                files_unpacked += 1;
            }
            // Обычный файл (b'0' или нулевой байт в старых архивах)
            b'0' | 0 => {
                if data_offset + size <= tar_bytes.len() {
                    let file_data = &tar_bytes[data_offset..data_offset + size];
                    let file_node = match parent_dir.lookup(leaf_name) {
                        Ok(node) => {
                            if node.metadata().itype != InodeType::File {
                                let _ = parent_dir.unlink(leaf_name);
                                parent_dir.create(leaf_name, InodeType::File)?
                            } else {
                                node
                            }
                        }
                        Err(VfsError::NotFound) => parent_dir.create(leaf_name, InodeType::File)?,
                        Err(e) => return Err(e),
                    };
                    file_node.write_at(0, file_data)?;
                    if mode != 0 {
                        let _ = file_node.chmod(mode);
                    }
                    let _ = file_node.chown(uid, gid);
                    files_unpacked += 1;
                }
            }
            _ => {
                // Неподдерживаемый тип записи TAR — пропускаем блок данных
            }
        }

        offset += 512 + data_blocks * 512;
    }

    Ok(files_unpacked)
}
