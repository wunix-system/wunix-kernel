use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsbTransferError {
    Stall,
    Timeout,
    IoError,
    InvalidEndpoint,
    NoDevice,
    Unsupported,
}

#[derive(Debug, Clone, Copy)]
pub struct UsbEndpointMeta {
    pub addr: u8,
    pub attr: u8,
    pub max_packet_size: u16,
    pub interval: u8,
}

#[derive(Debug, Clone)]
pub struct UsbInterfaceMeta {
    pub iface_num: u8,
    pub alt_setting: u8,
    pub class: u8,
    pub subclass: u8,
    pub protocol: u8,
    pub endpoints: Vec<UsbEndpointMeta>,
}

#[derive(Debug, Clone)]
pub struct UsbAttachedDevice {
    pub bus: u8,
    pub devnum: u8,
    pub slot_id: u8,
    pub speed: u8,
    pub vendor_id: u16,
    pub product_id: u16,
    pub dev_class: u8,
    pub dev_subclass: u8,
    pub dev_protocol: u8,
    pub max_packet_size: u16,
    pub raw_config_desc: Vec<u8>,
    pub interfaces: Vec<UsbInterfaceMeta>,
    pub ep_bulk_in: u8,
    pub ep_bulk_out: u8,
    pub ep_intr_in: u8,
}

/// Единый контракт хост-контроллера USB (xHCI, EHCI, OHCI, UHCI).
/// Используется как встроенными драйверами ядра, так и user-space драйверами через `/dev/bus/usb/*`.
pub trait UsbHostController: Send + Sync {
    fn bus_id(&self) -> u8;
    fn controller_name(&self) -> &'static str;

    fn control_transfer(
        &mut self,
        slot_id: u8,
        bm_request_type: u8,
        b_request: u8,
        w_value: u16,
        w_index: u16,
        data: &mut [u8],
    ) -> Result<usize, UsbTransferError>;

    fn bulk_transfer(
        &mut self,
        slot_id: u8,
        ep_addr: u8,
        data: &mut [u8],
        timeout_ms: u32,
    ) -> Result<usize, UsbTransferError>;

    fn interrupt_transfer(
        &mut self,
        slot_id: u8,
        ep_addr: u8,
        data: &mut [u8],
        timeout_ms: u32,
    ) -> Result<usize, UsbTransferError>;

    fn reset_device(&mut self, slot_id: u8) -> Result<(), UsbTransferError>;
}

#[derive(Clone)]
pub struct RegisteredUsbDevice {
    pub host: Arc<Mutex<dyn UsbHostController>>,
    pub info: UsbAttachedDevice,
}

static NEXT_USB_BUS_ID: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(1);
static USB_DEVICES: Mutex<Vec<RegisteredUsbDevice>> = Mutex::new(Vec::new());

pub fn alloc_usb_bus_id() -> u8 {
    NEXT_USB_BUS_ID.fetch_add(1, core::sync::atomic::Ordering::Relaxed)
}

pub fn register_usb_device(
    host: Arc<Mutex<dyn UsbHostController>>,
    info: UsbAttachedDevice,
) {
    USB_DEVICES.lock().push(RegisteredUsbDevice {
        host,
        info: info.clone(),
    });
    crate::fs::devfs::usbfs::expose_usb_device_node(&info);
}

pub fn list_usb_devices() -> Vec<RegisteredUsbDevice> {
    USB_DEVICES.lock().clone()
}

pub fn find_usb_device(bus: u8, devnum: u8) -> Option<RegisteredUsbDevice> {
    USB_DEVICES
        .lock()
        .iter()
        .find(|d| d.info.bus == bus && d.info.devnum == devnum)
        .cloned()
}

