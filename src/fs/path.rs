use alloc::string::String;
use alloc::vec::Vec;

// Преобразует путь в канонический формат единого дерева Wunix:
// 1. Транслирует диски Windows (C:\... или c:/...) в /win/drives/c/...
// 2. Заменяет все обратные слеши на прямые.
// 3. Раскрывает компоненты '.' и '..'.
pub fn translate_path(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return String::from("/");
    }

    let bytes = trimmed.as_bytes();
    let mut intermediate = String::new();

    // Проверяем наличие префикса диска Windows (например, 'C:' или 'c:')
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        let drive_letter = (bytes[0] as char).to_ascii_lowercase();
        intermediate.push_str("/win/drives/");
        intermediate.push(drive_letter);
        if bytes.len() > 2 {
            let rest = &trimmed[2..];
            if !rest.starts_with('\\') && !rest.starts_with('/') {
                intermediate.push('/');
            }
            intermediate.push_str(rest);
        }
    } else {
        intermediate.push_str(trimmed);
    }

    normalize_path(&intermediate)
}

// Нормализует разделители путей и схлопывает относительные переходы ('.' и '..').
// Гарантирует абсолютный путь, начинающийся со слеша.
pub fn normalize_path(path: &str) -> String {
    let mut stack: Vec<&str> = Vec::new();

    // Обрабатываем и прямые, и обратные слеши одинаково
    for part in path.split(|c| c == '/' || c == '\\') {
        match part {
            "" | "." => {}
            ".." => {
                stack.pop();
            }
            valid => {
                stack.push(valid);
            }
        }
    }

    if stack.is_empty() {
        return String::from("/");
    }

    let mut result = String::new();
    for seg in stack {
        result.push('/');
        result.push_str(seg);
    }
    result
}

// Определяет, относится ли путь к пространству имен подсистемы Windows NT.
// Для путей внутри /win/... действует регистронезависимый поиск файлов.
pub fn is_windows_subsystem(normalized_path: &str) -> bool {
    normalized_path == "/win" || normalized_path.starts_with("/win/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_path_normalization() {
        assert_eq!(normalize_path("/a/b/../c/./d"), "/a/c/d");
        assert_eq!(normalize_path("/a/../../b"), "/b");
        assert_eq!(normalize_path(""), "/");
        assert_eq!(normalize_path("///"), "/");
    }

    #[test_case]
    fn test_windows_drive_translation() {
        assert_eq!(
            translate_path("C:\\Windows\\system.ini"),
            "/win/drives/c/Windows/system.ini"
        );
        assert_eq!(
            translate_path("c:/windows/system32"),
            "/win/drives/c/windows/system32"
        );
        assert_eq!(translate_path("C:"), "/win/drives/c");
        assert_eq!(translate_path("c:\\"), "/win/drives/c");
        assert!(is_windows_subsystem("/win/drives/c"));
        assert!(!is_windows_subsystem("/etc/os-release"));
    }
}
