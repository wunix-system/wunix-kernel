use alloc::string::String;

use crate::fs::block;
use crate::fs::mount;
use crate::fs::ops::write_file_ramfs;

pub fn pci_vendor_name(vendor: u16) -> &'static str {
    match vendor {
        0x8086 => "Intel Corporation",
        0x1AF4 => "Red Hat, Inc.",
        0x80EE => "Oracle VM VirtualBox",
        0x15AD => "VMware, Inc.",
        0x1414 => "Microsoft Corporation",
        0x5853 => "Citrix Systems, Inc. / Xen",
        0x1234 => "Bochs/QEMU",
        0x1013 => "Cirrus Logic",
        0x10EC => "Realtek Semiconductor",
        _ => "Unknown Vendor",
    }
}

pub fn pci_device_name(vendor: u16, device: u16) -> &'static str {
    match (vendor, device) {
        (0x8086, 0x1237) => "440FX - 82441FX PMC [Natoma]",
        (0x8086, 0x7000) => "82371SB PIIX3 ISA [Natoma/Triton II]",
        (0x8086, 0x7010) => "82371SB PIIX3 IDE [Natoma/Triton II]",
        (0x8086, 0x7113) => "82371AB/EB/MB PIIX4 ACPI",
        (0x8086, 0x100E) => "82540EM Gigabit Ethernet Controller",
        (0x8086, 0x100F) => "82545EM Gigabit Ethernet Controller",
        (0x8086, 0x1004) => "82543GC Gigabit Ethernet Controller",
        (0x8086, 0x10D3) => "82574L Gigabit Network Connection",
        (0x80EE, 0xCAFE) => "VirtualBox Guest Integration (VMMDev)",
        (0x80EE, 0xBEEF) => "VirtualBox Graphics Adapter (VBoxSVGA)",
        (0x15AD, 0x0405) => "VMware SVGA II Adapter",
        (0x15AD, 0x07B0) => "VMware vmxnet3 Ethernet Controller",
        (0x15AD, 0x07C0) => "VMware PVSCSI Storage Controller",
        (0x1414, 0x5353) => "Microsoft Hyper-V Synthetic Video",
        (0x5853, 0x0001) => "Xen Platform PCI Device",
        (0x1AF4, 0x1000) | (0x1AF4, 0x1041) => "Virtio Network Device",
        (0x1AF4, 0x1001) | (0x1AF4, 0x1042) => "Virtio Block Device",
        (0x1AF4, 0x1002) | (0x1AF4, 0x1043) => "Virtio Memory Balloon",
        (0x1AF4, 0x1003) | (0x1AF4, 0x1044) => "Virtio Console",
        (0x1AF4, 0x1004) | (0x1AF4, 0x1045) => "Virtio SCSI Host",
        (0x1AF4, 0x1048) | (0x1AF4, 0x1050) => "Virtio GPU (Graphics)",
        (0x1AF4, 0x1052) => "Virtio Input Device",
        (0x1234, 0x1111) => "VGA / VBE Compatible Graphics Adapter",
        (0x1013, 0x00B8) => "CL-GD5446 VisualMedia Accelerator",
        (0x10EC, 0x8139) => "RTL-8139 Fast Ethernet NIC",
        (0x10EC, 0x8125) => "RTL8125 2.5GbE Ethernet Controller",
        (0x8086, 0x10FB) | (0x8086, 0x1528) | (0x8086, 0x1563) => "Intel 10-Gigabit Ethernet Controller (ixgbe)",
        (0x8086, 0x2723) | (0x8086, 0x06F0) | (0x8086, 0x2725) => "Intel Wi-Fi 6/6E Wireless Network Adapter",
        (0x14E4, 0x1600) | (0x14E4, 0x1659) | (0x14E4, 0x165A) | (0x14E4, 0x165F) => {
            "Broadcom NetXtreme Gigabit Ethernet (tg3)"
        }
        (0x10EC, 0x8168) | (0x10EC, 0x8111) | (0x10EC, 0x8136) | (0x10EC, 0x8169) => {
            "RTL8168/8111 PCIe Gigabit Ethernet"
        }
        (0x8086, 0x2415) | (0x8086, 0x2445) | (0x8086, 0x7195) => "82801AA AC'97 Audio",
        (0x8086, 0x2668) | (0x8086, 0x27D8) | (0x8086, 0x284B) | (0x8086, 0x293E) | (0x8086, 0x3B56) => {
            "Intel High Definition Audio Controller"
        }
        (0x1B36, 0x000D) => "QEMU USB 3.0 xHCI Controller",
        (0x1B36, 0x0010) => "NVM Express Controller",
        (0x8086, 0x265C) | (0x8086, 0x293A) => "Intel USB 2.0 EHCI Controller",
        (0x8086, 0x7020) | (0x8086, 0x7112) => "Intel USB 1.1 UHCI Controller",
        _ => "Generic Device",
    }
}

