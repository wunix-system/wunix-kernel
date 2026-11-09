use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, Ordering};
use spin::Mutex;

use smoltcp::iface::SocketHandle as SmolHandle;
use smoltcp::socket::icmp::{Endpoint as IcmpEndpoint, PacketBuffer as IcmpPacketBuffer, PacketMetadata as IcmpPacketMetadata, Socket as IcmpSocket};
use smoltcp::socket::tcp::{self, Socket as TcpSocket, SocketBuffer as TcpSocketBuffer, State as TcpState};
use smoltcp::socket::udp::{PacketBuffer as UdpPacketBuffer, PacketMetadata as UdpPacketMetadata, Socket as UdpSocket};
use smoltcp::wire::{IpAddress, IpEndpoint, IpListenEndpoint};

use crate::fs::{FileHandle, EPOLLERR, EPOLLHUP, EPOLLIN, EPOLLOUT, EPOLLRDHUP, O_NONBLOCK};
use crate::task::scheduler;

pub const AF_UNIX: u16 = 1;
pub const AF_INET: u16 = 2;

pub const SOCK_STREAM: u32 = 1;
pub const SOCK_DGRAM: u32 = 2;
pub const SOCK_RAW: u32 = 3;
pub const SOCK_NONBLOCK: u32 = 0x800;
pub const SOCK_CLOEXEC: u32 = 0x80000;

pub const IPPROTO_ICMP: i32 = 1;
pub const IPPROTO_TCP: i32 = 6;
pub const IPPROTO_UDP: i32 = 17;

// Генератор эфемерных портов для исходящих TCP/UDP подключений (IANA 49152..65535)
static NEXT_EPHEMERAL_PORT: AtomicU16 = AtomicU16::new(49152);

pub fn alloc_ephemeral_port() -> u16 {
    let port = NEXT_EPHEMERAL_PORT.fetch_add(1, Ordering::Relaxed);
    if port < 49152 || port > 65530 {
        NEXT_EPHEMERAL_PORT.store(49152, Ordering::Relaxed);
        49152
    } else {
        port
    }
}

/// Linux-совместимая структура IPv4 адреса сокета (`struct sockaddr_in`)
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct SockAddrIn {
    pub sin_family: u16,
    pub sin_port: u16,   // Big-endian сетевой порядок
    pub sin_addr: u32,   // Big-endian IPv4 адрес
    pub sin_zero: [u8; 8],
}

impl SockAddrIn {
    pub fn new_v4(octets: [u8; 4], port: u16) -> Self {
        let addr = u32::from_ne_bytes(octets);
        Self {
            sin_family: AF_INET,
            sin_port: port.to_be(),
            sin_addr: addr,
            sin_zero: [0u8; 8],
        }
    }

    pub fn to_endpoint(&self) -> IpEndpoint {
        let port = u16::from_be(self.sin_port);
        let b = self.sin_addr.to_ne_bytes();
        IpEndpoint::new(IpAddress::v4(b[0], b[1], b[2], b[3]), port)
    }

