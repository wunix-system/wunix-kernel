use core::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Language {
    Russian = 0,
    English = 1,
}

static CURRENT_LANGUAGE: AtomicU8 = AtomicU8::new(Language::Russian as u8);

pub fn current_language() -> Language {
    match CURRENT_LANGUAGE.load(Ordering::Relaxed) {
        0 => Language::Russian,
        _ => Language::English,
    }
}

pub fn set_language(lang: Language) {
    CURRENT_LANGUAGE.store(lang as u8, Ordering::Relaxed);
}

// Разбор параметров командной строки загрузчика (например, lang=ru или lang=en)
pub fn init_from_cmdline(cmdline: &str) {
    for token in cmdline.split_whitespace() {
        if let Some((k, v)) = token.split_once('=') {
            if k.eq_ignore_ascii_case("lang") || k.eq_ignore_ascii_case("locale") {
                match v {
                    "ru" | "RU" | "ru_RU" | "russian" => set_language(Language::Russian),
                    "en" | "EN" | "en_US" | "english" => set_language(Language::English),
                    _ => {}
                }
            }
        } else if token.eq_ignore_ascii_case("ru") || token.eq_ignore_ascii_case("russian") {
            set_language(Language::Russian);
        } else if token.eq_ignore_ascii_case("en") || token.eq_ignore_ascii_case("english") {
            set_language(Language::English);
        }
    }
}

// Быстрый перевод пары строк для русского и английского вариантов
#[inline]
pub fn t(ru: &'static str, en: &'static str) -> &'static str {
    match current_language() {
        Language::Russian => ru,
        Language::English => en,
    }
}

// Именованные ключи сообщений системы для масштабируемой локализации
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MsgKey {
    SystemReady,
    PromptInput,
    InterruptCancel,
    Cpu,
    Memory,
    Display,
    Timer,
    Keyboard,
    Uptime,
    Ring3Verified,
}

// Получение локализованного текста по типизированному ключу каталога
pub fn tr(key: MsgKey) -> &'static str {
    match current_language() {
        Language::Russian => match key {
            MsgKey::SystemReady => "wunix: система готова к работе",
            MsgKey::PromptInput => "wunix: введите текст с клавиатуры (переключение раскладки: Alt+Shift)...",
            MsgKey::InterruptCancel => "^C (ввод отменен)",
            MsgKey::Cpu => "процессор",
            MsgKey::Memory => "память",
            MsgKey::Display => "экран",
            MsgKey::Timer => "таймер",
            MsgKey::Keyboard => "клавиатура",
            MsgKey::Uptime => "время работы",
            MsgKey::Ring3Verified => "wunix: пространство пользователя (Ring 3) успешно верифицировано",
        },
        Language::English => match key {
            MsgKey::SystemReady => "wunix: system ready",
            MsgKey::PromptInput => "wunix: type something on keyboard (layout switch: Alt+Shift)...",
            MsgKey::InterruptCancel => "^C (input cancelled)",
            MsgKey::Cpu => "cpu",
            MsgKey::Memory => "memory",
            MsgKey::Display => "display",
            MsgKey::Timer => "timer",
            MsgKey::Keyboard => "keyboard",
            MsgKey::Uptime => "uptime",
            MsgKey::Ring3Verified => "wunix: userspace (Ring 3) verified successfully",
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_i18n_translation() {
        set_language(Language::Russian);
        assert_eq!(t("Привет", "Hello"), "Привет");
        assert_eq!(tr(MsgKey::SystemReady), "wunix: система готова к работе");

        set_language(Language::English);
        assert_eq!(t("Привет", "Hello"), "Hello");
        assert_eq!(tr(MsgKey::SystemReady), "wunix: system ready");

        set_language(Language::Russian);
    }

    #[test_case]
    fn test_init_from_cmdline() {
        set_language(Language::Russian);
        init_from_cmdline("boot=limine lang=en quiet");
        assert_eq!(current_language(), Language::English);

        init_from_cmdline("lang=ru");
        assert_eq!(current_language(), Language::Russian);

        init_from_cmdline("locale=en_US");
        assert_eq!(current_language(), Language::English);
    }
}
