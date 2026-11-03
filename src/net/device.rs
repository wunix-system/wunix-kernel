use alloc::sync::Arc;
use alloc::vec::Vec;
use smoltcp::phy::{ChecksumCapabilities, Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::time::Instant;
use spin::Mutex;

use crate::drivers::NetDevice;

/// Адаптер сетевого устройства `smoltcp::phy::Device` поверх зарегистрированного
/// драйвера `virtio-net` (`Arc<Mutex<dyn NetDevice>>`).
///
/// Обеспечивает прием и передачу полных Ethernet-кадров (включая 14-байтный заголовок MAC).
pub struct VirtioNetSmolDevice {
    net: Arc<Mutex<dyn NetDevice>>,
}

impl VirtioNetSmolDevice {
    pub fn new(net: Arc<Mutex<dyn NetDevice>>) -> Self {
        Self { net }
    }

    pub fn net_dev(&self) -> &Arc<Mutex<dyn NetDevice>> {
        &self.net
    }
}

pub struct VirtioRxToken(Vec<u8>);

impl RxToken for VirtioRxToken {
    fn consume<R, F>(mut self, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        f(&mut self.0)
    }
}

pub struct VirtioTxToken(Arc<Mutex<dyn NetDevice>>);

impl TxToken for VirtioTxToken {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        // Выделяем временный буфер под размер исходящего кадра,
        // даем smoltcp заполнить заголовки L2/L3/L4 и сразу выталкиваем в VirtQueue TX.
        let mut buf = alloc::vec![0u8; len];
        let res = f(&mut buf);
        let _ = self.0.lock().send_packet(&buf);
        res
    }
}

impl Device for VirtioNetSmolDevice {
    type RxToken<'a> = VirtioRxToken where Self: 'a;
    type TxToken<'a> = VirtioTxToken where Self: 'a;

    fn receive(&mut self, _timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        // Стандартный MTU Ethernet — 1514 байт (1500 байт MTU + 14 байт заголовка Ethernet).
        let mut rx_buf = [0u8; 1514];
        match self.net.lock().receive_packet(&mut rx_buf) {
            Ok(Some(len)) if len > 0 => {
                let frame = rx_buf[..len].to_vec();
                Some((VirtioRxToken(frame), VirtioTxToken(self.net.clone())))
            }
            _ => None,
        }
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
        Some(VirtioTxToken(self.net.clone()))
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ethernet;
        // 1514: 1500 байт IP MTU + 14 байт Ethernet MAC-заголовка
        caps.max_transmission_unit = 1514;
        caps.checksum = ChecksumCapabilities::default();
        caps
    }
}
