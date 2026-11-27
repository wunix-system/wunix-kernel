use alloc::collections::VecDeque;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;

use crate::drivers::manager::DeviceManager;
use crate::drivers::traits::{NetDevice, NetDeviceError};
use crate::drivers::usb::host::UsbHostController;
use crate::i18n::t;

const QUEUE_CAPACITY: usize = 64;

pub struct CdcNetDevice {
    mac: [u8; 6],
    pub slot_id: u8,
    pub bulk_in_ep: u8,
    pub bulk_out_ep: u8,
    host: Option<Arc<Mutex<dyn UsbHostController>>>,
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
            host: None,
            rx_queue: VecDeque::with_capacity(QUEUE_CAPACITY),
            tx_queue: VecDeque::with_capacity(QUEUE_CAPACITY),
            rx_bytes: 0,
            rx_packets: 0,
            tx_bytes: 0,
            tx_packets: 0,
        }
    }

    pub fn with_host(
        host: Arc<Mutex<dyn UsbHostController>>,
        slot_id: u8,
        in_ep: u8,
        out_ep: u8,
        mac: [u8; 6],
    ) -> Self {
        let mut dev = Self::new(slot_id, in_ep, out_ep, mac);
        dev.host = Some(host);
        dev
    }

    pub fn push_rx_packet(&mut self, packet: &[u8]) {
        if self.rx_queue.len() < QUEUE_CAPACITY {
            self.rx_queue.push_back(packet.to_vec());
        }
    }

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

        if let Some(ref host) = self.host {
            if self.bulk_out_ep == 0 {
                return Err(NetDeviceError::NotReady);
            }
            let mut copy = packet.to_vec();
            host.lock()
                .bulk_transfer(self.slot_id, self.bulk_out_ep, &mut copy, 200)
                .map_err(|_| NetDeviceError::IoError)?;
            self.tx_packets += 1;
            self.tx_bytes += packet.len();
            return Ok(());
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
            return Ok(Some(to_copy));
        }

        if let Some(ref host) = self.host {
            if self.bulk_in_ep == 0 || buf.is_empty() {
                return Ok(None);
            }
            match host
                .lock()
                .bulk_transfer(self.slot_id, self.bulk_in_ep, buf, 5)
            {
                Ok(n) if n > 0 => {
                    self.rx_packets += 1;
                    self.rx_bytes += n;
                    return Ok(Some(n));
                }
                _ => return Ok(None),
            }
        }

        Ok(None)
    }

    fn rx_bytes(&self) -> usize {
        self.rx_bytes
    }
    fn rx_packets(&self) -> usize {
        self.rx_packets
    }
    fn tx_bytes(&self) -> usize {
        self.tx_bytes
    }
    fn tx_packets(&self) -> usize {
        self.tx_packets
    }
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
        "usb_cdc_net: {} ({}: {}, MAC: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x})",
        t("USB Ethernet адаптер", "USB Ethernet adapter"),
        t("слот", "slot"),
        slot_id,
        mac[0],
        mac[1],
        mac[2],
        mac[3],
        mac[4],
        mac[5]
    );
    dm.register_net_device(Arc::new(Mutex::new(dev)));
}

pub fn probe_cdc_device_with_host(
    host: Arc<Mutex<dyn UsbHostController>>,
    slot_id: u8,
    in_ep: u8,
    out_ep: u8,
    mac: [u8; 6],
    dm: &mut DeviceManager,
) {
    let dev = CdcNetDevice::with_host(host, slot_id, in_ep, out_ep, mac);
    crate::klog!(
        "usb_cdc_net: {} ({}: {}, MAC: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x})",
        t("USB Ethernet адаптер", "USB Ethernet adapter"),
        t("слот", "slot"),
        slot_id,
        mac[0],
        mac[1],
        mac[2],
        mac[3],
        mac[4],
        mac[5]
    );
    dm.register_net_device(Arc::new(Mutex::new(dev)));
}
