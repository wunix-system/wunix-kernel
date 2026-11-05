/// Ошибки канального уровня сетевого адаптера
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetDeviceError {
    /// Аппаратный сбой или ошибка ввода-вывода контроллера
    IoError,
    /// Размер передаваемого кадра превышает поддерживаемый MTU
    PacketTooLarge,
    /// Буфер приёма недостаточен для входящего кадра
    BufferTooSmall,
    /// Очередь передачи переполнена или адаптер временно занят
    NotReady,
    /// Неподдерживаемая операция или некорректные параметры
    Unsupported,
}

/// Ошибки операций блочного накопителя
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockDeviceError {
    /// Ошибка ввода-вывода или аппаратный таймаут контроллера
    IoError,
    /// Обращение за пределы доступного количества секторов накопителя
    OutOfBounds,
    /// Контроллер или диск занят / не готов
    NotReady,
    /// Операция не поддерживается данным типом устройства
    Unsupported,
    /// Устройство защищено от записи
    WriteProtected,
}

/// Ошибки графического адаптера
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayDeviceError {
    /// Ошибка передачи данных в кадровый буфер или аппаратный сбой
    IoError,
    /// Операция не поддерживается адаптером
    Unsupported,
    /// Графический адаптер занят
    NotReady,
}

/// Ошибки воспроизведения звука
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioDeviceError {
    /// Ошибка передачи данных через DMA или таймаут контроллера
    IoError,
    /// Операция не поддерживается данным типом звукового чипа
    Unsupported,
    /// Звуковой буфер или кольцо воспроизведения переполнено
    NotReady,
}

/// Унифицированное событие устройства ввода (мышь, клавиатура, сенсорный экран)
#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct InputEvent {
    pub type_: u16,
    pub code: u16,
    pub value: u32,
}

/// Унифицированный интерфейс блочного накопителя (/dev/vda, ATA, SATA/AHCI, NVMe).
pub trait BlockDevice: Send + Sync {
    /// Считывает последовательность 512-байтных секторов, начиная с `start_sector`
    fn read_sectors(&mut self, start_sector: u64, buf: &mut [u8]) -> Result<(), BlockDeviceError>;

    /// Записывает последовательность 512-байтных секторов, начиная с `start_sector`
    fn write_sectors(&mut self, start_sector: u64, buf: &[u8]) -> Result<(), BlockDeviceError>;

    /// Размер одного сектора в байтах (обычно 512)
    fn sector_size(&self) -> usize;

    /// Общее количество секторов на накопителе
    fn sector_count(&self) -> u64;
}

/// Унифицированный интерфейс сетевого адаптера (Ethernet, Wi-Fi, Loopback).
pub trait NetDevice: Send + Sync {
    /// Название драйвера устройства (например, "virtio-net", "vmware-vmxnet3")
    fn driver_name(&self) -> &'static str {
        "generic-eth"
    }

    /// Аппаратный MAC-адрес устройства
    fn mac_address(&self) -> [u8; 6];

    /// Отправляет сетевой пакет (Ethernet-фрейм)
    fn send_packet(&mut self, packet: &[u8]) -> Result<(), NetDeviceError>;

    /// Принимает входящий сетевой пакет из очереди RX
    fn receive_packet(&mut self, buf: &mut [u8]) -> Result<Option<usize>, NetDeviceError>;
}

/// Унифицированный интерфейс устройства ввода (планшет, мышь, тачпад, клавиатура).
pub trait InputDevice: Send + Sync {
    /// Название устройства
    fn name(&self) -> &str;

    /// Извлекает следующее событие ввода из очереди (если доступно)
    fn poll_event(&mut self) -> Option<InputEvent>;

    /// Текущие абсолютные координаты курсора (X, Y)
    fn current_coordinates(&self) -> (u32, u32);

    /// Текущее битовое состояние кнопок (бит 0: Left, бит 1: Right, бит 2: Middle)
    fn button_mask(&self) -> u32;
}

/// Унифицированный интерфейс графического адаптера (GPU, Framebuffer).
pub trait DisplayDevice: Send + Sync {
    /// Разрешение экрана (ширина, высота)
    fn resolution(&self) -> (u32, u32);

    /// Отрисовывает пиксель заданного цвета в кадровом буфере
    fn draw_pixel(&mut self, x: u32, y: u32, color: u32);

    /// Сбрасывает измененный кадровый буфер на экран
    fn flush(&mut self) -> Result<(), DisplayDeviceError>;
}

/// Унифицированный интерфейс звукового контроллера (Intel HDA, AC'97, SB16).
pub trait AudioDevice: Send + Sync {
    /// Название звукового устройства
    fn device_name(&self) -> &'static str;

    /// Воспроизводит порцию PCM аудиоданных (16-бит, 44.1/48 кГц stereo)
    fn play_pcm(&mut self, samples: &[u8]) -> Result<(), AudioDeviceError>;

    /// Устанавливает громкость воспроизведения (0..100%)
    fn set_volume(&mut self, volume_percent: u8) -> Result<(), AudioDeviceError>;

    /// Проверяет, занят ли аудиовыход воспроизведением
    fn is_busy(&self) -> bool;
}