/// Разбирает raw USB Configuration Descriptor на интерфейсы и конечные точки.
pub fn parse_config_interfaces(raw: &[u8]) -> Vec<UsbInterfaceMeta> {
    let mut interfaces = Vec::new();
    let mut offset = 0usize;

    while offset + 2 <= raw.len() {
        let len = raw[offset] as usize;
        let dtype = raw[offset + 1];
        if len < 2 || offset + len > raw.len() {
            break;
        }
        if dtype == 4 && len >= 9 {
            interfaces.push(UsbInterfaceMeta {
                iface_num: raw[offset + 2],
                alt_setting: raw[offset + 3],
                class: raw[offset + 5],
                subclass: raw[offset + 6],
                protocol: raw[offset + 7],
                endpoints: Vec::new(),
            });
        } else if dtype == 5 && len >= 7 {
            if let Some(last_iface) = interfaces.last_mut() {
                let mps = u16::from_le_bytes([raw[offset + 4], raw[offset + 5]]) & 0x07FF;
                last_iface.endpoints.push(UsbEndpointMeta {
                    addr: raw[offset + 2],
                    attr: raw[offset + 3],
                    max_packet_size: mps,
                    interval: raw[offset + 6],
                });
            }
        }
        offset += len;
    }

    interfaces
}

/// Выбирает первый найденный адрес Bulk IN / Bulk OUT / Interrupt IN для группы конечных точек.
pub fn summarize_endpoints(eps: &[UsbEndpointMeta]) -> (u8, u8, u8) {
    let mut bulk_in = 0u8;
    let mut bulk_out = 0u8;
    let mut intr_in = 0u8;
    for ep in eps {
        let is_in = (ep.addr & 0x80) != 0;
        match (ep.attr & 0x03, is_in) {
            (2, true) if bulk_in == 0 => bulk_in = ep.addr,
            (2, false) if bulk_out == 0 => bulk_out = ep.addr,
            (3, true) if intr_in == 0 => intr_in = ep.addr,
            _ => {}
        }
    }
    (bulk_in, bulk_out, intr_in)
}

/// Читает Device Descriptor и Configuration Descriptor у адресуемого USB устройства
/// и активирует его первую конфигурацию (для UHCI/OHCI/EHCI).
pub fn query_device_descriptors<C: UsbHostController>(
    ctrl: &mut C,
    bus_id: u8,
    addr: u8,
    devnum: u8,
    speed: u8,
    default_mps: u16,
) -> Option<UsbAttachedDevice> {
    let mut dev_desc = [0u8; 18];
    ctrl.control_transfer(addr, 0x80, 6, 0x0100, 0, &mut dev_desc)
        .ok()?;
    let mps0 = if dev_desc[7] == 0 {
        default_mps
    } else {
        dev_desc[7] as u16
    };

    let mut cfg_hdr = [0u8; 9];
    let mut raw_cfg = Vec::new();
    let mut interfaces = Vec::new();

    if ctrl
        .control_transfer(addr, 0x80, 6, 0x0200, 0, &mut cfg_hdr)
        .is_ok()
    {
        let total_len = u16::from_le_bytes([cfg_hdr[2], cfg_hdr[3]]) as usize;
        let fetch_len = total_len.clamp(9, 512);
        raw_cfg.resize(fetch_len, 0);
        if ctrl
            .control_transfer(addr, 0x80, 6, 0x0200, 0, &mut raw_cfg)
            .is_ok()
        {
            let cfg_val = cfg_hdr[5].max(1);
            let _ = ctrl.control_transfer(addr, 0x00, 9, cfg_val as u16, 0, &mut []);
            interfaces = parse_config_interfaces(&raw_cfg);
        }
    }

    let all_eps: Vec<UsbEndpointMeta> = interfaces
        .iter()
        .flat_map(|i| i.endpoints.iter().copied())
        .collect();
    let (ep_bulk_in, ep_bulk_out, ep_intr_in) = summarize_endpoints(&all_eps);

    Some(UsbAttachedDevice {
        bus: bus_id,
        devnum,
        slot_id: addr,
        speed,
        vendor_id: u16::from_le_bytes([dev_desc[8], dev_desc[9]]),
        product_id: u16::from_le_bytes([dev_desc[10], dev_desc[11]]),
        dev_class: dev_desc[4],
        dev_subclass: dev_desc[5],
        dev_protocol: dev_desc[6],
        max_packet_size: mps0,
        raw_config_desc: raw_cfg,
        interfaces,
        ep_bulk_in,
        ep_bulk_out,
        ep_intr_in,
    })
}