pub fn pci_driver_name(vendor: u16, device: u16) -> &'static str {
    match (vendor, device) {
        (0x8086, 0x100E) | (0x8086, 0x100F) | (0x8086, 0x1004) | (0x8086, 0x10D3) => "e1000",
        (0x10EC, 0x8139) => "rtl8139",
        (0x10EC, 0x8168) | (0x10EC, 0x8111) | (0x10EC, 0x8136) | (0x10EC, 0x8169) => "rtl8168",
        (0x8086, 0x2415) | (0x8086, 0x2445) | (0x8086, 0x7195) => "snd_ac97",
        (0x8086, 0x2668) | (0x8086, 0x27D8) | (0x8086, 0x284B) | (0x8086, 0x293E) | (0x8086, 0x3B56) => {
            "snd_hda"
        }
        (0x1B36, 0x000D) => "xhci",
        (0x1B36, 0x0010) => "nvme",
        (0x8086, 0x265C) | (0x8086, 0x293A) => "ehci",
        (0x8086, 0x7020) | (0x8086, 0x7112) => "uhci",
        (0x80EE, 0xCAFE) => "vboxguest",
        (0x80EE, 0xBEEF) => "vboxvideo",
        (0x15AD, 0x0405) => "vmw_svga",
        (0x15AD, 0x07B0) => "vmxnet3",
        (0x15AD, 0x07C0) => "pvscsi",
        (0x1414, 0x5353) => "hyperv_fb",
        (0x5853, 0x0001) => "xen-platform",
        (0x1AF4, 0x1000) | (0x1AF4, 0x1041) => "virtio-net",
        (0x1AF4, 0x1001) | (0x1AF4, 0x1042) => "virtio-blk",
        (0x1AF4, 0x1048) | (0x1AF4, 0x1050) => "virtio-gpu",
        (0x1AF4, 0x1052) => "virtio-input",
        (0x8086, 0x1237) => "pci-host",
        (0x8086, 0x7000) => "isa-bridge",
        (0x8086, 0x7113) => "piix4-acpi",
        (0x1234, 0x1111) => "bochs-vga",
        _ => "pci",
    }
}

