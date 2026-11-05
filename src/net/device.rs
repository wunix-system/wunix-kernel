use alloc::sync::Arc;
use alloc::vec::Vec;
use smoltcp::phy::{ChecksumCapabilities, Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::time::Instant;
use spin::Mutex;

use crate::drivers::NetDevice;

/// Унифицированный сетевой адаптер канального уровня `smoltcp::phy::Device` поверх
/// любого драйвера ядра, реализующего интерфейс [`NetDevice`] (`Arc<Mutex<dyn NetDevice>>`).
///
/// Изолирует сетевой стек `smoltcp` от деталей конкретного железа (VirtIO, VMXNET3, NetVSC, e1000).
pub struct NetDeviceSmolAdapter {
    net: Arc<Mutex<dyn NetDevice>>,
}

/// Псевдоним типа для обратной совместимости
pub type VirtioNetSmolDevice = NetDeviceSmolAdapter;

impl NetDeviceSmolAdapter {
    pub fn new(net: Arc<Mutex<dyn NetDevice>>) -> Self {
        Self { net }
    }

    pub fn net_dev(&self) -> &Arc<Mutex<dyn NetDevice>> {
        &self.net
    }
}

pub struct NetRxToken(Vec<u8>);

impl RxToken for NetRxToken {
    fn consume<R, F>(mut self, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        f(&mut self.0)
    }
}

pub struct NetTxToken(Arc<Mutex<dyn NetDevice>>);

impl TxToken for NetTxToken {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let mut buf = alloc::vec![0u8; len];
        let res = f(&mut buf);
        let _ = self.0.lock().send_packet(&buf);
        res
    }
}

impl Device for NetDeviceSmolAdapter {
    type RxToken<'a> = NetRxToken where Self: 'a;
    type TxToken<'a> = NetTxToken where Self: 'a;

    fn receive(&mut self, _timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        // Стандартный MTU Ethernet — 1514 байт (1500 байт MTU + 14 байт заголовка Ethernet)
        let mut rx_buf = [0u8; 1514];
        match self.net.lock().receive_packet(&mut rx_buf) {
            Ok(Some(len)) if len > 0 => {
                let frame = rx_buf[..len].to_vec();
                Some((NetRxToken(frame), NetTxToken(self.net.clone())))
            }
            _ => None,
        }
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
        Some(NetTxToken(self.net.clone()))
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ethernet;
        caps.max_transmission_unit = 1514;
        caps.checksum = ChecksumCapabilities::default();
        caps
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drivers::traits::NetDeviceError;

    struct MockNetDevice {
        mac: [u8; 6],
        rx_queue: Vec<Vec<u8>>,
        tx_history: Vec<Vec<u8>>,
    }

    impl MockNetDevice {
        fn new(mac: [u8; 6]) -> Self {
            Self {
                mac,
                rx_queue: Vec::new(),
                tx_history: Vec::new(),
            }
        }
    }

    impl NetDevice for MockNetDevice {
        fn driver_name(&self) -> &'static str {
            "mock-nic"
        }

        fn mac_address(&self) -> [u8; 6] {
            self.mac
        }

        fn send_packet(&mut self, packet: &[u8]) -> Result<(), NetDeviceError> {
            self.tx_history.push(packet.to_vec());
            Ok(())
        }

        fn receive_packet(&mut self, buf: &mut [u8]) -> Result<Option<usize>, NetDeviceError> {
            if self.rx_queue.is_empty() {
                return Ok(None);
            }
            let packet = self.rx_queue.remove(0);
            let len = packet.len().min(buf.len());
            buf[..len].copy_from_slice(&packet[..len]);
            Ok(Some(len))
        }
    }

    #[test_case]
    fn test_adapter_mac_and_driver_name() {
        let mock = Arc::new(Mutex::new(MockNetDevice::new([0x02, 0x00, 0x00, 0x11, 0x22, 0x33])));
        let adapter = NetDeviceSmolAdapter::new(mock.clone());
        assert_eq!(adapter.net_dev().lock().driver_name(), "mock-nic");
        assert_eq!(adapter.net_dev().lock().mac_address(), [0x02, 0x00, 0x00, 0x11, 0x22, 0x33]);
    }

    #[test_case]
    fn test_adapter_send_and_receive() {
        let mock = Arc::new(Mutex::new(MockNetDevice::new([0x52, 0x54, 0x00, 0x12, 0x34, 0x56])));
        mock.lock().rx_queue.push(alloc::vec![0xAA, 0xBB, 0xCC, 0xDD]);

        let mut adapter = NetDeviceSmolAdapter::new(mock.clone());
        let caps = adapter.capabilities();
        assert_eq!(caps.max_transmission_unit, 1514);

        let rx = adapter.receive(Instant::from_millis(0));
        assert!(rx.is_some());
        let (rx_token, tx_token) = rx.unwrap();

        let received = rx_token.consume(|buf| buf.to_vec());
        assert_eq!(received, alloc::vec![0xAA, 0xBB, 0xCC, 0xDD]);

        tx_token.consume(3, |buf| {
            buf.copy_from_slice(&[1, 2, 3]);
        });
        assert_eq!(mock.lock().tx_history.len(), 1);
        assert_eq!(mock.lock().tx_history[0], alloc::vec![1, 2, 3]);
    }
}
