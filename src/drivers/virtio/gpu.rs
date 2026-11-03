use alloc::boxed::Box;
use core::sync::atomic::{fence, Ordering};

use super::queue::{VirtioBuffer, Virtqueue};
use super::transport::VirtioTransport;
use super::{
    VirtioDevice, VirtioDeviceType, VirtioError, VIRTIO_STATUS_ACKNOWLEDGE, VIRTIO_STATUS_DRIVER,
    VIRTIO_STATUS_DRIVER_OK, VIRTIO_STATUS_FEATURES_OK,
};
use crate::drivers::DisplayDevice;

// Команды протокола VirtIO-GPU
pub const VIRTIO_GPU_CMD_GET_DISPLAY_INFO: u32 = 0x0100;
pub const VIRTIO_GPU_CMD_RESOURCE_CREATE_2D: u32 = 0x0101;
pub const VIRTIO_GPU_CMD_RESOURCE_UNREF: u32 = 0x0102;
pub const VIRTIO_GPU_CMD_SET_SCANOUT: u32 = 0x0103;
pub const VIRTIO_GPU_CMD_RESOURCE_FLUSH: u32 = 0x0104;
pub const VIRTIO_GPU_CMD_TRANSFER_TO_HOST_2D: u32 = 0x0105;
pub const VIRTIO_GPU_CMD_RESOURCE_ATTACH_BACKING: u32 = 0x0106;
pub const VIRTIO_GPU_CMD_RESOURCE_DETACH_BACKING: u32 = 0x0107;

// Ответы видеоадаптера VirtIO-GPU
pub const VIRTIO_GPU_RESP_OK_NODATA: u32 = 0x1100;
pub const VIRTIO_GPU_RESP_OK_DISPLAY_INFO: u32 = 0x1101;

// 32-битный RGBA/BGRA формат пикселей
pub const VIRTIO_GPU_FORMAT_B8G8R8A8_UNORM: u32 = 1;
pub const VIRTIO_GPU_FORMAT_B8G8R8X8_UNORM: u32 = 2;

/// Заголовок управляющей команды/ответа VirtIO-GPU (24 байта)
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct VirtioGpuCtrlHdr {
    pub type_: u32,
    pub flags: u32,
    pub fence_id: u64,
    pub ctx_id: u32,
    pub padding: u32,
}