/// Регистрирует USB-устройство в usbfs (`/dev/bus/usb/BBB/DDD`) и подключает встроенные драйверы классов.
pub fn attach_enumerated_device(
    host_dyn: &Arc<Mutex<dyn UsbHostController>>,
    info: UsbAttachedDevice,
    dm: &mut crate::drivers::manager::DeviceManager,
    block_count: &mut usize,
) {
    use crate::drivers::traits::BlockDevice;
    use crate::drivers::usb::classes;
    use crate::i18n::t;

    register_usb_device(host_dyn.clone(), info.clone());

    for iface in &info.interfaces {
        let (iface_in, iface_out, iface_intr) = summarize_endpoints(&iface.endpoints);
        let bulk_in = if iface_in != 0 { iface_in } else { info.ep_bulk_in };
        let bulk_out = if iface_out != 0 { iface_out } else { info.ep_bulk_out };
        let intr_in = if iface_intr != 0 { iface_intr } else { info.ep_intr_in };

        match iface.class {
            0x08 if bulk_in != 0 && bulk_out != 0 => {
                if let Ok(mut storage) = classes::storage::UsbStorageDevice::with_host(
                    host_dyn.clone(),
                    info.slot_id,
                    bulk_in,
                    bulk_out,
                ) {
                    if storage.init_storage().is_ok() {
                        let letter = (b'a' + (*block_count as u8).min(25)) as char;
                        let dev_name = alloc::format!("sd{}", letter);
                        let cap_mb = (storage.sector_count() * storage.sector_size() as u64)
                            / (1024 * 1024);
                        crate::klog!(
                            "{}: /dev/{} (USB Mass Storage, {} МБ / MB)",
                            host_dyn.lock().controller_name(),
                            dev_name,
                            cap_mb
                        );
                        dm.register_block_device(dev_name, Arc::new(Mutex::new(storage)));
                        *block_count += 1;
                    }
                }
            }
            0x03 if intr_in != 0 => {
                if iface.protocol == 0x01 {
                    let kbd = classes::hid::UsbKeyboardDevice::with_host(
                        host_dyn.clone(),
                        info.slot_id,
                        intr_in,
                    );
                    crate::klog!(
                        "{}: {}",
                        host_dyn.lock().controller_name(),
                        t("зарегистрирована USB клавиатура", "registered USB HID keyboard")
                    );
                    dm.register_input_device(Arc::new(Mutex::new(kbd)));
                } else if iface.protocol == 0x02 {
                    let mouse = classes::hid::UsbMouseDevice::with_host(
                        host_dyn.clone(),
                        info.slot_id,
                        intr_in,
                    );
                    crate::klog!(
                        "{}: {}",
                        host_dyn.lock().controller_name(),
                        t("зарегистрирована USB мышь", "registered USB HID mouse")
                    );
                    dm.register_input_device(Arc::new(Mutex::new(mouse)));
                }
            }
            0x02 | 0x0A => {
                let mac = [0x02, 0x1A, 0x2B, 0x3C, info.slot_id, info.devnum];
                classes::cdc_net::probe_cdc_device_with_host(
                    host_dyn.clone(),
                    info.slot_id,
                    bulk_in,
                    bulk_out,
                    mac,
                    dm,
                );
            }
            0x01 => {
                classes::audio::probe_audio_device_with_host(
                    host_dyn.clone(),
                    info.slot_id,
                    bulk_out,
                    dm,
                );
            }
            0x09 => {
                let _hub = classes::hub::UsbHub::probe_with_host(host_dyn, info.slot_id);
            }
            _ => {}
        }
    }
}
