pub mod frame;

use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

#[derive(Debug, Clone)]
pub struct WirelessNetwork {
    pub ssid: String,
    pub bssid: [u8; 6],
    pub channel: u8,
    pub rssi: i8, // Уровень сигнала в dBm
    pub security: SecurityType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecurityType {
    Open,
    Wpa2Psk,
    Wpa3Sae,
}

pub static SCAN_RESULTS: Mutex<Vec<WirelessNetwork>> = Mutex::new(Vec::new());

/// Регистрирует обнаруженную беспроводную сеть в результатах сканирования
pub fn register_scanned_network(net: WirelessNetwork) {
    let mut list = SCAN_RESULTS.lock();
    if let Some(existing) = list.iter_mut().find(|n| n.bssid == net.bssid) {
        existing.rssi = net.rssi;
        existing.channel = net.channel;
    } else {
        list.push(net);
    }
}
