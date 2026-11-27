use alloc::format;
use alloc::sync::Arc;
use alloc::vec;
use core::sync::atomic::{AtomicU8, Ordering};
use wunix_drv::abi::usbfs::*;

use super::types::{CharDeviceOp, USB_DEVICE_MAJOR};
use crate::drivers::usb::host::{find_usb_device, list_usb_devices, UsbAttachedDevice, UsbTransferError};
use crate::fs::devfs::DevDirInode;
use crate::fs::vfs::{Inode, VfsError};

/// Узел `/dev/bus/usb/BBB/DDD`, позволяющий user-space драйверам напрямую
/// управлять USB-устройством (Control, Bulk, Interrupt трансферы поверх ядра).
pub struct UsbfsDeviceNode {
    bus: u8,
    devnum: u8,
    claimed_mask: AtomicU8,
}

impl UsbfsDeviceNode {
    pub fn new(bus: u8, devnum: u8) -> Self {
        Self {
            bus,
            devnum,
            claimed_mask: AtomicU8::new(0),
        }
    }
}

fn usb_err_to_errno(err: UsbTransferError) -> i64 {
    match err {
        UsbTransferError::Timeout => -110,
        UsbTransferError::Stall => -32,
        UsbTransferError::InvalidEndpoint => -22,
        UsbTransferError::NoDevice => -19,
        UsbTransferError::Unsupported => -95,
        UsbTransferError::IoError => -5,
    }
}

impl CharDeviceOp for UsbfsDeviceNode {
    fn size(&self) -> usize {
        find_usb_device(self.bus, self.devnum)
            .map(|d| 18 + d.info.raw_config_desc.len())
            .unwrap_or(0)
    }

    fn read(&self, offset: usize, buf: &mut [u8]) -> Result<usize, VfsError> {
        let reg = find_usb_device(self.bus, self.devnum).ok_or(VfsError::NotFound)?;
        let desc = UsbDeviceDescriptor {
            b_length: 18,
            b_descriptor_type: 1,
            bcd_usb: 0x0300,
            b_device_class: reg.info.dev_class,
            b_device_subclass: reg.info.dev_subclass,
            b_device_protocol: reg.info.dev_protocol,
            b_max_packet_size0: reg.info.max_packet_size.min(255) as u8,
            id_vendor: reg.info.vendor_id,
            id_product: reg.info.product_id,
            bcd_device: 0x0100,
            i_manufacturer: 0,
            i_product: 0,
            i_serial_number: 0,
            b_num_configurations: 1,
        };

        let mut raw = vec![0u8; 18 + reg.info.raw_config_desc.len()];
        let desc_bytes = unsafe {
            core::slice::from_raw_parts(&desc as *const UsbDeviceDescriptor as *const u8, 18)
        };
        raw[..18].copy_from_slice(desc_bytes);
        raw[18..].copy_from_slice(&reg.info.raw_config_desc);

        if offset >= raw.len() {
            return Ok(0);
        }
        let n = buf.len().min(raw.len() - offset);
        buf[..n].copy_from_slice(&raw[offset..offset + n]);
        Ok(n)
    }

    fn write(&self, _offset: usize, _buf: &[u8]) -> Result<usize, VfsError> {
        Err(VfsError::NotSupported)
    }

