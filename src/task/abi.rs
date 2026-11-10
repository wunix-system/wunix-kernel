//! Подсистема ABI и изолированных сред исполнения (Миров / Personalities) ядра Wunix.
//!
//! Историческая проблема Wunix заключалась в том, что системные вызовы Linux и Windows NT
//! попадали в одну общую таблицу номеров. Номер 6 в Windows NT (NtClose) пересекался
//! с номером 6 в Linux (SYS_LSTAT), из-за чего закрытие хэндла превращалось в попытку
//! прочитать метаданные файла через lstat.
//!
//! Данный модуль разделяет процессы на изолированные миры (ABI Personalities):
//! каждый процесс принадлежит строго одной подсистеме, и при вызове инструкции `syscall`
//! ядро маршрутизирует запрос исключительно в диспетчер целевого мира.

/// Идентификатор подсистемы / мира исполнения процесса.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbiPersonality {
    /// Родной Linux x86_64 ABI: стандартные системные вызовы Linux
    /// (SYS_read = 0, SYS_write = 1, SYS_open = 2, SYS_close = 3, SYS_lstat = 6...).
    Linux,

    /// Подсистема Windows NT: нативные системные вызовы NT API через SSN (System Service Numbers)
    /// (NtClose = 0x0006, NtOpenFile = 0x000A, NtCreateFile = 0x0012, NtReadFile = 0x0013...).
    WindowsNt,

    /// Подсистема Darwin / macOS: класс системных вызовов BSD (0x0200_0000 | nr) и Mach traps.
    Darwin,
}

impl AbiPersonality {
    /// Человекочитаемое имя подсистемы для отладки и системных логов.
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Linux => "Linux x86_64",
            Self::WindowsNt => "Windows NT x64",
            Self::Darwin => "macOS / Darwin x86_64",
        }
    }
}

impl core::fmt::Display for AbiPersonality {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_abi_personalities() {
        assert_ne!(AbiPersonality::Linux, AbiPersonality::WindowsNt);
        assert_ne!(AbiPersonality::Linux, AbiPersonality::Darwin);
        assert_ne!(AbiPersonality::WindowsNt, AbiPersonality::Darwin);

        assert_eq!(AbiPersonality::Linux.name(), "Linux x86_64");
        assert_eq!(AbiPersonality::WindowsNt.name(), "Windows NT x64");
        assert_eq!(AbiPersonality::Darwin.name(), "macOS / Darwin x86_64");
    }
}
