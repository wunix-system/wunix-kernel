/// 16-байтная запись списка дескрипторов буферов HDA (Buffer Descriptor List Entry)
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct BdlEntry {
    pub addr: u64,
    pub length: u32,
    pub ioc: u32,
}

const _: () = assert!(core::mem::size_of::<BdlEntry>() == 16);
