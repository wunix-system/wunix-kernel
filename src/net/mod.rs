pub mod device;
pub mod socket;

use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;

use smoltcp::iface::{Config, Interface, SocketHandle, SocketSet};
use smoltcp::socket::dhcpv4;
use smoltcp::time::Instant;
use smoltcp::wire::{EthernetAddress, HardwareAddress, IpAddress, IpCidr, IpEndpoint, Ipv4Address};

use self::device::NetDeviceSmolAdapter;
pub use self::socket::{create_socket, SockAddrIn, SocketFileHandle};
use crate::drivers::NetDevice;
use crate::serial_println;

/// Вычисляет IPv4-маску подсети в сетевом порядке байт по длине префикса CIDR
pub fn prefix_to_netmask(prefix: u8) -> [u8; 4] {
    let mask: u32 = if prefix == 0 {
        0
    } else if prefix >= 32 {
        0xFFFF_FFFF
    } else {
        !0u32 << (32 - prefix)
    };
    mask.to_be_bytes()
}

/// Глобальное состояние сетевого стека Wunix на базе `smoltcp`
pub struct NetworkStack {
    pub iface: Interface,
    pub device: NetDeviceSmolAdapter,
    pub sockets: SocketSet<'static>,
    pub dhcp_handle: SocketHandle,
    pub mac: [u8; 6],
    pub ip: [u8; 4],
    pub netmask: [u8; 4],
    pub gateway: [u8; 4],
    pub dns: [u8; 4],
    pub dhcp_active: bool,
}

pub static NET_STACK: Mutex<Option<NetworkStack>> = Mutex::new(None);

#[derive(Debug, Clone, Copy)]
pub struct NetInfo {
    pub mac: [u8; 6],
    pub ip: [u8; 4],
    pub netmask: [u8; 4],
    pub gateway: [u8; 4],
    pub dns: [u8; 4],
}