    fn ioctl(&self, cmd: u32, arg: u64) -> Result<u64, i64> {
        let reg = find_usb_device(self.bus, self.devnum).ok_or(-19i64)?;
        let slot_id = reg.info.slot_id;

        match cmd {
            USBDEVFS_GET_DEVICE_INFO => {
                let sz = core::mem::size_of::<UsbDeviceInfo>() as u64;
                if !crate::task::syscall::validate_user_buffer(arg, sz) {
                    return Err(-14);
                }
                let info = UsbDeviceInfo {
                    bus: reg.info.bus,
                    devnum: reg.info.devnum,
                    slot_id: reg.info.slot_id,
                    speed: reg.info.speed,
                    vendor_id: reg.info.vendor_id,
                    product_id: reg.info.product_id,
                    device_class: reg.info.dev_class,
                    device_subclass: reg.info.dev_subclass,
                    device_protocol: reg.info.dev_protocol,
                    num_interfaces: reg.info.interfaces.len() as u8,
                    ep_bulk_in: reg.info.ep_bulk_in,
                    ep_bulk_out: reg.info.ep_bulk_out,
                    ep_intr_in: reg.info.ep_intr_in,
                    claimed_mask: self.claimed_mask.load(Ordering::Acquire),
                    max_packet_size: reg.info.max_packet_size,
                    reserved: [0; 14],
                };
                unsafe { core::ptr::write_unaligned(arg as *mut UsbDeviceInfo, info); }
                Ok(0)
            }
            USBDEVFS_GET_RAW_DESCRIPTOR => {
                let sz = core::mem::size_of::<UsbfsRawDescriptor>() as u64;
                if !crate::task::syscall::validate_user_buffer(arg, sz) {
                    return Err(-14);
                }
                let mut req = unsafe { core::ptr::read_unaligned(arg as *const UsbfsRawDescriptor) };
                let to_copy = (req.len as usize).min(reg.info.raw_config_desc.len());
                if to_copy > 0 {
                    if !crate::task::syscall::validate_user_buffer(req.buf, to_copy as u64) {
                        return Err(-14);
                    }
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            reg.info.raw_config_desc.as_ptr(),
                            req.buf as *mut u8,
                            to_copy,
                        );
                    }
                }
                req.actual_len = to_copy as u32;
                unsafe { core::ptr::write_unaligned(arg as *mut UsbfsRawDescriptor, req); }
                Ok(0)
            }
            USBDEVFS_CLAIMINTERFACE => {
                if !crate::task::syscall::validate_user_buffer(arg, 4) {
                    return Err(-14);
                }
                let iface = unsafe { core::ptr::read_unaligned(arg as *const u32) };
                if iface >= 8 {
                    return Err(-22);
                }
                self.claimed_mask.fetch_or(1 << iface, Ordering::AcqRel);
                Ok(0)
            }
            USBDEVFS_RELEASEINTERFACE => {
                if !crate::task::syscall::validate_user_buffer(arg, 4) {
                    return Err(-14);
                }
                let iface = unsafe { core::ptr::read_unaligned(arg as *const u32) };
                if iface >= 8 {
                    return Err(-22);
                }
                self.claimed_mask.fetch_and(!(1 << iface), Ordering::AcqRel);
                Ok(0)
            }
            USBDEVFS_CONNECTINFO => {
                let sz = core::mem::size_of::<UsbfsConnectInfo>() as u64;
                if !crate::task::syscall::validate_user_buffer(arg, sz) {
                    return Err(-14);
                }
                let ci = UsbfsConnectInfo {
                    devnum: reg.info.devnum as u32,
                    slow: if reg.info.speed == 2 { 1 } else { 0 },
                    reserved: [0; 3],
                };
                unsafe { core::ptr::write_unaligned(arg as *mut UsbfsConnectInfo, ci); }
                Ok(0)
            }
            USBDEVFS_CONTROL => {
                let sz = core::mem::size_of::<UsbfsCtrlTransfer>() as u64;
                if !crate::task::syscall::validate_user_buffer(arg, sz) {
                    return Err(-14);
                }
                let ctrl = unsafe { core::ptr::read_unaligned(arg as *const UsbfsCtrlTransfer) };
                let len = ctrl.w_length as usize;
                let mut kbuf = vec![0u8; len];
                if len > 0 {
                    if !crate::task::syscall::validate_user_buffer(ctrl.data, len as u64) {
                        return Err(-14);
                    }
                    if (ctrl.bm_request_type & USB_DIR_IN) == 0 {
                        unsafe {
                            core::ptr::copy_nonoverlapping(ctrl.data as *const u8, kbuf.as_mut_ptr(), len);
                        }
                    }
                }
                let actual = reg
                    .host
                    .lock()
                    .control_transfer(
                        slot_id,
                        ctrl.bm_request_type,
                        ctrl.b_request,
                        ctrl.w_value,
                        ctrl.w_index,
                        &mut kbuf,
                    )
                    .map_err(usb_err_to_errno)?;
                if len > 0 && (ctrl.bm_request_type & USB_DIR_IN) != 0 {
                    let copy_n = actual.min(len);
                    unsafe {
                        core::ptr::copy_nonoverlapping(kbuf.as_ptr(), ctrl.data as *mut u8, copy_n);
                    }
                }
                Ok(actual as u64)
            }
            USBDEVFS_BULK => {
                let sz = core::mem::size_of::<UsbfsBulkTransfer>() as u64;
                if !crate::task::syscall::validate_user_buffer(arg, sz) {
                    return Err(-14);
                }
                let bulk = unsafe { core::ptr::read_unaligned(arg as *const UsbfsBulkTransfer) };
                let len = (bulk.len as usize).min(65536);
                let mut kbuf = vec![0u8; len];
                let is_in = (bulk.ep & 0x80) != 0;
                if len > 0 {
                    if !crate::task::syscall::validate_user_buffer(bulk.data, len as u64) {
                        return Err(-14);
                    }
                    if !is_in {
                        unsafe {
                            core::ptr::copy_nonoverlapping(bulk.data as *const u8, kbuf.as_mut_ptr(), len);
                        }
                    }
                }
                let actual = reg
                    .host
                    .lock()
                    .bulk_transfer(slot_id, bulk.ep as u8, &mut kbuf, bulk.timeout_ms)
                    .map_err(usb_err_to_errno)?;
                if len > 0 && is_in {
                    let copy_n = actual.min(len);
                    unsafe {
                        core::ptr::copy_nonoverlapping(kbuf.as_ptr(), bulk.data as *mut u8, copy_n);
                    }
                }
                Ok(actual as u64)
            }
            USBDEVFS_INTERRUPT => {
                let sz = core::mem::size_of::<UsbfsIntTransfer>() as u64;
                if !crate::task::syscall::validate_user_buffer(arg, sz) {
                    return Err(-14);
                }
                let intr = unsafe { core::ptr::read_unaligned(arg as *const UsbfsIntTransfer) };
                let len = (intr.len as usize).min(4096);
                let mut kbuf = vec![0u8; len];
                let is_in = (intr.ep & 0x80) != 0;
                if len > 0 && !crate::task::syscall::validate_user_buffer(intr.data, len as u64) {
                    return Err(-14);
                }
                if len > 0 && !is_in {
                    unsafe {
                        core::ptr::copy_nonoverlapping(intr.data as *const u8, kbuf.as_mut_ptr(), len);
                    }
                }
                let actual = reg
                    .host
                    .lock()
                    .interrupt_transfer(slot_id, intr.ep as u8, &mut kbuf, intr.timeout_ms)
                    .map_err(usb_err_to_errno)?;
                if len > 0 && is_in {
                    let copy_n = actual.min(len);
                    unsafe {
                        core::ptr::copy_nonoverlapping(kbuf.as_ptr(), intr.data as *mut u8, copy_n);
                    }
                }
                Ok(actual as u64)
            }
            USBDEVFS_RESET => {
                reg.host
                    .lock()
                    .reset_device(slot_id)
                    .map_err(usb_err_to_errno)?;
                Ok(0)
            }
            _ => Err(-25),
        }
    }
}

pub fn expose_usb_device_node(info: &UsbAttachedDevice) {
    let Some(devfs) = super::get_devfs() else {
        return;
    };
    let bus_name = format!("{:03}", info.bus);
    let dev_name = format!("{:03}", info.devnum);

    let bus_dir = if let Ok(node) = devfs.usb_bus_dir.lookup(&bus_name) {
        node
    } else {
        let new_dir = Arc::new(DevDirInode::new());
        devfs.usb_bus_dir.insert(&bus_name, new_dir.clone());
        new_dir
    };

    let minor = ((info.bus as u32) << 7) | (info.devnum as u32 & 0x7F);
    let node = Arc::new(super::DevInode::new_char(
        super::makedev(USB_DEVICE_MAJOR, minor),
        0o660,
        Arc::new(UsbfsDeviceNode::new(info.bus, info.devnum)),
    ));
    let _ = bus_dir.insert_child(&dev_name, node);
}

pub fn sync_all_usb_nodes() {
    for reg in list_usb_devices() {
        expose_usb_device_node(&reg.info);
    }
}
