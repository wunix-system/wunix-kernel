use alloc::vec::Vec;
use spin::Mutex;
use x86_64::structures::paging::PhysFrame;

use crate::memory::{allocate_contiguous_frames, deallocate_frame, hhdm_offset};

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VmbusGuid(pub [u8; 16]);

pub const GUID_NETVSC: VmbusGuid = VmbusGuid([
    0x63, 0x51, 0x61, 0xf8, 0x3e, 0xdf, 0xc5, 0x46, 0x91, 0x3f, 0xf2, 0xd2, 0xf9, 0x5d, 0x52, 0x93,
]);
pub const GUID_STORVSC: VmbusGuid = VmbusGuid([
    0xd8, 0x63, 0x61, 0xba, 0x28, 0x04, 0x61, 0x4d, 0x9b, 0x57, 0x0d, 0x19, 0xe7, 0x3b, 0x52, 0x3a,
]);
pub const GUID_SYNTHVID: VmbusGuid = VmbusGuid([
    0x02, 0x78, 0x0a, 0xda, 0x77, 0xe3, 0xac, 0x4a, 0x8e, 0x77, 0x05, 0x58, 0xeb, 0x10, 0xe6, 0xf2,
]);
pub const GUID_HEARTBEAT: VmbusGuid = VmbusGuid([
    0x39, 0x4f, 0x16, 0x57, 0x15, 0x91, 0x78, 0x4e, 0xab, 0x55, 0x38, 0x2f, 0x3b, 0xd5, 0x42, 0x2d,
]);
pub const GUID_SHUTDOWN: VmbusGuid = VmbusGuid([
    0x31, 0x60, 0x0b, 0x0e, 0x13, 0x52, 0x34, 0x49, 0x81, 0x8b, 0x38, 0xd9, 0x0c, 0xed, 0x39, 0xdb,
]);
pub const GUID_KVP: VmbusGuid = VmbusGuid([
    0xe0, 0xf4, 0xa0, 0xa9, 0x56, 0x56, 0x41, 0x44, 0x97, 0xda, 0x57, 0x83, 0x44, 0xe6, 0x72, 0x72,
]);

#[repr(C)]
pub struct VmbusRingBufferHeader {
    pub write_index: u32,
    pub read_index: u32,
    pub interrupt_mask: u32,
    pub pending_send_sz: u32,
    pub reserved: [u32; 12],
}

pub struct VmbusChannel {
    pub channel_id: u32,
    pub service_guid: VmbusGuid,
    dma_frame: PhysFrame,
    inbound_ring: *mut u8,
    outbound_ring: *mut u8,
    ring_size: usize,
}

unsafe impl Send for VmbusChannel {}
unsafe impl Sync for VmbusChannel {}

impl VmbusChannel {
    pub fn new(channel_id: u32, service_guid: VmbusGuid) -> Result<Self, &'static str> {
        let dma_frame = allocate_contiguous_frames(4)
            .ok_or("VMBus: failed to allocate channel ring buffer")?;
        let dma_phys = dma_frame.start_address().as_u64();
        let dma_virt = (hhdm_offset() + dma_phys) as *mut u8;

        unsafe {
            core::ptr::write_bytes(dma_virt, 0, 4 * 4096);
        }

        let ring_size = 2 * 4096;
        let outbound_ring = dma_virt;
        let inbound_ring = unsafe { dma_virt.add(ring_size) };

        Ok(Self {
            channel_id,
            service_guid,
            dma_frame,
            inbound_ring,
            outbound_ring,
            ring_size,
        })
    }

    pub fn send_packet(&mut self, packet_type: u16, data: &[u8]) -> Result<(), &'static str> {
        let total_size = core::mem::size_of::<VmbusPacketHeader>() + data.len();
        let ring_header = unsafe { &mut *(self.outbound_ring as *mut VmbusRingBufferHeader) };
        let data_capacity = self.ring_size - core::mem::size_of::<VmbusRingBufferHeader>();

        let cur_write = ring_header.write_index as usize;
        let cur_read = ring_header.read_index as usize;

        let available = if cur_write >= cur_read {
            data_capacity - (cur_write - cur_read)
        } else {
            cur_read - cur_write
        };

        if available < total_size + 8 {
            return Err("VMBus: outbound ring full");
        }

        let hdr = VmbusPacketHeader {
            packet_type,
            data_offset8: (core::mem::size_of::<VmbusPacketHeader>() / 8) as u16,
            length8: ((total_size + 7) / 8) as u16,
            flags: 0,
            transaction_id: 1,
        };

        let ring_data = unsafe { self.outbound_ring.add(core::mem::size_of::<VmbusRingBufferHeader>()) };
        unsafe {
            let hdr_ptr = ring_data.add(cur_write % data_capacity) as *mut VmbusPacketHeader;
            core::ptr::copy_nonoverlapping(&hdr as *const _ as *const u8, hdr_ptr as *mut u8, core::mem::size_of::<VmbusPacketHeader>());
            let payload_ptr = ring_data.add((cur_write + core::mem::size_of::<VmbusPacketHeader>()) % data_capacity);
            core::ptr::copy_nonoverlapping(data.as_ptr(), payload_ptr, data.len());
        }

        ring_header.write_index = ((cur_write + total_size) % data_capacity) as u32;
        Ok(())
    }

    pub fn receive_packet(&mut self, buf: &mut [u8]) -> Result<Option<usize>, &'static str> {
        let ring_header = unsafe { &mut *(self.inbound_ring as *mut VmbusRingBufferHeader) };
        let data_capacity = self.ring_size - core::mem::size_of::<VmbusRingBufferHeader>();

        let cur_read = ring_header.read_index as usize;
        let cur_write = ring_header.write_index as usize;

        if cur_read == cur_write {
            return Ok(None);
        }

        let ring_data = unsafe { self.inbound_ring.add(core::mem::size_of::<VmbusRingBufferHeader>()) };
        let hdr_ptr = unsafe { ring_data.add(cur_read % data_capacity) as *const VmbusPacketHeader };
        let hdr = unsafe { *hdr_ptr };

        let payload_len = (hdr.length8 as usize * 8).saturating_sub(core::mem::size_of::<VmbusPacketHeader>());
        let to_copy = payload_len.min(buf.len());

        if to_copy > 0 {
            let payload_ptr = unsafe { ring_data.add((cur_read + core::mem::size_of::<VmbusPacketHeader>()) % data_capacity) };
            unsafe {
                core::ptr::copy_nonoverlapping(payload_ptr, buf.as_mut_ptr(), to_copy);
            }
        }

        let total_packet_bytes = hdr.length8 as usize * 8;
        ring_header.read_index = ((cur_read + total_packet_bytes) % data_capacity) as u32;

        Ok(Some(to_copy))
    }
}

impl Drop for VmbusChannel {
    fn drop(&mut self) {
        deallocate_frame(self.dma_frame);
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VmbusPacketHeader {
    pub packet_type: u16,
    pub data_offset8: u16,
    pub length8: u16,
    pub flags: u16,
    pub transaction_id: u64,
}

pub static ACTIVE_CHANNELS: Mutex<Vec<VmbusChannel>> = Mutex::new(Vec::new());