/// Инициализация сетевого стека ядра поверх сетевого адаптера `NetDevice` с автонастройкой по DHCP
pub fn init(net_dev: Arc<Mutex<dyn NetDevice>>) {
    let mac = net_dev.lock().mac_address();
    let mut device = NetDeviceSmolAdapter::new(net_dev.clone());

    // Конфигурируем L2 MAC-адрес интерфейса
    let config = Config::new(HardwareAddress::Ethernet(EthernetAddress(mac)));
    let mut iface = Interface::new(config, &mut device, Instant::from_millis(0));

    let mut sockets = SocketSet::new(alloc::vec![]);

    // Регистрируем сокет DHCPv4 клиента для автоматического получения IP, шлюза и DNS
    let dhcp_socket = dhcpv4::Socket::new();
    let dhcp_handle = sockets.add(dhcp_socket);

    serial_println!("net: discovering network configuration via DHCPv4...");

    let mut ip = [0u8; 4];
    let mut netmask = [0u8; 4];
    let mut gateway = [0u8; 4];
    let mut dns = [1, 1, 1, 1]; // Нейтральный глобальный fallback Cloudflare DNS
    let mut dhcp_ok = false;

    // Первоначальный опрос DHCP-сервера (таймаут ~1.5 секунды / 150 тиков)
    let start_tick = crate::arch::x86_64::timer::ticks();
    while crate::arch::x86_64::timer::ticks().saturating_sub(start_tick) < 150 {
        let now_ms = crate::arch::x86_64::timer::uptime_ms() as i64;
        let now = Instant::from_millis(now_ms);
        let _ = iface.poll(now, &mut device, &mut sockets);

        let event = sockets.get_mut::<dhcpv4::Socket>(dhcp_handle).poll();
        if let Some(dhcpv4::Event::Configured(cfg)) = event {
            let cidr = cfg.address;
            ip = cidr.address().0;
            netmask = prefix_to_netmask(cidr.prefix_len());

            iface.update_ip_addrs(|addrs| {
                addrs.clear();
                let _ = addrs.push(IpCidr::Ipv4(cidr));
            });

            if let Some(router) = cfg.router {
                gateway = router.0;
                let _ = iface.routes_mut().add_default_ipv4_route(router);
            }

            if let Some(dns_server) = cfg.dns_servers.first() {
                dns = dns_server.0;
            }

            dhcp_ok = true;
            break;
        }

        core::hint::spin_loop();
    }

    if !dhcp_ok {
        // Если DHCP-сервер в сети не обнаружен, используем fallback
        serial_println!("net: DHCP discovery timed out, applying fallback static configuration");
        ip = [10, 0, 2, 15];
        netmask = [255, 255, 255, 0];
        gateway = [10, 0, 2, 2];
        dns = [10, 0, 2, 3];

        iface.update_ip_addrs(|addrs| {
            let cidr = IpCidr::new(IpAddress::v4(ip[0], ip[1], ip[2], ip[3]), 24);
            let _ = addrs.push(cidr);
        });

        let _ = iface.routes_mut().add_default_ipv4_route(Ipv4Address::new(
            gateway[0], gateway[1], gateway[2], gateway[3],
        ));
    }

    // Отправляем ARP-запрос шлюзу (gateway) для немедленного прогрева кэша соседей (neighbor_cache).
    // Без этого первый исходящий TCP SYN к внешним хостам сбрасывается smoltcp в ожидании ARP,
    // переводя сокет в режим молчания (DISCOVERY_SILENT_TIME) ровно на 1000 мс (1 секунду задержки!).
    let mut arp_frame = [0u8; 42];
    arp_frame[0..6].copy_from_slice(&[0xFF; 6]); // Broadcast MAC
    arp_frame[6..12].copy_from_slice(&mac);
    arp_frame[12..14].copy_from_slice(&[0x08, 0x06]); // ARP
    arp_frame[14..16].copy_from_slice(&[0x00, 0x01]); // HW Type: Ethernet
    arp_frame[16..18].copy_from_slice(&[0x08, 0x00]); // Proto: IPv4
    arp_frame[18] = 6;
    arp_frame[19] = 4;
    arp_frame[20..22].copy_from_slice(&[0x00, 0x01]); // ARP Request
    arp_frame[22..28].copy_from_slice(&mac);
    arp_frame[28..32].copy_from_slice(&ip);
    arp_frame[32..38].copy_from_slice(&[0x00; 6]);
    arp_frame[38..42].copy_from_slice(&gateway);
    let _ = net_dev.lock().send_packet(&arp_frame);

    // Даем smoltcp обработать ответ ARP от шлюза
    for _ in 0..5 {
        let now_ms = crate::arch::x86_64::timer::uptime_ms() as i64;
        let now = Instant::from_millis(now_ms);
        let _ = iface.poll(now, &mut device, &mut sockets);
    }

    // Сохраняем состояние стека
    *NET_STACK.lock() = Some(NetworkStack {
        iface,
        device,
        sockets,
        dhcp_handle,
        mac,
        ip,
        netmask,
        gateway,
        dns,
        dhcp_active: dhcp_ok,
    });

    // Динамически создаем /etc/resolv.conf с полученным DNS сервером
    let resolv_content = alloc::format!(
        "# /etc/resolv.conf — Конфигурация DNS-резолвера Wunix\n\
         # Сгенерировано автоматически по протоколу DHCP\n\
         nameserver {}.{}.{}.{}\n",
        dns[0], dns[1], dns[2], dns[3]
    );
    let _ = crate::fs::ops::write_file_ramfs("/etc/resolv.conf", resolv_content.as_bytes());

    if dhcp_ok {
        serial_println!(
            "net: eth0 ready (DHCP lease): IP {}.{}.{}.{}, Mask {}.{}.{}.{}, GW {}.{}.{}.{}, DNS {}.{}.{}.{}",
            ip[0], ip[1], ip[2], ip[3],
            netmask[0], netmask[1], netmask[2], netmask[3],
            gateway[0], gateway[1], gateway[2], gateway[3],
            dns[0], dns[1], dns[2], dns[3]
        );
    } else {
        serial_println!(
            "net: eth0 ready (fallback): IP {}.{}.{}.{}/24, GW {}.{}.{}.{}",
            ip[0], ip[1], ip[2], ip[3],
            gateway[0], gateway[1], gateway[2], gateway[3]
        );
    }
}

static LAST_ARP_REFRESH: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

fn send_gateway_arp_probe(stack: &mut NetworkStack) {
    let mut arp_frame = [0u8; 42];
    arp_frame[0..6].copy_from_slice(&[0xFF; 6]); // Broadcast MAC
    arp_frame[6..12].copy_from_slice(&stack.mac);
    arp_frame[12..14].copy_from_slice(&[0x08, 0x06]); // ARP
    arp_frame[14..16].copy_from_slice(&[0x00, 0x01]); // HW Type: Ethernet
    arp_frame[16..18].copy_from_slice(&[0x08, 0x00]); // Proto: IPv4
    arp_frame[18] = 6;
    arp_frame[19] = 4;
    arp_frame[20..22].copy_from_slice(&[0x00, 0x01]); // ARP Request
    arp_frame[22..28].copy_from_slice(&stack.mac);
    arp_frame[28..32].copy_from_slice(&stack.ip);
    arp_frame[32..38].copy_from_slice(&[0x00; 6]);
    arp_frame[38..42].copy_from_slice(&stack.gateway);

    let _ = stack.device.net_dev().lock().send_packet(&arp_frame);
}

