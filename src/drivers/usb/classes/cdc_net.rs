use alloc::collections::VecDeque;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;
use crate::drivers::manager::DeviceManager;
use crate::drivers::traits::{NetDevice, NetDeviceError};

const QUEUE_CAPACITY: usize = 64;

pub struct CdcNetDevice {
    mac: [u8; 6],
    pub slot_id: u8,
    pub bulk_in_ep: u8,
    pub bulk_out_ep: u8,
    rx_queue: VecDeque<Vec<u8>>,
    tx_queue: VecDeque<Vec<u8>>,
    rx_bytes: usize,
    rx_packets: usize,
    tx_bytes: usize,
    tx_packets: usize,
}

impl CdcNetDevice {
    pub fn new(slot_id: u8, in_ep: u8, out_ep: u8, mac: [u8; 6]) -> Self {
        Self {
            mac,
            slot_id,
            bulk_in_ep: in_ep,
            bulk_out_ep: out_ep,
            rx_queue: VecDeque::with_capacity(QUEUE_CAPACITY),
            tx_queue: VecDeque::with_capacity(QUEUE_CAPACITY),
            rx_bytes: 0,
            rx_packets: 0,
            tx_bytes: 0,
            tx_packets: 0,
        }
    }

    /// Добавляет принятый по шине USB Ethernet-пакет в очередь приема
    pub fn push_rx_packet(&mut self, packet: &[u8]) {
        if self.rx_queue.len() < QUEUE_CAPACITY {
            self.rx_queue.push_back(packet.to_vec());
        }
    }

    /// Извлекает отправленный пакет для передачи в Bulk OUT endpoint контроллера
    pub fn pop_tx_packet(&mut self) -> Option<Vec<u8>> {
        self.tx_queue.pop_front()
    }
}

impl NetDevice for CdcNetDevice {
    fn driver_name(&self) -> &'static str {
        "usb-cdc-ethernet"
    }

    fn mac_address(&self) -> [u8; 6] {
        self.mac
    }

    fn send_packet(&mut self, packet: &[u8]) -> Result<(), NetDeviceError> {
        if packet.len() > 1514 {
            return Err(NetDeviceError::PacketTooLarge);
        }
        if self.tx_queue.len() >= QUEUE_CAPACITY {
            return Err(NetDeviceError::NotReady);
        }

        let len = packet.len();
        self.tx_queue.push_back(packet.to_vec());
        self.tx_packets += 1;
        self.tx_bytes += len;
        Ok(())
    }

    fn receive_packet(&mut self, buf: &mut [u8]) -> Result<Option<usize>, NetDeviceError> {
        if let Some(pkt) = self.rx_queue.pop_front() {
            let to_copy = pkt.len().min(buf.len());
            buf[..to_copy].copy_from_slice(&pkt[..to_copy]);
            self.rx_packets += 1;
            self.rx_bytes += to_copy;
            Ok(Some(to_copy))
        } else {
            Ok(None)
        }
    }

    fn rx_bytes(&self) -> usize { self.rx_bytes }
    fn rx_packets(&self) -> usize { self.rx_packets }
    fn tx_bytes(&self) -> usize { self.tx_bytes }
    fn tx_packets(&self) -> usize { self.tx_packets }
}

pub fn probe_cdc_device(
    slot_id: u8,
    in_ep: u8,
    out_ep: u8,
    mac: [u8; 6],
    dm: &mut DeviceManager,
) {
    let dev = CdcNetDevice::new(slot_id, in_ep, out_ep, mac);
    crate::klog!(
        "usb_cdc_net: registered USB Ethernet adapter on slot {} (MAC: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x})",
        slot_id, mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
    );
    dm.register_net_device(Arc::new(Mutex::new(dev)));
}
