use alloc::vec::Vec;
use super::DriverInfo;
use crate::drivers::manager::DeviceManager;

/// Добавляет драйверы синтетических и паравиртуальных подсистем виртуализации
pub fn append_virt_drivers(list: &mut Vec<DriverInfo>, dm: &DeviceManager) {
    let pci_devs = dm.pci_devices();
    let count_pci = |drv_name: &str| -> usize {
        pci_devs.iter().filter(|d| d.driver_name == Some(drv_name)).count()
    };

    // 1. VirtIO устройства (QEMU, KVM, Cloud-Hypervisor, Proxmox VE)
    let virtio_c = pci_devs.iter().filter(|d| d.vendor_id == 0x1AF4).count();
    if virtio_c > 0 {
        list.push(DriverInfo {
            name: "virtio_pci",
            description: "VirtIO Modern/Legacy PCI Transport",
            category: "virt",
            version: "1.0.0",
            author: "Wunix Kernel Team",
            license: "MIT/Apache-2.0",
            devices_count: virtio_c,
            status: "Live",
            size: 20480,
            dependencies: "pci_core",
        });
    }

    let virtio_blk_c = count_pci("virtio_blk");
    if virtio_blk_c > 0 {
        list.push(DriverInfo {
            name: "virtio_blk",
            description: "VirtIO Paravirtual Block Device (/dev/vd*)",
            category: "block",
            version: "1.0.0",
            author: "Wunix Kernel Team",
            license: "MIT/Apache-2.0",
            devices_count: virtio_blk_c,
            status: "Live",
            size: 24576,
            dependencies: "virtio_pci",
        });
    }

    let virtio_net_c = count_pci("virtio_net");
    if virtio_net_c > 0 {
        list.push(DriverInfo {
            name: "virtio_net",
            description: "VirtIO Paravirtual Network Adapter (Multi-Queue)",
            category: "net",
            version: "1.0.0",
            author: "Wunix Kernel Team",
            license: "MIT/Apache-2.0",
            devices_count: virtio_net_c,
            status: "Live",
            size: 28672,
            dependencies: "virtio_pci",
        });
    }

    let virtio_gpu_c = count_pci("virtio_gpu");
    if virtio_gpu_c > 0 {
        list.push(DriverInfo {
            name: "virtio_gpu",
            description: "VirtIO 2D/3D GPU Display Adapter",
            category: "display",
            version: "1.0.0",
            author: "Wunix Kernel Team",
            license: "MIT/Apache-2.0",
            devices_count: virtio_gpu_c,
            status: "Live",
            size: 32768,
            dependencies: "virtio_pci",
        });
    }

    let virtio_scsi_c = count_pci("virtio_scsi");
    if virtio_scsi_c > 0 {
        list.push(DriverInfo {
            name: "virtio_scsi",
            description: "VirtIO SCSI Paravirtual Controller (/dev/sd*)",
            category: "block",
            version: "1.0.0",
            author: "Wunix Kernel Team",
            license: "MIT/Apache-2.0",
            devices_count: virtio_scsi_c,
            status: "Live",
            size: 24576,
            dependencies: "virtio_pci",
        });
    }

    // 2. QEMU / Bochs интеграция
    let bochs_c = count_pci("bochs-vga");
    if bochs_c > 0 {
        list.push(DriverInfo {
            name: "bochs_vga",
            description: "QEMU / Bochs Standard VGA Display (VBE DISPI)",
            category: "display",
            version: "1.0.0",
            author: "Wunix Kernel Team",
            license: "MIT/Apache-2.0",
            devices_count: bochs_c,
            status: "Live",
            size: 16384,
            dependencies: "pci_core",
        });
    }

    // 3. VirtualBox Guest интеграция
    let vbox_guest_c = count_pci("vboxguest");
    if vbox_guest_c > 0 {
        list.push(DriverInfo {
            name: "vboxguest",
            description: "VirtualBox VMMDev Services (TimeSync, HGCM, Mouse)",
            category: "virt",
            version: "1.0.0",
            author: "Wunix Kernel Team",
            license: "MIT/Apache-2.0",
            devices_count: vbox_guest_c,
            status: "Live",
            size: 28672,
            dependencies: "pci_core",
        });
    }

    let vbox_video_c = count_pci("vboxvideo");
    if vbox_video_c > 0 {
        list.push(DriverInfo {
            name: "vboxvideo",
            description: "VirtualBox Graphics Adapter (VBoxSVGA / LFB)",
            category: "display",
            version: "1.0.0",
            author: "Wunix Kernel Team",
            license: "MIT/Apache-2.0",
            devices_count: vbox_video_c,
            status: "Live",
            size: 16384,
            dependencies: "pci_core",
        });
    }

    // 4. VMware Workstation / ESXi интеграция
    let vmw_svga_c = count_pci("vmw_svga");
    if vmw_svga_c > 0 {
        list.push(DriverInfo {
            name: "vmw_svga",
            description: "VMware SVGA II Accelerated Video Adapter",
            category: "display",
            version: "1.0.0",
            author: "Wunix Kernel Team",
            license: "MIT/Apache-2.0",
            devices_count: vmw_svga_c,
            status: "Live",
            size: 20480,
            dependencies: "pci_core",
        });
    }

    let vmxnet3_c = count_pci("vmxnet3");
    if vmxnet3_c > 0 {
        list.push(DriverInfo {
            name: "vmxnet3",
            description: "VMware vmxnet3 Paravirtual 10GbE Network Adapter",
            category: "net",
            version: "1.0.0",
            author: "Wunix Kernel Team",
            license: "MIT/Apache-2.0",
            devices_count: vmxnet3_c,
            status: "Live",
            size: 32768,
            dependencies: "pci_core",
        });
    }

    let pvscsi_c = count_pci("pvscsi");
    if pvscsi_c > 0 {
        list.push(DriverInfo {
            name: "pvscsi",
            description: "VMware PVSCSI Paravirtual SCSI Storage Controller",
            category: "block",
            version: "1.0.0",
            author: "Wunix Kernel Team",
            license: "MIT/Apache-2.0",
            devices_count: pvscsi_c,
            status: "Live",
            size: 28672,
            dependencies: "pci_core",
        });
    }

    // 5. Microsoft Hyper-V интеграция
    let hyperv_fb_c = count_pci("hyperv_fb");
    if hyperv_fb_c > 0 {
        list.push(DriverInfo {
            name: "hyperv_fb",
            description: "Microsoft Hyper-V Synthetic Video Framebuffer",
            category: "display",
            version: "1.0.0",
            author: "Wunix Kernel Team",
            license: "MIT/Apache-2.0",
            devices_count: hyperv_fb_c,
            status: "Live",
            size: 16384,
            dependencies: "pci_core",
        });
    }

    // 6. Xen гипервизор
    let xen_c = count_pci("xen-platform");
    if xen_c > 0 {
        list.push(DriverInfo {
            name: "xen_platform",
            description: "Xen HVM Platform Device & Grant Tables",
            category: "virt",
            version: "1.0.0",
            author: "Wunix Kernel Team",
            license: "MIT/Apache-2.0",
            devices_count: xen_c,
            status: "Live",
            size: 24576,
            dependencies: "pci_core",
        });
    }
}
