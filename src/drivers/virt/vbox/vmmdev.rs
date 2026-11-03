use spin::Mutex;
use x86_64::instructions::port::Port;
use x86_64::structures::paging::PhysFrame;

use crate::drivers::pci::PciDevice;
use crate::memory::{allocate_frame, deallocate_frame, hhdm_offset};

pub const VBOX_PCI_VENDOR_ID: u16 = 0x80EE;
pub const VBOX_VMMDEV_DEVICE_ID: u16 = 0xCAFE;
pub const VBOX_VIDEO_DEVICE_ID: u16 = 0xBEEF;

pub const VMMDEV_REQUEST_HEADER_VERSION: u32 = 0x10001;

pub const VMMDEV_REQ_GET_MOUSE_STATUS: u32 = 1;
pub const VMMDEV_REQ_SET_MOUSE_STATUS: u32 = 2;
pub const VMMDEV_REQ_GET_HOST_TIME: u32 = 10;
pub const VMMDEV_REQ_ACK_EVENTS: u32 = 41;
pub const VMMDEV_REQ_CTL_FILTER_MASK: u32 = 42;
pub const VMMDEV_REQ_REPORT_GUEST_INFO: u32 = 50;
pub const VMMDEV_REQ_HGCM_CONNECT: u32 = 60;
pub const VMMDEV_REQ_HGCM_DISCONNECT: u32 = 61;
pub const VMMDEV_REQ_HGCM_CALL: u32 = 62;

pub const VBOX_MOUSE_FEATURES_ABS_PTR: u32 = 1 << 0;

/// Заголовок любого запроса к шлюзу VirtualBox VMMDev.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VmmDevRequestHeader {
    pub size: u32,
    pub version: u32,
    pub request_type: u32,
    pub rc: i32,
    pub reserved1: u32,
    pub reserved2: u32,
}

impl VmmDevRequestHeader {
    pub const fn new(size: u32, request_type: u32) -> Self {
        Self {
            size,
            version: VMMDEV_REQUEST_HEADER_VERSION,
            request_type,
            rc: 0,
            reserved1: 0,
            reserved2: 0,
        }
    }
}

/// Запрос статуса абсолютного указателя мыши.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VmmDevMouseStatus {
    pub header: VmmDevRequestHeader,
    pub mouse_features: u32,
    pub pointer_x: i32,
    pub pointer_y: i32,
}

/// Запрос текущего времени хоста (UTC в миллисекундах).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VmmDevHostTime {
    pub header: VmmDevRequestHeader,
    pub host_time_ms: u64,
}

/// Драйвер интеграционной шины Oracle VM VirtualBox (VMMDev).
pub struct VBoxGuestDevice {
    io_port: u16,
    dma_frame: PhysFrame,
    dma_virt: *mut u8,
}

unsafe impl Send for VBoxGuestDevice {}
unsafe impl Sync for VBoxGuestDevice {}

impl VBoxGuestDevice {
    /// Инициализирует VMMDev из обнаруженного PCI устройства.
    pub fn new(pci: &PciDevice) -> Result<Self, &'static str> {
        let io_bar = pci.first_io_bar().ok_or("VBox VMMDev: I/O BAR0 not found")?;
        pci.enable_io_space();
        pci.enable_bus_mastering();

        let dma_frame = allocate_frame().ok_or("VBox VMMDev: Failed to allocate DMA frame")?;
        let dma_virt = (hhdm_offset() + dma_frame.start_address().as_u64()) as *mut u8;

        unsafe {
            core::ptr::write_bytes(dma_virt, 0, 4096);
        }

        let dev = Self {
            io_port: io_bar.address as u16,
            dma_frame,
            dma_virt,
        };

        Ok(dev)
    }

    #[inline]
    pub fn dma_phys(&self) -> u64 {
        self.dma_frame.start_address().as_u64()
    }

    #[inline]
    pub fn dma_ptr<T>(&self) -> *mut T {
        self.dma_virt as *mut T
    }

    pub fn execute_request(&mut self) -> Result<i32, &'static str> {
        let phys_addr = self.dma_phys() as u32;

        unsafe {
            let mut port = Port::<u32>::new(self.io_port);
            port.write(phys_addr);

            let header = &*(self.dma_virt as *const VmmDevRequestHeader);
            Ok(header.rc)
        }
    }

    pub fn get_host_time(&mut self) -> Result<u64, &'static str> {
        unsafe {
            let req = self.dma_ptr::<VmmDevHostTime>();
            *req = VmmDevHostTime {
                header: VmmDevRequestHeader::new(
                    core::mem::size_of::<VmmDevHostTime>() as u32,
                    VMMDEV_REQ_GET_HOST_TIME,
                ),
                host_time_ms: 0,
            };

            let rc = self.execute_request()?;
            if rc >= 0 {
                Ok((*req).host_time_ms)
            } else {
                Err("VBox VMMDev: host time query failed")
            }
        }
    }

    pub fn set_mouse_status(&mut self, enable_abs: bool) -> Result<(), &'static str> {
        let features = if enable_abs {
            VBOX_MOUSE_FEATURES_ABS_PTR
        } else {
            0
        };

        unsafe {
            let req = self.dma_ptr::<VmmDevMouseStatus>();
            *req = VmmDevMouseStatus {
                header: VmmDevRequestHeader::new(
                    core::mem::size_of::<VmmDevMouseStatus>() as u32,
                    VMMDEV_REQ_SET_MOUSE_STATUS,
                ),
                mouse_features: features,
                pointer_x: 0,
                pointer_y: 0,
            };

            let rc = self.execute_request()?;
            if rc >= 0 {
                Ok(())
            } else {
                Err("VBox VMMDev: set mouse status failed")
            }
        }
    }

    pub fn get_mouse_status(&mut self) -> Result<(i32, i32, u32), &'static str> {
        unsafe {
            let req = self.dma_ptr::<VmmDevMouseStatus>();
            *req = VmmDevMouseStatus {
                header: VmmDevRequestHeader::new(
                    core::mem::size_of::<VmmDevMouseStatus>() as u32,
                    VMMDEV_REQ_GET_MOUSE_STATUS,
                ),
                mouse_features: 0,
                pointer_x: 0,
                pointer_y: 0,
            };

            let rc = self.execute_request()?;
            if rc >= 0 {
                let r = &*req;
                Ok((r.pointer_x, r.pointer_y, r.mouse_features))
            } else {
                Err("VBox VMMDev: get mouse status failed")
            }
        }
    }
}

impl Drop for VBoxGuestDevice {
    fn drop(&mut self) {
        deallocate_frame(self.dma_frame);
    }
}

pub static VBOX_GUEST: Mutex<Option<VBoxGuestDevice>> = Mutex::new(None);
