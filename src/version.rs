macro_rules! kernel_name { () => { "wunix" }; }
macro_rules! kernel_stage { () => { "pre-alpha" }; }
macro_rules! kernel_arch { () => { "x86_64-wunix" }; }

// Источником версии ядра служит Cargo.toml, чтобы исключить расхождения
// и не обновлять строку выпуска вручную при релизах.
//
// Макрос concat!() в Rust принимает только литералы и макросы этапа компиляции
// (константы-идентификаторы не поддерживаются синтаксическим анализатором).
// Для единого источника правды части строки вынесены в макросы подстановки.
pub const NAME: &str = kernel_name!();
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const STAGE: &str = kernel_stage!();
pub const ARCH: &str = kernel_arch!();
pub const FULL_VERSION: &str = concat!(
    kernel_name!(),
    " ",
    env!("CARGO_PKG_VERSION"),
    "-",
    kernel_stage!(),
    " (",
    kernel_arch!(),
    ")"
);
