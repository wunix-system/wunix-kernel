use crate::drivers::virtio::VirtioError;

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

impl From<VirtioError> for NetDeviceError {
    fn from(err: VirtioError) -> Self {
        match err {
            VirtioError::IoError => NetDeviceError::IoError,
            VirtioError::QueueFull | VirtioError::QueueTooSmall => NetDeviceError::NotReady,
            VirtioError::Unsupported => NetDeviceError::Unsupported,
            _ => NetDeviceError::IoError,
        }
    }
}

/// Унифицированный интерфейс блочного накопителя (/dev/vda, ATA, NVMe)
pub trait BlockDevice: Send + Sync {
    /// Считывает последовательность 512-байтных секторов, начиная с `start_sector`
    fn read_sectors(&mut self, start_sector: u64, buf: &mut [u8]) -> Result<(), VirtioError>;

    /// Записывает последовательность 512-байтных секторов, начиная с `start_sector`
    fn write_sectors(&mut self, start_sector: u64, buf: &[u8]) -> Result<(), VirtioError>;

    /// Размер одного сектора в байтах (обычно 512)
    fn sector_size(&self) -> usize;

    /// Общее количество секторов на накопителе
    fn sector_count(&self) -> u64;
}

/// Унифицированный интерфейс сетевого адаптера (Ethernet, Wi-Fi, Loopback).
///
/// Сетевой стек ядра взаимодействует только с этим типажом, не привязываясь
/// к конкретной реализации оборудования (VirtIO, VMXNET3, NetVSC, e1000).
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

/// Унифицированный интерфейс устройства ввода (планшет, мышь, тачпад)
pub trait InputDevice: Send + Sync {
    /// Название устройства
    fn name(&self) -> &str;

    /// Извлекает следующее событие ввода из очереди (если доступно)
    fn poll_event(&mut self) -> Option<crate::drivers::virtio::input::VirtioInputEvent>;

    /// Текущие абсолютные координаты курсора (X, Y)
    fn current_coordinates(&self) -> (u32, u32);

    /// Текущее битовое состояние кнопок (бит 0: Left, бит 1: Right, бит 2: Middle)
    fn button_mask(&self) -> u32;
}

/// Унифицированный интерфейс графического адаптера (GPU, Framebuffer)
pub trait DisplayDevice: Send + Sync {
    /// Разрешение экрана (ширина, высота)
    fn resolution(&self) -> (u32, u32);

    /// Отрисовывает пиксель заданного цвета в кадровом буфере
    fn draw_pixel(&mut self, x: u32, y: u32, color: u32);

    /// Сбрасывает измененный кадровый буфер на экран
    fn flush(&mut self) -> Result<(), VirtioError>;
}