/// Прямоугольная область экрана
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct VirtioGpuRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct ResourceCreate2dReq {
    hdr: VirtioGpuCtrlHdr,
    resource_id: u32,
    format: u32,
    width: u32,
    height: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct MemEntry {
    addr: u64,
    length: u32,
    padding: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct ResourceAttachBackingReq {
    hdr: VirtioGpuCtrlHdr,
    resource_id: u32,
    nr_entries: u32,
    entry: MemEntry,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct SetScanoutReq {
    hdr: VirtioGpuCtrlHdr,
    r: VirtioGpuRect,
    scanout_id: u32,
    resource_id: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct TransferToHost2dReq {
    hdr: VirtioGpuCtrlHdr,
    r: VirtioGpuRect,
    offset: u64,
    resource_id: u32,
    padding: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct ResourceFlushReq {
    hdr: VirtioGpuCtrlHdr,
    r: VirtioGpuRect,
    resource_id: u32,
    padding: u32,
}

/// Драйвер графического видеоадаптера VirtIO-GPU с аппаратным 2D-ускорением
pub struct VirtioGpu {
    transport: Box<dyn VirtioTransport>,
    ctrl_queue: Virtqueue,
    _cursor_queue: Virtqueue,
    width: u32,
    height: u32,
    resource_id: u32,
    // Выделенный фреймбуфер в физической памяти гостя
    fb_phys: u64,
    fb_virt: *mut u32,
    _fb_pages: usize,
    // Выделенный буфер для отправки команд и получения ответов
    cmd_phys: u64,
    cmd_virt: *mut u8,
}

unsafe impl Send for VirtioGpu {}
unsafe impl Sync for VirtioGpu {}

impl VirtioGpu {
    /// Инициализирует видеоадаптер VirtIO-GPU, создает 2D-ресурс и настраивает scanout
    pub fn new(
        mut transport: Box<dyn VirtioTransport>,
        width: u32,
        height: u32,
    ) -> Result<Self, VirtioError> {
        transport.reset();

        let mut status = VIRTIO_STATUS_ACKNOWLEDGE;
        transport.set_status(status);

        status |= VIRTIO_STATUS_DRIVER;
        transport.set_status(status);

        let _features = transport.read_device_features();
        transport.write_driver_features(0);

        status |= VIRTIO_STATUS_FEATURES_OK;
        transport.set_status(status);

        // Инициализируем очередь управления (Queue 0 - controlq)
        transport.select_queue(0);
        let ctrl_max = transport.get_queue_size();
        let ctrl_size = ctrl_max;
        let ctrl_queue = Virtqueue::new(0, ctrl_size)?;
        transport.setup_queue(&ctrl_queue);

        // Инициализируем очередь курсора (Queue 1 - cursorq)
        transport.select_queue(1);
        let cur_max = transport.get_queue_size();
        let cur_size = cur_max;
        let cursor_queue = Virtqueue::new(1, cur_size)?;
        transport.setup_queue(&cursor_queue);

        status |= VIRTIO_STATUS_DRIVER_OK;
        transport.set_status(status);

        // 1 страница под управляющие запросы DMA (команды и ответы)
        let cmd_frame = crate::memory::allocate_contiguous_frames(1)
            .ok_or(VirtioError::AllocationFailed)?;
        let cmd_phys = cmd_frame.start_address().as_u64();
        let hhdm = crate::memory::hhdm_offset();
        let cmd_virt = (hhdm + cmd_phys) as *mut u8;

        // Выделяем непрерывную память под графический фреймбуфер (RGBA: 4 байта на пиксель)
        let fb_bytes = (width as usize) * (height as usize) * 4;
        let fb_pages = (fb_bytes + 4095) / 4096;
        let fb_frame = crate::memory::allocate_contiguous_frames(fb_pages)
            .ok_or(VirtioError::AllocationFailed)?;
        let fb_phys = fb_frame.start_address().as_u64();
        let fb_virt = (hhdm + fb_phys) as *mut u32;

        unsafe {
            core::ptr::write_bytes(fb_virt as *mut u8, 0, fb_pages * 4096);
        }

        let resource_id = 1;
        let mut dev = Self {
            transport,
            ctrl_queue,
            _cursor_queue: cursor_queue,
            width,
            height,
            resource_id,
            fb_phys,
            fb_virt,
            _fb_pages: fb_pages,
            cmd_phys,
            cmd_virt,
        };

        // 1. Создаем аппаратный 2D-ресурс хоста
        dev.create_resource_2d()?;

        // 2. Привязываем физическую память гостевого фреймбуфера к ресурсу хоста
        dev.attach_backing()?;

        // 3. Назначаем ресурс на первичный дисплей (Scanout 0)
        dev.set_scanout()?;

        // 4. Первичный сброс буфера на экран
        dev.flush()?;

        Ok(dev)
    }

    /// Отправляет синхронную команду в очередь controlq и ожидает ответа
    fn send_cmd<T: Copy>(&mut self, cmd: &T) -> Result<(), VirtioError> {
        let cmd_size = core::mem::size_of::<T>();
        let resp_size = core::mem::size_of::<VirtioGpuCtrlHdr>();

        unsafe {
            core::ptr::copy_nonoverlapping(
                cmd as *const T as *const u8,
                self.cmd_virt,
                cmd_size,
            );
            core::ptr::write_bytes(self.cmd_virt.add(256), 0, resp_size);
        }

        fence(Ordering::SeqCst);

        let buffers = [
            VirtioBuffer {
                phys_addr: self.cmd_phys,
                len: cmd_size as u32,
                is_write: false,
            },
            VirtioBuffer {
                phys_addr: self.cmd_phys + 256,
                len: resp_size as u32,
                is_write: true,
            },
        ];

        let _head = self.ctrl_queue.add_buffer(&buffers)?;
        self.transport.notify_queue(0);

        let mut spins = 0u64;
        while !self.ctrl_queue.can_pop() {
            x86_64::instructions::nop();
            spins += 1;
            if spins > 10_000_000 {
                return Err(VirtioError::IoError);
            }
        }

        let (popped_head, _) = self.ctrl_queue.pop_used().ok_or(VirtioError::IoError)?;
        self.ctrl_queue.free_chain(popped_head);

        fence(Ordering::SeqCst);

        let resp = unsafe { *(self.cmd_virt.add(256) as *const VirtioGpuCtrlHdr) };
        if resp.type_ != VIRTIO_GPU_RESP_OK_NODATA && resp.type_ != VIRTIO_GPU_RESP_OK_DISPLAY_INFO {
            return Err(VirtioError::DeviceError((resp.type_ & 0xFF) as u8));
        }

        Ok(())
    }

    fn create_resource_2d(&mut self) -> Result<(), VirtioError> {
        let req = ResourceCreate2dReq {
            hdr: VirtioGpuCtrlHdr {
                type_: VIRTIO_GPU_CMD_RESOURCE_CREATE_2D,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                padding: 0,
            },
            resource_id: self.resource_id,
            format: VIRTIO_GPU_FORMAT_B8G8R8X8_UNORM,
            width: self.width,
            height: self.height,
        };
        self.send_cmd(&req)
    }

    fn attach_backing(&mut self) -> Result<(), VirtioError> {
        let fb_bytes = self.width * self.height * 4;
        let req = ResourceAttachBackingReq {
            hdr: VirtioGpuCtrlHdr {
                type_: VIRTIO_GPU_CMD_RESOURCE_ATTACH_BACKING,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                padding: 0,
            },
            resource_id: self.resource_id,
            nr_entries: 1,
            entry: MemEntry {
                addr: self.fb_phys,
                length: fb_bytes,
                padding: 0,
            },
        };
        self.send_cmd(&req)
    }

    fn set_scanout(&mut self) -> Result<(), VirtioError> {
        let req = SetScanoutReq {
            hdr: VirtioGpuCtrlHdr {
                type_: VIRTIO_GPU_CMD_SET_SCANOUT,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                padding: 0,
            },
            r: VirtioGpuRect {
                x: 0,
                y: 0,
                width: self.width,
                height: self.height,
            },
            scanout_id: 0,
            resource_id: self.resource_id,
        };
        self.send_cmd(&req)
    }

    /// Сбрасывает измененную область кадрового буфера в видеопамять дисплея хоста
    pub fn flush_rect(&mut self, x: u32, y: u32, width: u32, height: u32) -> Result<(), VirtioError> {
        // 1. Передаем пиксели из оперативной памяти гостя в текстуру 2D-ресурса хоста
        let transfer_req = TransferToHost2dReq {
            hdr: VirtioGpuCtrlHdr {
                type_: VIRTIO_GPU_CMD_TRANSFER_TO_HOST_2D,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                padding: 0,
            },
            r: VirtioGpuRect {
                x,
                y,
                width,
                height,
            },
            offset: ((y * self.width + x) * 4) as u64,
            resource_id: self.resource_id,
            padding: 0,
        };
        self.send_cmd(&transfer_req)?;

        // 2. Вызываем аппаратный Flush текстуры на физический дисплей
        let flush_req = ResourceFlushReq {
            hdr: VirtioGpuCtrlHdr {
                type_: VIRTIO_GPU_CMD_RESOURCE_FLUSH,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                padding: 0,
            },
            r: VirtioGpuRect {
                x,
                y,
                width,
                height,
            },
            resource_id: self.resource_id,
            padding: 0,
        };
        self.send_cmd(&flush_req)?;

        Ok(())
    }

    pub fn flush(&mut self) -> Result<(), VirtioError> {
        let w = self.width;
        let h = self.height;
        self.flush_rect(0, 0, w, h)
    }
}

impl VirtioDevice for VirtioGpu {
    fn name(&self) -> &str {
        "VirtIO-GPU"
    }

    fn device_type(&self) -> VirtioDeviceType {
        VirtioDeviceType::Gpu
    }

    fn reset(&mut self) {
        self.transport.reset();
    }
}

impl DisplayDevice for VirtioGpu {
    fn resolution(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn draw_pixel(&mut self, x: u32, y: u32, color: u32) {
        if x < self.width && y < self.height {
            let offset = (y * self.width + x) as usize;
            unsafe {
                core::ptr::write_volatile(self.fb_virt.add(offset), color);
            }
        }
    }

    fn flush(&mut self) -> Result<(), VirtioError> {
        VirtioGpu::flush(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_gpu_ctrl_hdr_and_rect_size() {
        assert_eq!(core::mem::size_of::<VirtioGpuCtrlHdr>(), 24);
        assert_eq!(core::mem::size_of::<VirtioGpuRect>(), 16);
    }
}