/// Периодический поллинг сетевого интерфейса, очередей сокетов и обновлений DHCP
pub fn poll() {
    if let Some(mut guard) = NET_STACK.try_lock() {
        if let Some(stack) = guard.as_mut() {
            let now_ms = crate::arch::x86_64::timer::uptime_ms() as i64;
            let now = Instant::from_millis(now_ms);
            let _ = stack.iface.poll(now, &mut stack.device, &mut stack.sockets);

            // Каждые 30 секунд (3000 тиков) отправляем ARP-запрос к шлюзу,
            // чтобы запись никогда не устаревала в neighbor_cache smoltcp (время жизни 60с)
            let ticks = crate::arch::x86_64::timer::ticks();
            let last = LAST_ARP_REFRESH.load(core::sync::atomic::Ordering::Relaxed);
            if ticks.saturating_sub(last) > 3000 {
                LAST_ARP_REFRESH.store(ticks, core::sync::atomic::Ordering::Relaxed);
                send_gateway_arp_probe(stack);
            }

            // Обрабатываем обновления аренды DHCP
            let event = stack.sockets.get_mut::<dhcpv4::Socket>(stack.dhcp_handle).poll();
            if let Some(dhcpv4::Event::Configured(cfg)) = event {
                let cidr = cfg.address;
                stack.ip = cidr.address().0;
                stack.netmask = prefix_to_netmask(cidr.prefix_len());
                stack.iface.update_ip_addrs(|addrs| {
                    addrs.clear();
                    let _ = addrs.push(IpCidr::Ipv4(cidr));
                });
                if let Some(router) = cfg.router {
                    stack.gateway = router.0;
                    let _ = stack.iface.routes_mut().add_default_ipv4_route(router);
                }
                if let Some(dns_server) = cfg.dns_servers.first() {
                    stack.dns = dns_server.0;
                }
                stack.dhcp_active = true;
            } else if let Some(dhcpv4::Event::Deconfigured) = event {
                stack.dhcp_active = false;
            }
        }
    }
}

pub fn is_initialized() -> bool {
    NET_STACK.lock().is_some()
}

pub fn get_info() -> Option<NetInfo> {
    NET_STACK.lock().as_ref().map(|s| NetInfo {
        mac: s.mac,
        ip: s.ip,
        netmask: s.netmask,
        gateway: s.gateway,
        dns: s.dns,
    })
}

#[derive(Debug, Clone)]
pub struct SocketDumpEntry {
    pub proto: &'static str,
    pub state: &'static str,
    pub local_ip: [u8; 4],
    pub local_port: u16,
    pub remote_ip: [u8; 4],
    pub remote_port: u16,
    pub rx_queue: usize,
    pub tx_queue: usize,
}

/// Собирает список открытых сокетов для утилиты `ss` и `/proc/net/`
pub fn dump_sockets() -> Vec<SocketDumpEntry> {
    let mut list = Vec::new();
    let mut guard = NET_STACK.lock();
    if let Some(stack) = guard.as_mut() {
        for (_handle, socket) in stack.sockets.iter_mut() {
            match socket {
                smoltcp::socket::Socket::Tcp(s) => {
                    let local = s.local_endpoint().unwrap_or(IpEndpoint::new(IpAddress::v4(0, 0, 0, 0), 0));
                    let remote = s.remote_endpoint().unwrap_or(IpEndpoint::new(IpAddress::v4(0, 0, 0, 0), 0));

                    let local_ip = match local.addr {
                        IpAddress::Ipv4(v4) => v4.0,
                    };
                    let remote_ip = match remote.addr {
                        IpAddress::Ipv4(v4) => v4.0,
                    };

                    let state_str = match s.state() {
                        smoltcp::socket::tcp::State::Closed => "CLOSED",
                        smoltcp::socket::tcp::State::Listen => "LISTEN",
                        smoltcp::socket::tcp::State::SynSent => "SYN_SENT",
                        smoltcp::socket::tcp::State::SynReceived => "SYN_RECV",
                        smoltcp::socket::tcp::State::Established => "ESTAB",
                        smoltcp::socket::tcp::State::FinWait1 => "FIN_WAIT1",
                        smoltcp::socket::tcp::State::FinWait2 => "FIN_WAIT2",
                        smoltcp::socket::tcp::State::CloseWait => "CLOSE_WAIT",
                        smoltcp::socket::tcp::State::Closing => "CLOSING",
                        smoltcp::socket::tcp::State::LastAck => "LAST_ACK",
                        smoltcp::socket::tcp::State::TimeWait => "TIME_WAIT",
                    };

                    list.push(SocketDumpEntry {
                        proto: "tcp",
                        state: state_str,
                        local_ip,
                        local_port: local.port,
                        remote_ip,
                        remote_port: remote.port,
                        rx_queue: s.recv_queue(),
                        tx_queue: s.send_queue(),
                    });
                }
                smoltcp::socket::Socket::Udp(s) => {
                    let local = s.endpoint();
                    let local_port = local.port;
                    let local_ip = match local.addr {
                        Some(IpAddress::Ipv4(v4)) => v4.0,
                        _ => [0, 0, 0, 0],
                    };

                    list.push(SocketDumpEntry {
                        proto: "udp",
                        state: "UNCONN",
                        local_ip,
                        local_port,
                        remote_ip: [0, 0, 0, 0],
                        remote_port: 0,
                        rx_queue: 0,
                        tx_queue: 0,
                    });
                }
                _ => {}
            }
        }
    }
    list
}
