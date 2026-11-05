use alloc::sync::Arc;
use spin::Mutex;
use crate::drivers::manager::DeviceManager;
use crate::drivers::traits::{NetDevice, NetDeviceError};

pub struct CdcNetDevice {
    mac: [u8; 6],
    pub _slot_id: u8,
    pub _bulk_in_ep: u8,
    pub _bulk_out_ep: u8,
    rx_bytes: usize,
    rx_packets: usize,
    tx_bytes: usize,
    tx_packets: usize,
}

impl CdcNetDevice {
    pub fn new(slot_id: u8, in_ep: u8, out_ep: u8, mac: [u8; 6]) -> Self {
        Self {
            mac,
            _slot_id: slot_id,
            _bulk_in_ep: in_ep,
            _bulk_out_ep: out_ep,
            rx_bytes: 0,
            rx_packets: 0,
            tx_bytes: 0,
            tx_packets: 0,
        }
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
        let len = packet.len();
        self.tx_packets += 1;
        self.tx_bytes += len;
        Ok(())
    }

    fn receive_packet(&mut self, _buf: &mut [u8]) -> Result<Option<usize>, NetDeviceError> {
        Ok(None)
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