pub fn refresh_proc_files(path: &str) {
    if path == "/proc/mounts" || path == "/etc/mtab" {
        let mut text = String::from("rootfs / rootfs rw 0 0\n");
        for m in mount::list_mounts() {
            text.push_str(&alloc::format!("{} {} {} rw,relatime 0 0\n", m.source, m.target, m.fstype));
        }
        let _ = write_file_ramfs("/proc/mounts", text.as_bytes());
    } else if path == "/proc/partitions" {
        let mut text = String::from("major minor  #blocks  name\n");
        for (i, (name, count, size)) in block::list_block_devices().iter().enumerate() {
            let blocks = (count * *size as u64) / 1024;
            text.push_str(&alloc::format!(" 254 {:5} {:8} {}\n", i, blocks, name));
        }
        let _ = write_file_ramfs("/proc/partitions", text.as_bytes());
    } else if path == "/proc/net/dev" || path.starts_with("/sys/class/net") {
        let mut text = String::from(
            "Inter-|   Receive                                                |  Transmit\n \
             face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed\n"
        );
        // Интерфейс Loopback
        text.push_str("    lo:       0       0    0    0    0     0          0         0        0       0    0    0    0     0       0          0\n");
        let _ = write_file_ramfs("/sys/class/net/lo/address", b"00:00:00:00:00:00\n");
        let _ = write_file_ramfs("/sys/class/net/lo/mtu", b"65536\n");
        let _ = write_file_ramfs("/sys/class/net/lo/operstate", b"unknown\n");
        let _ = write_file_ramfs("/sys/class/net/lo/flags", b"0x49\n");
        let _ = write_file_ramfs("/sys/class/net/lo/ipv4_address", b"127.0.0.1\n");
        let _ = write_file_ramfs("/sys/class/net/lo/ipv4_netmask", b"255.0.0.0\n");
        let _ = write_file_ramfs("/sys/class/net/lo/ipv4_prefix", b"8\n");
        let _ = write_file_ramfs("/sys/class/net/lo/ipv4_broadcast", b"127.255.255.255\n");

        if let Some(info) = crate::net::get_info() {
            let (rx_b, rx_p, tx_b, tx_p) = crate::drivers::with_device_manager(|dm| {
                if let Some(nic) = dm.get_net_device(0) {
                    let dev = nic.lock();
                    (dev.rx_bytes(), dev.rx_packets(), dev.tx_bytes(), dev.tx_packets())
                } else {
                    (0, 0, 0, 0)
                }
            }).unwrap_or((0, 0, 0, 0));
            text.push_str(&alloc::format!(
                "  eth0: {:7} {:7}    0    0    0     0          0         0  {:7} {:7}    0    0    0     0       0          0\n",
                rx_b, rx_p, tx_b, tx_p
            ));
            let mac_str = alloc::format!(
                "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}\n",
                info.mac[0], info.mac[1], info.mac[2], info.mac[3], info.mac[4], info.mac[5]
            );
            let _ = write_file_ramfs("/sys/class/net/eth0/address", mac_str.as_bytes());
            let _ = write_file_ramfs("/sys/class/net/eth0/mtu", b"1500\n");
            let _ = write_file_ramfs("/sys/class/net/eth0/operstate", b"up\n");
            let _ = write_file_ramfs("/sys/class/net/eth0/flags", b"0x1043\n");
            let ip_str = alloc::format!("{}.{}.{}.{}\n", info.ip[0], info.ip[1], info.ip[2], info.ip[3]);
            let _ = write_file_ramfs("/sys/class/net/eth0/ipv4_address", ip_str.as_bytes());
            let mask_str = alloc::format!("{}.{}.{}.{}\n", info.netmask[0], info.netmask[1], info.netmask[2], info.netmask[3]);
            let _ = write_file_ramfs("/sys/class/net/eth0/ipv4_netmask", mask_str.as_bytes());
            let prefix = info.netmask.iter().map(|&b| b.count_ones()).sum::<u32>();
            let _ = write_file_ramfs("/sys/class/net/eth0/ipv4_prefix", alloc::format!("{}\n", prefix).as_bytes());
            let bcast_str = alloc::format!(
                "{}.{}.{}.{}\n",
                info.ip[0] | !info.netmask[0],
                info.ip[1] | !info.netmask[1],
                info.ip[2] | !info.netmask[2],
                info.ip[3] | !info.netmask[3],
            );
            let _ = write_file_ramfs("/sys/class/net/eth0/ipv4_broadcast", bcast_str.as_bytes());
            let gw_str = alloc::format!("{}.{}.{}.{}\n", info.gateway[0], info.gateway[1], info.gateway[2], info.gateway[3]);
            let _ = write_file_ramfs("/sys/class/net/eth0/ipv4_gateway", gw_str.as_bytes());
            let dns_str = alloc::format!("{}.{}.{}.{}\n", info.dns[0], info.dns[1], info.dns[2], info.dns[3]);
            let _ = write_file_ramfs("/sys/class/net/eth0/ipv4_dns", dns_str.as_bytes());
            let resolv_content = alloc::format!(
                "# /etc/resolv.conf — Конфигурация DNS-резолвера Wunix\n\
                 # Адрес виртуального DNS-сервера (QEMU SLIRP / локальная сеть)\n\
                 nameserver {}\n",
                dns_str
            );
            let _ = write_file_ramfs("/etc/resolv.conf", resolv_content.as_bytes());
        }
        let _ = write_file_ramfs("/proc/net/dev", text.as_bytes());
    } else if path == "/proc/net/route" || path == "/proc/net/routes" {
        let mut text = String::from("Iface\tDestination\tGateway\tFlags\tRefCnt\tUse\tMetric\tMask\tMTU\tWindow\tIRTT\n");
        if let Some(info) = crate::net::get_info() {
            // Default gateway route (%08X little-endian в стиле ядра Linux)
            text.push_str(&alloc::format!(
                "eth0\t00000000\t{:02X}{:02X}{:02X}{:02X}\t0003\t0\t0\t0\t00000000\t0\t0\t0\n",
                info.gateway[3], info.gateway[2], info.gateway[1], info.gateway[0]
            ));
            // Subnet link route (%08X little-endian в стиле ядра Linux)
            let sub0 = info.ip[0] & info.netmask[0];
            let sub1 = info.ip[1] & info.netmask[1];
            let sub2 = info.ip[2] & info.netmask[2];
            let sub3 = info.ip[3] & info.netmask[3];
            text.push_str(&alloc::format!(
                "eth0\t{:02X}{:02X}{:02X}{:02X}\t00000000\t0001\t0\t0\t0\t{:02X}{:02X}{:02X}{:02X}\t0\t0\t0\n",
                sub3, sub2, sub1, sub0,
                info.netmask[3], info.netmask[2], info.netmask[1], info.netmask[0]
            ));
        }
        let _ = write_file_ramfs("/proc/net/route", text.as_bytes());
    } else if path == "/proc/net/tcp" || path == "/proc/net/udp" {
        let mut text = String::from("  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n");
        let entries = crate::net::dump_sockets();
        for (i, e) in entries.iter().enumerate() {
            if path == "/proc/net/tcp" && e.proto != "tcp" { continue; }
            if path == "/proc/net/udp" && e.proto != "udp" { continue; }
            let state_code = match e.state {
                "ESTAB" => 1,
                "SYN_SENT" => 2,
                "SYN_RECV" => 3,
                "FIN_WAIT1" => 4,
                "FIN_WAIT2" => 5,
                "TIME_WAIT" => 6,
                "CLOSED" => 7,
                "CLOSE_WAIT" => 8,
                "LAST_ACK" => 9,
                "LISTEN" => 10,
                "CLOSING" => 11,
                _ => 1,
            };
            text.push_str(&alloc::format!(
                "{:4}: {:02X}{:02X}{:02X}{:02X}:{:04X} {:02X}{:02X}{:02X}{:02X}:{:04X} {:02X} {:08X}:{:08X} 00:00000000 00000000     0        0 {}\n",
                i,
                e.local_ip[3], e.local_ip[2], e.local_ip[1], e.local_ip[0], e.local_port,
                e.remote_ip[3], e.remote_ip[2], e.remote_ip[1], e.remote_ip[0], e.remote_port,
                state_code,
                e.tx_queue, e.rx_queue,
                1000 + i
            ));
        }
        let _ = write_file_ramfs(path, text.as_bytes());
    } else if path == "/proc/bus/pci/devices" || path == "/proc/pci" || path == "/sys/bus/pci/devices" {
        let mut raw_table = String::new();
        let mut human_table = String::from("PCI devices found:\n");

        crate::drivers::with_device_manager(|dm| {
            for dev in dm.pci_devices() {
                let dfn = ((dev.address.bus as u32) << 8)
                    | (((dev.address.device & 0x1F) as u32) << 3)
                    | ((dev.address.function & 0x07) as u32);
                let vend_dev = ((dev.vendor_id as u32) << 16) | (dev.device_id as u32);
                let drv = dev.driver_name.unwrap_or_else(|| pci_driver_name(dev.vendor_id, dev.device_id));

                let mut b_addrs = [0u64; 6];
                let mut b_sizes = [0u64; 6];
                for i in 0..6 {
                    if let Some(bar) = dev.bars[i] {
                        b_addrs[i] = bar.address;
                        b_sizes[i] = bar.size;
                    }
                }

                raw_table.push_str(&alloc::format!(
                    "{:04x}\t{:08x}\t{:x}\t{:x}\t{:x}\t{:x}\t{:x}\t{:x}\t{:x}\t{:x}\t{:x}\t{:x}\t{:x}\t{:x}\t{:x}\t{}\t{:02x}{:02x}\n",
                    dfn, vend_dev, dev.interrupt_line,
                    b_addrs[0], b_addrs[1], b_addrs[2], b_addrs[3], b_addrs[4], b_addrs[5],
                    b_sizes[0], b_sizes[1], b_sizes[2], b_sizes[3], b_sizes[4], b_sizes[5],
                    drv,
                    dev.class_code, dev.subclass
                ));

                let class_name = crate::drivers::pci::PciClass::name(dev.class_code, dev.subclass);
                let vend_name = pci_vendor_name(dev.vendor_id);
                let dev_name = pci_device_name(dev.vendor_id, dev.device_id);

                human_table.push_str(&alloc::format!(
                    "  Bus {:2}, device {:2}, function {}:\n    {}: {} {} (rev {:02x})\n      Vendor: 0x{:04x}, Device: 0x{:04x}, IRQ: {}\n",
                    dev.address.bus, dev.address.device, dev.address.function,
                    class_name, vend_name, dev_name, dev.revision_id,
                    dev.vendor_id, dev.device_id, dev.interrupt_line
                ));
            }
        });

        let _ = write_file_ramfs("/proc/bus/pci/devices", raw_table.as_bytes());
        let _ = write_file_ramfs("/sys/bus/pci/devices", raw_table.as_bytes());
        let _ = write_file_ramfs("/proc/pci", human_table.as_bytes());
    } else if path == "/proc/modules" {
        let mut text = String::new();
        for drv in crate::drivers::list_drivers() {
            text.push_str(&alloc::format!(
                "{} {} {} {} {} 0x0\n",
                drv.name,
                drv.size,
                drv.devices_count,
                drv.dependencies,
                drv.status
            ));
        }
        let _ = write_file_ramfs("/proc/modules", text.as_bytes());
    } else if path == "/proc/devices" {
        let text = "Character devices:\n \
             1 mem\n \
             4 tty\n \
             4 ttyS\n \
            10 misc\n \
            13 input\n \
            29 fb\n\
           254 rtc\n\n\
           Block devices:\n\
           254 virtblk\n";
        let _ = write_file_ramfs("/proc/devices", text.as_bytes());
    } else if path == "/proc/interrupts" {
        let text = "           CPU0\n \
             0:         timer0 (pit)\n \
             1:         i8042 (keyboard)\n \
             4:         ttyS0 (serial)\n \
             8:         rtc0 (cmos)\n \
            11:         pci-virtio\n";
        let _ = write_file_ramfs("/proc/interrupts", text.as_bytes());
    } else if path == "/proc/ioports" {
        let mut text = String::from(
            "0020-0021 : pic1\n\
             0040-0043 : timer0\n\
             0060-0064 : i8042\n\
             0070-0071 : rtc0\n\
             00a0-00a1 : pic2\n\
             03f8-03ff : serial (ttyS0)\n\
             0cf8-0cff : pci\n"
        );
        crate::drivers::with_device_manager(|dm| {
            for dev in dm.pci_devices() {
                for bar_opt in &dev.bars {
                    if let Some(bar) = bar_opt {
                        if bar.bar_type == crate::drivers::pci::BarType::Io && bar.address > 0 && bar.size > 0 {
                            let end = bar.address + bar.size - 1;
                            let drv = pci_driver_name(dev.vendor_id, dev.device_id);
                            text.push_str(&alloc::format!("{:04x}-{:04x} : {}\n", bar.address, end, drv));
                        }
                    }
                }
            }
        });
        let _ = write_file_ramfs("/proc/ioports", text.as_bytes());
    } else if path == "/proc/iomem" {
        let mut text = String::from(
            "00000000-0009ffff : System RAM\n\
             000a0000-000bffff : Video RAM\n\
             000c0000-000fffff : ROM Option\n\
             00100000-1fffffff : System RAM\n"
        );
        crate::drivers::with_device_manager(|dm| {
            for dev in dm.pci_devices() {
                for bar_opt in &dev.bars {
                    if let Some(bar) = bar_opt {
                        if bar.bar_type == crate::drivers::pci::BarType::Memory32 || bar.bar_type == crate::drivers::pci::BarType::Memory64 {
                            if bar.address > 0 && bar.size > 0 {
                                let end = bar.address + bar.size - 1;
                                let drv = pci_driver_name(dev.vendor_id, dev.device_id);
                                text.push_str(&alloc::format!("{:08x}-{:08x} : {}\n", bar.address, end, drv));
                            }
                        }
                    }
                }
            }
        });
        if let Some(console) = crate::drivers::display::CONSOLE.lock().as_ref() {
            let fb_start = console.framebuffer.address as u64;
            let fb_size = (console.framebuffer.height * console.framebuffer.pitch) as u64;
            if fb_start > 0 && fb_size > 0 {
                text.push_str(&alloc::format!("{:08x}-{:08x} : Framebuffer\n", fb_start, fb_start + fb_size - 1));
            }
        }
        let _ = write_file_ramfs("/proc/iomem", text.as_bytes());

    } else if path == "/proc/cpuinfo" {
        let cpu = crate::arch::x86_64::CpuInfo::current();
        let text = alloc::format!(
            "processor\t: 0\n\
             vendor_id\t: {}\n\
             cpu family\t: 6\n\
             model\t\t: 3\n\
             model name\t: {}\n\
             stepping\t: 3\n\
             cpu MHz\t\t: 2594.123\n\
             cache size\t: 4096 KB\n\
             fpu\t\t: yes\n\
             fpu_exception\t: yes\n\
             cpuid level\t: 13\n\
             wp\t\t: yes\n\
             flags\t\t: fpu vme de pse tsc msr pae mce cx8 apic sep mtrr pge mca cmov pat pse36 clflush mmx fxsr sse sse2 ss syscall nx lm\n\
             bugs\t\t:\n\
             bogomips\t: 5188.24\n\
             clflush size\t: 64\n\
             cache_alignment\t: 64\n\
             address sizes\t: 40 bits physical, 48 bits virtual\n",
            cpu.vendor(),
            cpu.brand()
        );
        let _ = write_file_ramfs("/proc/cpuinfo", text.as_bytes());
    } else if path == "/proc/driver/rtc" {
        let dt = crate::drivers::rtc::read_datetime();
        let text = alloc::format!(
            "rtc_time\t: {:02}:{:02}:{:02}\n\
             rtc_date\t: {:04}-{:02}-{:02}\n\
             rtc_epoch\t: 1970\n\
             alarm\t\t: 00:00:00\n\
             DST_enable\t: no\n\
             periodic_irq\t: no\n\
             update_irq\t: no\n\
             HPET_emulated\t: no\n\
             BCD\t\t: no\n\
             24hr\t\t: yes\n",
            dt.hour, dt.minute, dt.second,
            dt.year, dt.month, dt.day
        );
        let _ = write_file_ramfs("/proc/driver/rtc", text.as_bytes());
    } else if path.starts_with("/sys/class/graphics/fb0") {
        if let Some(console) = crate::drivers::display::CONSOLE.lock().as_ref() {
            let (w, h) = (console.framebuffer.width, console.framebuffer.height);
            let bpp = console.framebuffer.bpp;
            let pitch = console.framebuffer.pitch;
            let addr = console.framebuffer.address as u64;
            let _ = write_file_ramfs("/sys/class/graphics/fb0/modes", alloc::format!("{}x{}-{}\n", w, h, bpp).as_bytes());
            let _ = write_file_ramfs("/sys/class/graphics/fb0/virtual_size", alloc::format!("{},{}\n", w, h).as_bytes());
            let _ = write_file_ramfs("/sys/class/graphics/fb0/stride", alloc::format!("{}\n", pitch).as_bytes());
            let _ = write_file_ramfs("/sys/class/graphics/fb0/bits_per_pixel", alloc::format!("{}\n", bpp).as_bytes());
            let _ = write_file_ramfs("/sys/class/graphics/fb0/address", alloc::format!("0x{:08x}\n", addr).as_bytes());
        }
    } else if path == "/proc/hypervisor" {
        let hv = crate::drivers::virt::current_hypervisor();
        let mut text = alloc::format!(
            "Hypervisor: {}\nType: {}\nStatus: active\n",
            hv.display_name(true),
            hv.name()
        );
        match hv {
            crate::drivers::virt::HypervisorType::VirtualBox => {
                text.push_str("Platform: Oracle VM VirtualBox\nDrivers: vboxguest, vboxvideo, vboxmouse, vboxsf\nFeatures: HGCM, TimeSync, Seamless Pointer, Shared Folders\n");
            }
            crate::drivers::virt::HypervisorType::VMware => {
                text.push_str("Platform: VMware Workstation / ESXi\nDrivers: vmw_svga, vmxnet3, pvscsi, vmmouse\nFeatures: Backdoor I/O, Host TimeSync, Absolute Mouse Pointer\n");
            }
            crate::drivers::virt::HypervisorType::HyperV => {
                text.push_str("Platform: Microsoft Hyper-V / Azure\nDrivers: hyperv_vmbus, hyperv_fb, netvsc, storvsc\nFeatures: VMBus, Heartbeat, Guest Shutdown, KVP Exchange\n");
            }
            crate::drivers::virt::HypervisorType::Kvm | crate::drivers::virt::HypervisorType::Qemu => {
                text.push_str("Platform: KVM / Proxmox VE / QEMU\nDrivers: virtio-blk, virtio-net, virtio-scsi, virtio-gpu, virtio-balloon, virtio-rng, virtio-console, bochs-vga\nFeatures: VirtIO Modern/Legacy, pvclock, fw_cfg\n");
            }
            crate::drivers::virt::HypervisorType::Xen => {
                text.push_str("Platform: Xen Paravirtualization\nDrivers: xen-blkfront, xen-netfront, xen-platform\nFeatures: Hypercalls, Grant Tables v1, Event Channels\n");
            }
            crate::drivers::virt::HypervisorType::BareMetal => {
                text.push_str("Platform: Physical Machine (Bare Metal)\nFeatures: Native Direct Hardware MMIO/Port Access\n");
            }
            _ => {
                text.push_str("Platform: Generic Virtual Machine\n");
            }
        }
        let _ = write_file_ramfs("/proc/hypervisor", text.as_bytes());
    } else if path == "/sys/hypervisor/type" {
        let hv = crate::drivers::virt::current_hypervisor();
        let text = alloc::format!("{}\n", hv.name());
        let _ = write_file_ramfs("/sys/hypervisor/type", text.as_bytes());
    } else if path.starts_with("/sys/class/power_supply") {
        let pwr = crate::drivers::acpi::power::get_power_supply_info();
        let _ = write_file_ramfs("/sys/class/power_supply/AC/online", if pwr.ac_online { b"1\n" } else { b"0\n" });
        let _ = write_file_ramfs("/sys/class/power_supply/AC/type", b"Mains\n");
        let _ = write_file_ramfs("/sys/class/power_supply/BAT0/status", alloc::format!("{}\n", pwr.battery_status).as_bytes());
        let _ = write_file_ramfs("/sys/class/power_supply/BAT0/capacity", alloc::format!("{}\n", pwr.battery_percent).as_bytes());
        let _ = write_file_ramfs("/sys/class/power_supply/BAT0/present", if pwr.battery_present { b"1\n" } else { b"0\n" });
        let _ = write_file_ramfs("/sys/class/power_supply/BAT0/type", b"Battery\n");
    } else if path == "/proc/net/wireless" {
        let text = "Inter-| sta-|   Quality        |   Discarded packets               | Missed | WE\n face | tus | link level noise |  nwid  crypt   frag  retry   misc | beacon | 22\nwlan0: 0000   65.  -45.  -256        0      0      0      0      0        0\n";
        let _ = write_file_ramfs("/proc/net/wireless", text.as_bytes());
    }
}
