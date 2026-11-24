use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Language {
    Russian = 0,
    English = 1,
    German = 2,
    French = 3,
    Other = 4,
}

#[repr(transparent)]
struct SyncCell<T>(UnsafeCell<T>);
unsafe impl<T> Sync for SyncCell<T> {}
impl<T> SyncCell<T> {
    const fn new(val: T) -> Self {
        Self(UnsafeCell::new(val))
    }
    #[inline(always)]
    fn get(&self) -> *mut T {
        self.0.get()
    }
}

static CURRENT_LANGUAGE: AtomicU8 = AtomicU8::new(Language::Russian as u8);
static CURRENT_LOCALE_BUF: SyncCell<[u8; 32]> =
    SyncCell::new(*b"ru_RU.UTF-8\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0");
static CURRENT_LOCALE_LEN: SyncCell<usize> = SyncCell::new(11);

pub fn current_language() -> Language {
    match CURRENT_LANGUAGE.load(Ordering::Relaxed) {
        0 => Language::Russian,
        1 => Language::English,
        2 => Language::German,
        3 => Language::French,
        _ => Language::Other,
    }
}

pub fn set_language(lang: Language) {
    CURRENT_LANGUAGE.store(lang as u8, Ordering::Relaxed);
}

pub fn current_locale() -> &'static str {
    unsafe {
        let len = *CURRENT_LOCALE_LEN.get();
        let buf = &*CURRENT_LOCALE_BUF.get();
        core::str::from_utf8_unchecked(&buf[..len])
    }
}

pub fn set_locale_str(loc: &str) {
    let bytes = loc.trim().as_bytes();
    let len = bytes.len().min(31);
    unsafe {
        let buf = &mut *CURRENT_LOCALE_BUF.get();
        buf[..len].copy_from_slice(&bytes[..len]);
        buf[len] = 0;
        *CURRENT_LOCALE_LEN.get() = len;
    }
}

// Разбор параметров командной строки загрузчика (например, lang=ru, lang=en, lang=de, lang=fr)
pub fn init_from_cmdline(cmdline: &str) {
    for token in cmdline.split_whitespace() {
        if let Some((k, v)) = token.split_once('=') {
            if k.eq_ignore_ascii_case("lang") || k.eq_ignore_ascii_case("locale") {
                match v {
                    "ru" | "RU" | "russian" => {
                        set_locale_str("ru_RU.UTF-8");
                        set_language(Language::Russian);
                    }
                    "en" | "EN" | "english" => {
                        set_locale_str("en_US.UTF-8");
                        set_language(Language::English);
                    }
                    "de" | "DE" | "german" => {
                        set_locale_str("de_DE.UTF-8");
                        set_language(Language::German);
                    }
                    "fr" | "FR" | "french" => {
                        set_locale_str("fr_FR.UTF-8");
                        set_language(Language::French);
                    }
                    custom => {
                        set_locale_str(custom);
                        if custom.starts_with("ru") || custom.starts_with("RU") {
                            set_language(Language::Russian);
                        } else if custom.starts_with("en") || custom.starts_with("EN") {
                            set_language(Language::English);
                        } else if custom.starts_with("de") || custom.starts_with("DE") {
                            set_language(Language::German);
                        } else if custom.starts_with("fr") || custom.starts_with("FR") {
                            set_language(Language::French);
                        } else {
                            set_language(Language::Other);
                        }
                    }
                }
            }
        } else if token.eq_ignore_ascii_case("ru") || token.eq_ignore_ascii_case("russian") {
            set_locale_str("ru_RU.UTF-8");
            set_language(Language::Russian);
        } else if token.eq_ignore_ascii_case("en") || token.eq_ignore_ascii_case("english") {
            set_locale_str("en_US.UTF-8");
            set_language(Language::English);
        } else if token.eq_ignore_ascii_case("de") || token.eq_ignore_ascii_case("german") {
            set_locale_str("de_DE.UTF-8");
            set_language(Language::German);
        } else if token.eq_ignore_ascii_case("fr") || token.eq_ignore_ascii_case("french") {
            set_locale_str("fr_FR.UTF-8");
            set_language(Language::French);
        }
    }
}

// Быстрый перевод пары строк для русского и английского вариантов
#[inline]
pub fn t(ru: &'static str, en: &'static str) -> &'static str {
    match current_language() {
        Language::Russian => ru,
        _ => en,
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
        Language::German => match key {
            MsgKey::SystemReady => "wunix: System ist betriebsbereit",
            MsgKey::PromptInput => "wunix: Tastatureingabe (Tastaturbelegung wechseln: Alt+Shift)...",
            MsgKey::InterruptCancel => "^C (Eingabe abgebrochen)",
            MsgKey::Cpu => "Prozessor",
            MsgKey::Memory => "Speicher",
            MsgKey::Display => "Bildschirm",
            MsgKey::Timer => "Timer",
            MsgKey::Keyboard => "Tastatur",
            MsgKey::Uptime => "Betriebszeit",
            MsgKey::Ring3Verified => "wunix: Benutzerbereich (Ring 3) erfolgreich verifiziert",
        },
        Language::French => match key {
            MsgKey::SystemReady => "wunix: système prêt",
            MsgKey::PromptInput => "wunix: saisissez du texte (changement de disposition: Alt+Shift)...",
            MsgKey::InterruptCancel => "^C (saisie annulée)",
            MsgKey::Cpu => "processeur",
            MsgKey::Memory => "mémoire",
            MsgKey::Display => "affichage",
            MsgKey::Timer => "minuteur",
            MsgKey::Keyboard => "clavier",
            MsgKey::Uptime => "temps de fonctionnement",
            MsgKey::Ring3Verified => "wunix: espace utilisateur (Ring 3) vérifié avec succès",
        },
        Language::Other => match key {
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

        set_language(Language::German);
        assert_eq!(tr(MsgKey::SystemReady), "wunix: System ist betriebsbereit");

        set_language(Language::Russian);
    }

    #[test_case]
    fn test_init_from_cmdline() {
        set_language(Language::Russian);
        init_from_cmdline("boot=limine lang=en quiet");
        assert_eq!(current_language(), Language::English);
        assert_eq!(current_locale(), "en_US.UTF-8");

        init_from_cmdline("lang=de");
        assert_eq!(current_language(), Language::German);
        assert_eq!(current_locale(), "de_DE.UTF-8");

        init_from_cmdline("lang=ru");
        assert_eq!(current_language(), Language::Russian);
        assert_eq!(current_locale(), "ru_RU.UTF-8");

        init_from_cmdline("locale=fr_FR.UTF-8");
        assert_eq!(current_language(), Language::French);
        assert_eq!(current_locale(), "fr_FR.UTF-8");
    }
}
