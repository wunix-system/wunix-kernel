/// 8-байтный дескриптор буфера AC'97 DMA (Buffer Descriptor)
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Ac97BufferDesc {
    pub addr: u32,
    pub length: u16,       // Количество сэмплов (1 сэмпл = 2 байта для 16-бит)
    pub flags: u16,        // Бит 15: IOC (Interrupt on Completion), бит 14: BUP (Buffer Underrun Policy)
}

const _: () = assert!(core::mem::size_of::<Ac97BufferDesc>() == 8);

pub const BD_IOC: u16 = 1 << 15;
pub const BD_BUP: u16 = 1 << 14;
