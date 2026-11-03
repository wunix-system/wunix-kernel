use x86_64::instructions::interrupts;
use x86_64::instructions::port::Port;

const CMOS_ADDR: u16 = 0x70;
const CMOS_DATA: u16 = 0x71;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RtcDateTime {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

// Проверяем флаг UIP (Update In Progress) в регистре A (0x0A).
// Если микросхема RTC прямо сейчас обновляет внутренние счетчики времени (бит 7 = 1),
// считывание данных вернет рассинхронизированный мусор.
fn is_updating() -> bool {
    unsafe {
        let mut addr = Port::<u8>::new(CMOS_ADDR);
        let mut data = Port::<u8>::new(CMOS_DATA);
        // Бит 7 порта 0x70 управляет NMI. Оставляем 0, чтобы не заблокировать немаскируемые прерывания.
        addr.write(0x0A);
        (data.read() & 0x80) != 0
    }
}

fn read_cmos_register(reg: u8) -> u8 {
    unsafe {
        let mut addr = Port::<u8>::new(CMOS_ADDR);
        let mut data = Port::<u8>::new(CMOS_DATA);
        addr.write(reg);
        data.read()
    }
}

fn write_cmos_register(reg: u8, val: u8) {
    unsafe {
        let mut addr = Port::<u8>::new(CMOS_ADDR);
        let mut data = Port::<u8>::new(CMOS_DATA);
        addr.write(reg & 0x7F);
        data.write(val);
    }
}

fn bcd_to_binary(val: u8) -> u8 {
    (val & 0x0F) + ((val / 16) * 10)
}

fn binary_to_bcd(val: u8) -> u8 {
    ((val / 10) << 4) | (val % 10)
}

/// Считывает текущее время и дату из энергонезависимой памяти CMOS RTC (чип Motorola MC146818).
pub fn read_datetime() -> RtcDateTime {
    interrupts::without_interrupts(|| {
        // Дожидаемся окончания текущего тика обновления часов (длится около 1 мс)
        let mut spins = 0;
        while is_updating() && spins < 100_000 {
            spins += 1;
            core::hint::spin_loop();
        }

        // Выполняем двойное чтение: если между регистрами произошел перенос секунд
        // (например, 23:59:59 -> 00:00:00), повторный замер отловит расхождение.
        let mut sec = read_cmos_register(0x00);
        let mut min = read_cmos_register(0x02);
        let mut hour = read_cmos_register(0x04);
        let mut day = read_cmos_register(0x07);
        let mut month = read_cmos_register(0x08);
        let mut year = read_cmos_register(0x09);
        let century = read_cmos_register(0x32);

        loop {
            let last_sec = sec;
            let last_min = min;
            let last_hour = hour;
            let last_day = day;
            let last_month = month;
            let last_year = year;

            sec = read_cmos_register(0x00);
            min = read_cmos_register(0x02);
            hour = read_cmos_register(0x04);
            day = read_cmos_register(0x07);
            month = read_cmos_register(0x08);
            year = read_cmos_register(0x09);

            if last_sec == sec
                && last_min == min
                && last_hour == hour
                && last_day == day
                && last_month == month
                && last_year == year
            {
                break;
            }
        }

        let reg_b = read_cmos_register(0x0B);

        // Бит 2 регистра B указывает на формат данных: 0 = BCD (двоично-десятичный), 1 = двоичный.
        // Стандартный IBM PC BIOS инициализирует RTC в BCD-режиме.
        let is_bcd = (reg_b & 0x04) == 0;

        if is_bcd {
            sec = bcd_to_binary(sec);
            min = bcd_to_binary(min);
            hour = bcd_to_binary(hour & 0x7F) | (hour & 0x80);
            day = bcd_to_binary(day);
            month = bcd_to_binary(month);
            year = bcd_to_binary(year);
        }

        // Бит 1 регистра B задает 12- или 24-часовой формат.
        // В 12-часовом режиме бит 7 часа взведен для времени PM.
        let is_24h = (reg_b & 0x02) != 0;
        if !is_24h && (hour & 0x80) != 0 {
            hour = ((hour & 0x7F) + 12) % 24;
        }

        let full_year = if century > 0 && century < 100 {
            let cent = if is_bcd { bcd_to_binary(century) } else { century };
            (cent as u16) * 100 + (year as u16)
        } else {
            // Если BIOS не поддерживает регистр века 0x32, предполагаем 21 век
            2000 + (year as u16)
        };

        RtcDateTime {
            year: full_year,
            month,
            day,
            hour,
            minute: min,
            second: sec,
        }
    })
}

/// Преобразует календарную дату и время RTC в секунды с начала эпохи Unix (1970-01-01 00:00:00 UTC).
pub fn rtc_to_epoch(dt: &RtcDateTime) -> i64 {
    const DAYS_BEFORE_MONTH: [i64; 12] = [
        0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334,
    ];
    let year = dt.year as i64;
    let m = dt.month.clamp(1, 12) as usize;
    let day = dt.day.clamp(1, 31) as i64;

    let y = year - 1;
    let leap_days = (y / 4 - y / 100 + y / 400) - (1969 / 4 - 1969 / 100 + 1969 / 400);
    let mut days = (year - 1970) * 365 + leap_days;
    days += DAYS_BEFORE_MONTH[m - 1];
    let is_leap = (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0);
    if is_leap && m > 2 {
        days += 1;
    }
    days += day - 1;

    days * 86400
        + (dt.hour as i64) * 3600
        + (dt.minute as i64) * 60
        + (dt.second as i64)
}

/// Записывает дату и время в энергонезависимые регистры CMOS RTC (чип Motorola MC146818).
pub fn write_datetime(dt: &RtcDateTime) {
    interrupts::without_interrupts(|| {
        let mut spins = 0;
        while is_updating() && spins < 100_000 {
            spins += 1;
            core::hint::spin_loop();
        }

        let reg_b = read_cmos_register(0x0B);
        let is_bcd = (reg_b & 0x04) == 0;
        let is_24h = (reg_b & 0x02) != 0;

        // Взводим бит 7 (SET) регистра B: запрещает аппаратное обновление счетчиков
        // пока мы последовательно переписываем секунды, минуты, часы и дату.
        write_cmos_register(0x0B, reg_b | 0x80);

        let encode = |v: u8| -> u8 {
            if is_bcd {
                binary_to_bcd(v)
            } else {
                v
            }
        };

        write_cmos_register(0x00, encode(dt.second));
        write_cmos_register(0x02, encode(dt.minute));

        let encoded_hour = if is_24h {
            encode(dt.hour)
        } else {
            let is_pm = dt.hour >= 12;
            let h = match dt.hour % 12 {
                0 => 12,
                other => other,
            };
            let mut enc = encode(h);
            if is_pm {
                enc |= 0x80;
            }
            enc
        };
        write_cmos_register(0x04, encoded_hour);
        write_cmos_register(0x07, encode(dt.day));
        write_cmos_register(0x08, encode(dt.month));
        write_cmos_register(0x09, encode((dt.year % 100) as u8));
        let century = (dt.year / 100) as u8;
        if century > 0 {
            write_cmos_register(0x32, encode(century));
        }

        // Сбрасываем бит 7 (SET) регистра B: возобновляет нормальный ход аппаратных часов
        write_cmos_register(0x0B, reg_b & !0x80);
    });
}

/// Преобразует секунды эпохи Unix (UTC) в структуру календарной даты и времени RTC.
pub fn epoch_to_rtc(epoch_sec: i64) -> RtcDateTime {
    let mut days = epoch_sec / 86400;
    let mut rem_sec = epoch_sec % 86400;
    if rem_sec < 0 {
        rem_sec += 86400;
        days -= 1;
    }

    let hour = (rem_sec / 3600) as u8;
    let minute = ((rem_sec % 3600) / 60) as u8;
    let second = (rem_sec % 60) as u8;

    // Сдвиг от 1970-01-01 к алгоритму гражданского календаря (начало 400-летнего цикла с марта 0000)
    // 719468 дней от 0000-03-01 до 1970-01-01
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };

    RtcDateTime {
        year: year.clamp(1970, 9999) as u16,
        month: m as u8,
        day: d as u8,
        hour,
        minute,
        second,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_rtc_epoch_roundtrip() {
        let dt1 = RtcDateTime {
            year: 2026,
            month: 11,
            day: 2,
            hour: 15,
            minute: 40,
            second: 30,
        };
        let epoch = rtc_to_epoch(&dt1);
        let dt2 = epoch_to_rtc(epoch);
        assert_eq!(dt1, dt2);

        // Проверяем начало эпохи Unix
        let epoch_zero = epoch_to_rtc(0);
        assert_eq!(epoch_zero.year, 1970);
        assert_eq!(epoch_zero.month, 1);
        assert_eq!(epoch_zero.day, 1);
        assert_eq!(epoch_zero.hour, 0);
        assert_eq!(epoch_zero.minute, 0);
        assert_eq!(epoch_zero.second, 0);
    }
}
