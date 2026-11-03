use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use x86_64::instructions::port::Port;

// Базовая тактовая частота осциллятора 8254 PIT (1.193182 МГц), унаследованная еще от оригинальной IBM PC XT.
const PIT_BASE_FREQUENCY_HZ: u32 = 1_193_182;

// Порты ввода-вывода контроллера 8254 PIT
const PIT_CHANNEL_0_DATA: u16 = 0x40;
const PIT_COMMAND_PORT: u16 = 0x43;

// Командный байт 0x36 (0b0011_0110):
// - Биты 7..6 = 00: выбор Канала 0 (подключен напрямую к IRQ0 контроллера прерываний)
// - Биты 5..4 = 11: доступ к 16-битному счетчику в два захода (сначала младший байт, затем старший)
// - Биты 3..1 = 011: Режим 3 (генератор прямоугольных импульсов / square wave)
// - Бит 0 = 0: 16-битный двоичный счетчик (не BCD)
const PIT_MODE_SQUARE_WAVE: u8 = 0x36;

static TICKS: AtomicU64 = AtomicU64::new(0);
static TIMER_FREQ_HZ: AtomicU32 = AtomicU32::new(100);

pub fn init(frequency_hz: u32) {
    let freq = frequency_hz.clamp(19, PIT_BASE_FREQUENCY_HZ);
    TIMER_FREQ_HZ.store(freq, Ordering::Relaxed);

    // Вычисляем 16-битный делитель базовой частоты.
    // При делителе 0 таймер интерпретирует его как 65536 (минимальная частота ~18.2 Гц).
    let divisor = (PIT_BASE_FREQUENCY_HZ / freq) as u16;

    unsafe {
        let mut cmd_port = Port::<u8>::new(PIT_COMMAND_PORT);
        let mut data_port = Port::<u8>::new(PIT_CHANNEL_0_DATA);

        // Переводим канал 0 в режим генератора прямоугольных импульсов
        cmd_port.write(PIT_MODE_SQUARE_WAVE);

        // 8254 ожидает делитель строго побайтово: low byte, затем high byte
        data_port.write((divisor & 0xFF) as u8);
        data_port.write(((divisor >> 8) & 0xFF) as u8);
    }
}

#[inline]
pub fn on_tick() {
    TICKS.fetch_add(1, Ordering::Relaxed);
}

#[inline]
pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

pub fn uptime_ms() -> u64 {
    let freq = TIMER_FREQ_HZ.load(Ordering::Relaxed) as u64;
    if freq == 0 {
        return 0;
    }
    (ticks() * 1000) / freq
}

pub fn sleep_ms(ms: u64) {
    let freq = TIMER_FREQ_HZ.load(Ordering::Relaxed) as u64;
    if freq == 0 {
        return;
    }

    // Округляем вверх, чтобы гарантированно проспать не меньше запрошенного времени
    let wait_ticks = (ms * freq + 999) / 1000;
    let start = ticks();

    while ticks().saturating_sub(start) < wait_ticks {
        // Уступаем квант времени другим готовым потокам, если они есть в очереди
        crate::task::yield_now();
        // hlt переводит процессор в спящий режим до следующего аппаратного прерывания (IRQ0),
        // исключая бесполезный 100% нагрев CPU в пустом цикле busy-wait.
        x86_64::instructions::hlt();
    }
}