    pub fn from_endpoint(ep: IpEndpoint) -> Self {
        let b = match ep.addr {
            IpAddress::Ipv4(v4) => v4.0,
        };
        let sin_addr = u32::from_ne_bytes(b);
        Self {
            sin_family: AF_INET,
            sin_port: ep.port.to_be(),
            sin_addr,
            sin_zero: [0u8; 8],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmolSocketKind {
    Tcp(SmolHandle),
    Udp(SmolHandle),
    Icmp(SmolHandle),
    Unix,
}

/// Сокетный дескриптор ядра Wunix
pub struct SocketFileHandle {
    pub domain: u16,
    pub sock_type: u32,
    pub protocol: i32,
    pub flags: AtomicU32,
    pub smol_kind: Mutex<SmolSocketKind>,
    pub bound_endpoint: Mutex<Option<IpEndpoint>>,
    pub peer_endpoint: Mutex<Option<IpEndpoint>>,
    pub is_listening: AtomicBool,
    pub unix_buffer: Mutex<Vec<u8>>,
}

impl SocketFileHandle {
    pub fn new(domain: u16, sock_type: u32, protocol: i32, flags: u32, kind: SmolSocketKind) -> Self {
        Self {
            domain,
            sock_type,
            protocol,
            flags: AtomicU32::new(flags),
            smol_kind: Mutex::new(kind),
            bound_endpoint: Mutex::new(None),
            peer_endpoint: Mutex::new(None),
            is_listening: AtomicBool::new(false),
            unix_buffer: Mutex::new(Vec::new()),
        }
    }
}

impl FileHandle for SocketFileHandle {
    fn read(&self, buf: &mut [u8]) -> Result<usize, i64> {
        if buf.is_empty() {
            return Ok(0);
        }

        let kind = *self.smol_kind.lock();
        match kind {
            SmolSocketKind::Unix => {
                loop {
                    let mut q = self.unix_buffer.lock();
                    if !q.is_empty() {
                        let to_read = buf.len().min(q.len());
                        let drained: Vec<u8> = q.drain(0..to_read).collect();
                        buf[..to_read].copy_from_slice(&drained);
                        return Ok(to_read);
                    }
                    if (self.flags.load(Ordering::Relaxed) & O_NONBLOCK) != 0 {
                        return Err(-11); // -EAGAIN
                    }
                    drop(q);
                    scheduler::yield_now();
                }
            }
            SmolSocketKind::Tcp(handle) => {
                let start_tick = crate::arch::x86_64::timer::ticks();
                loop {
                    if crate::task::signal::is_sigint_pending_for_current() {
                        return Err(-4); // -EINTR
                    }

                    crate::net::poll();

                    {
                        let mut guard = crate::net::NET_STACK.lock();
                        if let Some(stack) = guard.as_mut() {
                            if !stack.sockets.iter().any(|(h, _)| h == handle) {
                                return Ok(0);
                            }
                            let sock = stack.sockets.get_mut::<TcpSocket>(handle);
                            if sock.can_recv() {
                                match sock.recv_slice(buf) {
                                    Ok(n) => return Ok(n),
                                    Err(tcp::RecvError::Finished) => return Ok(0), // EOF: соединение закрыто пиром
                                    Err(_) => return Err(-5), // -EIO
                                }
                            }
                            if !sock.may_recv()
                                || !sock.is_active()
                                || sock.state() == TcpState::CloseWait
                                || sock.state() == TcpState::Closed
                            {
                                return Ok(0); // EOF: удалённая сторона завершила передачу (получен TCP FIN)
                            }
                        } else {
                            return Err(-107); // -ENOTCONN
                        }
                    }

                    if (self.flags.load(Ordering::Relaxed) & O_NONBLOCK) != 0 {
                        return Err(-11); // -EAGAIN
                    }

                    // Таймаут на чтение (10 секунд для предотвращения вечного зависания)
                    if crate::arch::x86_64::timer::ticks().saturating_sub(start_tick) > 1000 {
                        return Err(-110); // -ETIMEDOUT
                    }

                    scheduler::yield_now();
                }
            }
            SmolSocketKind::Udp(handle) => {
                let start_tick = crate::arch::x86_64::timer::ticks();
                loop {
                    if crate::task::signal::is_sigint_pending_for_current() {
                        return Err(-4); // -EINTR
                    }

                    crate::net::poll();
                    {
                        let mut guard = crate::net::NET_STACK.lock();
                        if let Some(stack) = guard.as_mut() {
                            let sock = stack.sockets.get_mut::<UdpSocket>(handle);
                            if sock.can_recv() {
                                match sock.recv_slice(buf) {
                                    Ok((n, meta)) => {
                                        *self.peer_endpoint.lock() = Some(meta.endpoint);
                                        return Ok(n);
                                    }
                                    Err(_) => return Err(-5), // -EIO
                                }
                            }
                        } else {
                            return Err(-107); // -ENOTCONN
                        }
                    }

                    if (self.flags.load(Ordering::Relaxed) & O_NONBLOCK) != 0 {
                        return Err(-11); // -EAGAIN
                    }

                    // Таймаут на чтение дейтаграммы UDP (300 тиков = ~3 секунды)
                    if crate::arch::x86_64::timer::ticks().saturating_sub(start_tick) > 300 {
                        return Err(-110); // -ETIMEDOUT
                    }

                    scheduler::yield_now();
                }
            }
            SmolSocketKind::Icmp(handle) => {
                let start_tick = crate::arch::x86_64::timer::ticks();
                loop {
                    if crate::task::signal::is_sigint_pending_for_current() {
                        return Err(-4); // -EINTR
                    }

                    crate::net::poll();
                    {
                        let mut guard = crate::net::NET_STACK.lock();
                        if let Some(stack) = guard.as_mut() {
                            let sock = stack.sockets.get_mut::<IcmpSocket>(handle);
                            if sock.can_recv() {
                                match sock.recv_slice(buf) {
                                    Ok((n, addr)) => {
                                        *self.peer_endpoint.lock() = Some(IpEndpoint::new(addr, 0));
                                        return Ok(n);
                                    }
                                    Err(_) => return Err(-5), // -EIO
                                }
                            }
                        } else {
                            return Err(-107); // -ENOTCONN
                        }
                    }

                    if (self.flags.load(Ordering::Relaxed) & O_NONBLOCK) != 0 {
                        return Err(-11); // -EAGAIN
                    }

                    // Таймаут на чтение эхо-ответа ICMP (300 тиков = ~3 секунды)
                    if crate::arch::x86_64::timer::ticks().saturating_sub(start_tick) > 300 {
                        return Err(-110); // -ETIMEDOUT
                    }

                    scheduler::yield_now();
                }
            }
        }
    }

    fn write(&self, buf: &[u8]) -> Result<usize, i64> {
        if buf.is_empty() {
            return Ok(0);
        }

        let kind = *self.smol_kind.lock();
        match kind {
            SmolSocketKind::Unix => {
                let mut q = self.unix_buffer.lock();
                q.extend_from_slice(buf);
                Ok(buf.len())
            }
            SmolSocketKind::Tcp(handle) => {
                let start_tick = crate::arch::x86_64::timer::ticks();
                loop {
                    if crate::task::signal::is_sigint_pending_for_current() {
                        return Err(-4); // -EINTR
                    }

                    crate::net::poll();

                    {
                        let mut guard = crate::net::NET_STACK.lock();
                        if let Some(stack) = guard.as_mut() {
                            if !stack.sockets.iter().any(|(h, _)| h == handle) {
                                return Err(-32); // -EPIPE
                            }
                            let sock = stack.sockets.get_mut::<TcpSocket>(handle);
                            if sock.can_send() {
                                match sock.send_slice(buf) {
                                    Ok(n) => {
                                        drop(guard);
                                        crate::net::poll();
                                        return Ok(n);
                                    }
                                    Err(_) => return Err(-32), // -EPIPE
                                }
                            }
                            if !sock.may_send() || sock.state() == TcpState::Closed {
                                return Err(-32); // -EPIPE
                            }
                        } else {
                            return Err(-107); // -ENOTCONN
                        }
                    }

                    if (self.flags.load(Ordering::Relaxed) & O_NONBLOCK) != 0 {
                        return Err(-11); // -EAGAIN
                    }

                    // Таймаут на отправку данных TCP (1000 тиков = ~10 секунд)
                    if crate::arch::x86_64::timer::ticks().saturating_sub(start_tick) > 1000 {
                        return Err(-110); // -ETIMEDOUT
                    }

                    scheduler::yield_now();
                }
            }
            SmolSocketKind::Udp(handle) => {
                let peer = *self.peer_endpoint.lock();
                let dest = match peer {
                    Some(ep) => ep,
                    None => return Err(-89), // -EDESTADDRREQ
                };

                let mut guard = crate::net::NET_STACK.lock();
                if let Some(stack) = guard.as_mut() {
                    let sock = stack.sockets.get_mut::<UdpSocket>(handle);
                    if !sock.is_open() {
                        let local_port = alloc_ephemeral_port();
                        let _ = sock.bind(local_port);
                    }
                    if sock.can_send() {
                        match sock.send_slice(buf, dest) {
                            Ok(()) => {
                                drop(guard);
                                crate::net::poll();
                                return Ok(buf.len());
                            }
                            Err(_) => return Err(-5),
                        }
                    }
                }
                Err(-11)
            }
            SmolSocketKind::Icmp(handle) => {
                let peer = *self.peer_endpoint.lock();
                let dest_addr = match peer {
                    Some(ep) => ep.addr,
                    None => return Err(-89), // -EDESTADDRREQ
                };

                let mut guard = crate::net::NET_STACK.lock();
                if let Some(stack) = guard.as_mut() {
                    let sock = stack.sockets.get_mut::<IcmpSocket>(handle);
                    if !sock.is_open() {
                        // Извлекаем идентификатор Echo (ident) из заголовка ICMP (байты 4..6),
                        // чтобы smoltcp фильтровал и принимал входящие ответы именно с этим ident
                        let ident = if buf.len() >= 6 {
                            u16::from_be_bytes([buf[4], buf[5]])
                        } else {
                            0x0001
                        };
                        let _ = sock.bind(IcmpEndpoint::Ident(ident));
                    }
                    if sock.can_send() {
                        match sock.send_slice(buf, dest_addr) {
                            Ok(()) => {
                                drop(guard);
                                crate::net::poll();
                                return Ok(buf.len());
                            }
                            Err(_) => return Err(-5),
                        }
                    }
                }
                Err(-11)
            }
        }
    }

    fn poll_events(&self) -> u32 {
        let kind = *self.smol_kind.lock();
        match kind {
            SmolSocketKind::Unix => {
                let mut ev = EPOLLOUT;
                if !self.unix_buffer.lock().is_empty() {
                    ev |= EPOLLIN;
                }
                ev
            }
            SmolSocketKind::Tcp(handle) => {
                crate::net::poll();
                let mut guard = crate::net::NET_STACK.lock();
                if let Some(stack) = guard.as_mut() {
                    if !stack.sockets.iter().any(|(h, _)| h == handle) {
                        return EPOLLHUP | EPOLLERR;
                    }
                    let sock = stack.sockets.get_mut::<TcpSocket>(handle);
                    let mut ev = 0;
                    let is_listening = self.is_listening.load(Ordering::Relaxed);
                    if is_listening {
                        if sock.state() == TcpState::Established {
                            ev |= EPOLLIN;
                        }
                    } else {
                        if sock.can_recv() {
                            ev |= EPOLLIN;
                        }
                        if sock.can_send() {
                            ev |= EPOLLOUT;
                        }
                    }
                    if sock.state() == TcpState::CloseWait || sock.state() == TcpState::Closed {
                        ev |= EPOLLHUP | EPOLLRDHUP;
                    }
                    ev
                } else {
                    0
                }
            }
            SmolSocketKind::Udp(handle) => {
                crate::net::poll();
                let mut guard = crate::net::NET_STACK.lock();
                if let Some(stack) = guard.as_mut() {
                    let sock = stack.sockets.get_mut::<UdpSocket>(handle);
                    let mut ev = 0;
                    if sock.can_recv() {
                        ev |= EPOLLIN;
                    }
                    if sock.can_send() {
                        ev |= EPOLLOUT;
                    }
                    ev
                } else {
                    0
                }
            }
            SmolSocketKind::Icmp(handle) => {
                crate::net::poll();
                let mut guard = crate::net::NET_STACK.lock();
                if let Some(stack) = guard.as_mut() {
                    let sock = stack.sockets.get_mut::<IcmpSocket>(handle);
                    let mut ev = 0;
                    if sock.can_recv() {
                        ev |= EPOLLIN;
                    }
                    if sock.can_send() {
                        ev |= EPOLLOUT;
                    }
                    ev
                } else {
                    0
                }
            }
        }
    }

    fn get_flags(&self) -> u32 {
        self.flags.load(Ordering::Relaxed)
    }

    fn set_flags(&self, flags: u32) {
        self.flags.store(flags, Ordering::Relaxed);
    }

    fn close(&self) {
        let mut kind_guard = self.smol_kind.lock();
        let kind = *kind_guard;
        *kind_guard = SmolSocketKind::Unix;
        let mut guard = crate::net::NET_STACK.lock();
        if let Some(stack) = guard.as_mut() {
            match kind {
                SmolSocketKind::Tcp(handle) => {
                    if stack.sockets.iter().any(|(h, _)| h == handle) {
                        let sock = stack.sockets.get_mut::<TcpSocket>(handle);
                        sock.abort();
                        stack.sockets.remove(handle);
                    }
                }
                SmolSocketKind::Udp(handle) => {
                    if stack.sockets.iter().any(|(h, _)| h == handle) {
                        stack.sockets.remove(handle);
                    }
                }
                SmolSocketKind::Icmp(handle) => {
                    if stack.sockets.iter().any(|(h, _)| h == handle) {
                        stack.sockets.remove(handle);
                    }
                }
                SmolSocketKind::Unix => {}
            }
        }
    }

    fn socket_connect(&self, addr: &SockAddrIn) -> Result<(), i64> {
        let endpoint = addr.to_endpoint();
        *self.peer_endpoint.lock() = Some(endpoint);

        let kind = *self.smol_kind.lock();
        match kind {
            SmolSocketKind::Tcp(handle) => {
                let local_port = alloc_ephemeral_port();
                *self.bound_endpoint.lock() = Some(IpEndpoint::new(IpAddress::v4(0, 0, 0, 0), local_port));

                {
                    let mut guard = crate::net::NET_STACK.lock();
                    let stack = guard.as_mut().ok_or(-107i64)?;
                    let sock = stack.sockets.get_mut::<TcpSocket>(handle);
                    let cx = stack.iface.context();
                    sock.connect(cx, endpoint, local_port).map_err(|e| {
                        crate::serial_println!("net: tcp connect error: {:?}", e);
                        -111i64 // -ECONNREFUSED
                    })?;
                }

                // Отправляем SYN-пакет через виртуальную сеть
                crate::net::poll();

                let is_nonblock = (self.flags.load(Ordering::Relaxed) & O_NONBLOCK) != 0;
                if is_nonblock {
                    return Err(-115); // -EINPROGRESS
                }

                // Блокирующее ожидание завершения 3-way handshake (SYN -> SYN/ACK -> ACK)
                let start_tick = crate::arch::x86_64::timer::ticks();
                loop {
                    if crate::task::signal::is_sigint_pending_for_current() {
                        return Err(-4); // -EINTR
                    }

                    crate::net::poll();

                    {
                        let mut guard = crate::net::NET_STACK.lock();
                        if let Some(stack) = guard.as_mut() {
                            if !stack.sockets.iter().any(|(h, _)| h == handle) {
                                return Err(-107); // -ENOTCONN
                            }
                            let sock = stack.sockets.get_mut::<TcpSocket>(handle);
                            match sock.state() {
                                TcpState::Established => return Ok(()),
                                TcpState::Closed | TcpState::TimeWait => return Err(-111), // -ECONNREFUSED
                                _ => {}
                            }
                        }
                    }

                    // Таймаут на подключение (300 тиков = ~3 секунды в QEMU)
                    if crate::arch::x86_64::timer::ticks().saturating_sub(start_tick) > 300 {
                        return Err(-110); // -ETIMEDOUT
                    }

                    scheduler::yield_now();
                }
            }
            SmolSocketKind::Udp(_) | SmolSocketKind::Icmp(_) => {
                // Для UDP/ICMP вызов connect просто фиксирует адрес назначения по умолчанию
                Ok(())
            }
            SmolSocketKind::Unix => Ok(()),
        }
    }

    fn socket_bind(&self, addr: &SockAddrIn) -> Result<(), i64> {
        let endpoint = addr.to_endpoint();
        *self.bound_endpoint.lock() = Some(endpoint);

        let kind = *self.smol_kind.lock();
        match kind {
            SmolSocketKind::Tcp(handle) => {
                let mut guard = crate::net::NET_STACK.lock();
                let stack = guard.as_mut().ok_or(-107i64)?;
                let sock = stack.sockets.get_mut::<TcpSocket>(handle);
                let listen_ep = IpListenEndpoint {
                    addr: if endpoint.addr.is_unspecified() { None } else { Some(endpoint.addr) },
                    port: endpoint.port,
                };
                sock.listen(listen_ep).map_err(|_| -98i64)?; // -EADDRINUSE
                Ok(())
            }
            SmolSocketKind::Udp(handle) => {
                let mut guard = crate::net::NET_STACK.lock();
                let stack = guard.as_mut().ok_or(-107i64)?;
                let sock = stack.sockets.get_mut::<UdpSocket>(handle);
                let listen_ep = IpListenEndpoint {
                    addr: if endpoint.addr.is_unspecified() { None } else { Some(endpoint.addr) },
                    port: endpoint.port,
                };
                sock.bind(listen_ep).map_err(|_| -98i64)?;
                Ok(())
            }
            SmolSocketKind::Icmp(handle) => {
                let mut guard = crate::net::NET_STACK.lock();
                let stack = guard.as_mut().ok_or(-107i64)?;
                let sock = stack.sockets.get_mut::<IcmpSocket>(handle);
                sock.bind(IcmpEndpoint::Ident(endpoint.port)).map_err(|_| -98i64)?;
                Ok(())
            }
            SmolSocketKind::Unix => Ok(()),
        }
    }

    fn socket_listen(&self, _backlog: i32) -> Result<(), i64> {
        let kind = *self.smol_kind.lock();
        match kind {
            SmolSocketKind::Tcp(handle) => {
                let bound = *self.bound_endpoint.lock();
                let port = match bound {
                    Some(ep) => ep.port,
                    None => return Err(-22), // -EINVAL
                };

                let mut guard = crate::net::NET_STACK.lock();
                let stack = guard.as_mut().ok_or(-107i64)?;
                let sock = stack.sockets.get_mut::<TcpSocket>(handle);
                if !sock.is_listening() {
                    sock.listen(port).map_err(|_| -98i64)?;
                }
                self.is_listening.store(true, Ordering::SeqCst);
                Ok(())
            }
            _ => Err(-95), // -EOPNOTSUPP
        }
    }

    fn socket_accept(&self, flags: u32) -> Result<(u64, SockAddrIn), i64> {
        let kind = *self.smol_kind.lock();
        match kind {
            SmolSocketKind::Tcp(handle) => {
                let bound_port = self.bound_endpoint.lock().map(|ep| ep.port).unwrap_or(0);
                if bound_port == 0 {
                    return Err(-22); // -EINVAL
                }

                let start_tick = crate::arch::x86_64::timer::ticks();
                loop {
                    if crate::task::signal::is_sigint_pending_for_current() {
                        return Err(-4); // -EINTR
                    }

                    crate::net::poll();

                    {
                        let mut guard = crate::net::NET_STACK.lock();
                        if let Some(stack) = guard.as_mut() {
                            if !stack.sockets.iter().any(|(h, _)| h == handle) {
                                return Err(-22); // -EINVAL
                            }
                            let sock = stack.sockets.get_mut::<TcpSocket>(handle);
                            if sock.state() == TcpState::Established {
                                let remote_ep = sock.remote_endpoint().unwrap_or(IpEndpoint::new(IpAddress::v4(0, 0, 0, 0), 0));
                                let local_ep = sock.local_endpoint().unwrap_or(IpEndpoint::new(IpAddress::v4(0, 0, 0, 0), bound_port));

                                // Создаем новый TCP-сокет для продолжения прослушивания входящих подключений
                                let new_rx = TcpSocketBuffer::new(alloc::vec![0u8; 16384]);
                                let new_tx = TcpSocketBuffer::new(alloc::vec![0u8; 16384]);
                                let mut new_tcp = TcpSocket::new(new_rx, new_tx);
                                let _ = new_tcp.listen(bound_port);
                                let new_handle = stack.sockets.add(new_tcp);

                                // Заменяем внутренний сокет слушателя на новый
                                *self.smol_kind.lock() = SmolSocketKind::Tcp(new_handle);

                                // Создаем дескриптор для установленного соединения
                                let client_sock = Arc::new(SocketFileHandle::new(
                                    AF_INET,
                                    SOCK_STREAM,
                                    IPPROTO_TCP,
                                    flags,
                                    SmolSocketKind::Tcp(handle),
                                ));
                                *client_sock.bound_endpoint.lock() = Some(local_ep);
                                *client_sock.peer_endpoint.lock() = Some(remote_ep);

                                let new_fd = crate::fs::insert_file_handle(client_sock);
                                return Ok((new_fd, SockAddrIn::from_endpoint(remote_ep)));
                            }
                        }
                    }

                    if (self.flags.load(Ordering::Relaxed) & O_NONBLOCK) != 0 || (flags & SOCK_NONBLOCK) != 0 {
                        return Err(-11); // -EAGAIN
                    }

                    if crate::arch::x86_64::timer::ticks().saturating_sub(start_tick) > 3000 {
                        return Err(-11);
                    }

                    scheduler::yield_now();
                }
            }
            _ => Err(-95),
        }
    }

    fn socket_sendto(&self, buf: &[u8], _flags: i32, dest: Option<&SockAddrIn>) -> Result<usize, i64> {
        if let Some(dest_addr) = dest {
            *self.peer_endpoint.lock() = Some(dest_addr.to_endpoint());
        }
        self.write(buf)
    }

    fn socket_recvfrom(&self, buf: &mut [u8], _flags: i32) -> Result<(usize, SockAddrIn), i64> {
        let n = self.read(buf)?;
        let peer = self.peer_endpoint.lock().map(SockAddrIn::from_endpoint).unwrap_or_default();
        Ok((n, peer))
    }

    fn socket_getsockname(&self) -> Result<SockAddrIn, i64> {
        let ep = self.bound_endpoint.lock().unwrap_or(IpEndpoint::new(IpAddress::v4(0, 0, 0, 0), 0));
        Ok(SockAddrIn::from_endpoint(ep))
    }

    fn socket_getpeername(&self) -> Result<SockAddrIn, i64> {
        let ep = self.peer_endpoint.lock().ok_or(-107i64)?;
        Ok(SockAddrIn::from_endpoint(ep))
    }

    fn socket_shutdown(&self, _how: i32) -> Result<(), i64> {
        let kind = *self.smol_kind.lock();
        if let SmolSocketKind::Tcp(handle) = kind {
            let mut guard = crate::net::NET_STACK.lock();
            if let Some(stack) = guard.as_mut() {
                let sock = stack.sockets.get_mut::<TcpSocket>(handle);
                sock.close();
            }
        }
        Ok(())
    }

    fn socket_setsockopt(&self, _level: i32, _optname: i32, _optval: &[u8]) -> Result<(), i64> {
        // Базовые сокетные флаги SOL_SOCKET / IPPROTO_TCP успешно принимаются ядром
        Ok(())
    }

    fn socket_getsockopt(&self, level: i32, optname: i32) -> Result<alloc::vec::Vec<u8>, i64> {
        // level 0 = IPPROTO_IP:
        // optname 4 = IP_OPTIONS: опции маршрутизации отсутствуют. Возвращаем пустой срез (длина 0),
        // чтобы OpenSSH check_ip_options не считал, что в пакете переданы source routing options (что вызывает fatal drop)
        if level == 0 && optname == 4 {
            return Ok(alloc::vec![]);
        }
        // Для SOL_SOCKET (1) с SO_ERROR (4) или опций буфера возвращаем нулевой статус (0 ошибок)
        Ok(alloc::vec![0u8; 4])
    }
}

impl Drop for SocketFileHandle {
    fn drop(&mut self) {
        // Очищаем низкоуровневый сокет smoltcp только когда уничтожается
        // последняя сильная ссылка Arc на данный дескриптор в системе (RAII).
        self.close();
    }
}

/// Создает новый сетевой сокет заданного семейства и типа
pub fn create_socket(domain: u16, sock_type: u32, protocol: i32) -> Result<Arc<dyn FileHandle>, i64> {
    if domain != AF_UNIX && domain != AF_INET {
        return Err(-97); // -EAFNOSUPPORT
    }

    let actual_type = sock_type & !(SOCK_NONBLOCK | SOCK_CLOEXEC);
    let flags = if (sock_type & SOCK_NONBLOCK) != 0 {
        O_NONBLOCK
    } else {
        0
    };

    if domain == AF_UNIX {
        return Ok(Arc::new(SocketFileHandle::new(domain, actual_type, protocol, flags, SmolSocketKind::Unix)));
    }

    let mut guard = crate::net::NET_STACK.lock();
    let stack = guard.as_mut().ok_or(-107i64)?; // -ENOTCONN (сеть не готова)

    match actual_type {
        SOCK_STREAM => {
            let rx = TcpSocketBuffer::new(alloc::vec![0u8; 16384]);
            let tx = TcpSocketBuffer::new(alloc::vec![0u8; 16384]);
            let tcp = TcpSocket::new(rx, tx);
            let handle = stack.sockets.add(tcp);
            Ok(Arc::new(SocketFileHandle::new(domain, actual_type, protocol, flags, SmolSocketKind::Tcp(handle))))
        }
        SOCK_DGRAM => {
            let rx = UdpPacketBuffer::new(alloc::vec![UdpPacketMetadata::EMPTY; 16], alloc::vec![0u8; 16384]);
            let tx = UdpPacketBuffer::new(alloc::vec![UdpPacketMetadata::EMPTY; 16], alloc::vec![0u8; 16384]);
            let udp = UdpSocket::new(rx, tx);
            let handle = stack.sockets.add(udp);
            Ok(Arc::new(SocketFileHandle::new(domain, actual_type, protocol, flags, SmolSocketKind::Udp(handle))))
        }
        SOCK_RAW => {
            let rx = IcmpPacketBuffer::new(alloc::vec![IcmpPacketMetadata::EMPTY; 16], alloc::vec![0u8; 4096]);
            let tx = IcmpPacketBuffer::new(alloc::vec![IcmpPacketMetadata::EMPTY; 16], alloc::vec![0u8; 4096]);
            let icmp = IcmpSocket::new(rx, tx);
            let handle = stack.sockets.add(icmp);
            Ok(Arc::new(SocketFileHandle::new(domain, actual_type, protocol, flags, SmolSocketKind::Icmp(handle))))
        }
        _ => Err(-94), // -ESOCKTNOSUPPORT
    }
}
