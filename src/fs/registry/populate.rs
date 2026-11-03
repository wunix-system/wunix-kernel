use alloc::string::String;
use super::{reg_create_key, reg_set_value, Hive, RegValue};

pub fn init_system_registry() {
    let _ = populate_nt_current_version();
    let _ = populate_system_control();
    let _ = populate_session_manager();
    let _ = populate_hardware_description();
    let _ = populate_user_profile();
    let _ = populate_classes_root();
}

fn populate_session_manager() -> Result<(), super::RegError> {
    let env = reg_create_key(Hive::LocalMachine, "SYSTEM/CurrentControlSet/Control/Session Manager/Environment")?;
    reg_set_value(&env, "PATH", &RegValue::Sz(String::from("C:\\Windows\\System32;C:\\Windows;/bin;/usr/bin")))?;
    reg_set_value(&env, "OS", &RegValue::Sz(String::from("Windows_NT")))?;
    reg_set_value(&env, "PROCESSOR_ARCHITECTURE", &RegValue::Sz(String::from("AMD64")))?;
    reg_set_value(&env, "NUMBER_OF_PROCESSORS", &RegValue::Sz(String::from("1")))?;
    reg_set_value(&env, "SystemDrive", &RegValue::Sz(String::from("C:")))?;
    reg_set_value(&env, "SystemRoot", &RegValue::Sz(String::from("C:\\Windows")))?;
    reg_set_value(&env, "ComSpec", &RegValue::Sz(String::from("C:\\Windows\\System32\\wsh.exe")))?;
    reg_set_value(&env, "TEMP", &RegValue::Sz(String::from("C:\\Windows\\Temp")))?;
    reg_set_value(&env, "TMP", &RegValue::Sz(String::from("C:\\Windows\\Temp")))?;
    Ok(())
}

fn populate_nt_current_version() -> Result<(), super::RegError> {
    let key = reg_create_key(Hive::LocalMachine, "SOFTWARE/Microsoft/Windows NT/CurrentVersion")?;
    reg_set_value(&key, "ProductName", &RegValue::Sz(String::from("Wunix 10 Pro")))?;
    reg_set_value(&key, "CurrentVersion", &RegValue::Sz(String::from("10.0")))?;
    reg_set_value(&key, "CurrentBuild", &RegValue::Sz(String::from("26100")))?;
    reg_set_value(&key, "CurrentBuildNumber", &RegValue::Sz(String::from("26100")))?;
    reg_set_value(&key, "ReleaseId", &RegValue::Sz(String::from("24H2")))?;
    reg_set_value(&key, "EditionID", &RegValue::Sz(String::from("Professional")))?;
    reg_set_value(&key, "InstallationType", &RegValue::Sz(String::from("Client")))?;
    reg_set_value(&key, "ProductId", &RegValue::Sz(String::from("00330-80000-00000-AAOEM")))?;
    reg_set_value(&key, "SystemRoot", &RegValue::Sz(String::from("C:\\Windows")))?;
    reg_set_value(&key, "PathName", &RegValue::Sz(String::from("C:\\Windows")))?;
    reg_set_value(&key, "RegisteredOwner", &RegValue::Sz(String::from("Administrator")))?;
    reg_set_value(&key, "RegisteredOrganization", &RegValue::Sz(String::from("Wunix Project")))?;

    let winlogon = reg_create_key(Hive::LocalMachine, "SOFTWARE/Microsoft/Windows NT/CurrentVersion/Winlogon")?;
    reg_set_value(&winlogon, "Shell", &RegValue::Sz(String::from("explorer.exe")))?;
    reg_set_value(&winlogon, "DefaultUserName", &RegValue::Sz(String::from("Administrator")))?;

    Ok(())
}

fn populate_system_control() -> Result<(), super::RegError> {
    let control = reg_create_key(Hive::LocalMachine, "SYSTEM/CurrentControlSet/Control")?;
    reg_set_value(&control, "SystemBootDevice", &RegValue::Sz(String::from("\\Device\\HarddiskVolume1")))?;
    reg_set_value(&control, "OsLoaderPath", &RegValue::Sz(String::from("\\EFI\\BOOT\\BOOTX64.EFI")))?;
    reg_set_value(&control, "CurrentUser", &RegValue::Sz(String::from("Administrator")))?;

    let comp_name = reg_create_key(Hive::LocalMachine, "SYSTEM/CurrentControlSet/Control/ComputerName/ActiveComputerName")?;
    reg_set_value(&comp_name, "ComputerName", &RegValue::Sz(String::from("WUNIX")))?;

    let tcpip = reg_create_key(Hive::LocalMachine, "SYSTEM/CurrentControlSet/Services/Tcpip/Parameters")?;
    reg_set_value(&tcpip, "Hostname", &RegValue::Sz(String::from("wunix")))?;
    reg_set_value(&tcpip, "Domain", &RegValue::Sz(String::from("local")))?;
    reg_set_value(&tcpip, "NameServer", &RegValue::Sz(String::from("1.1.1.1 8.8.8.8")))?;

    Ok(())
}

fn populate_hardware_description() -> Result<(), super::RegError> {
    let cpu = reg_create_key(Hive::LocalMachine, "HARDWARE/DESCRIPTION/System/CentralProcessor/0")?;
    reg_set_value(&cpu, "ProcessorNameString", &RegValue::Sz(String::from("x86_64 Multiprocessor")))?;
    reg_set_value(&cpu, "VendorIdentifier", &RegValue::Sz(String::from("GenuineIntel")))?;
    reg_set_value(&cpu, "Identifier", &RegValue::Sz(String::from("Intel64 Family 6 Model 142 Stepping 10")))?;
    reg_set_value(&cpu, "~MHz", &RegValue::DWord(2400))?;

    let bios = reg_create_key(Hive::LocalMachine, "HARDWARE/DESCRIPTION/System/BIOS")?;
    reg_set_value(&bios, "BIOSVendor", &RegValue::Sz(String::from("Limine Bootloader")))?;
    reg_set_value(&bios, "BIOSVersion", &RegValue::Sz(String::from("0.6.5")))?;
    reg_set_value(&bios, "SystemManufacturer", &RegValue::Sz(String::from("QEMU / Wunix Virtual Platform")))?;

    Ok(())
}

fn populate_user_profile() -> Result<(), super::RegError> {
    let env = reg_create_key(Hive::CurrentUser, "Environment")?;
    reg_set_value(&env, "PATH", &RegValue::Sz(String::from("C:\\Windows\\System32;C:\\Windows;C:\\Program Files")))?;
    reg_set_value(&env, "TEMP", &RegValue::Sz(String::from("C:\\Windows\\Temp")))?;
    reg_set_value(&env, "TMP", &RegValue::Sz(String::from("C:\\Windows\\Temp")))?;
    reg_set_value(&env, "USERNAME", &RegValue::Sz(String::from("Administrator")))?;
    reg_set_value(&env, "USERPROFILE", &RegValue::Sz(String::from("C:\\Users\\Administrator")))?;

    let folders = reg_create_key(Hive::CurrentUser, "Software/Microsoft/Windows/CurrentVersion/Explorer/Shell Folders")?;
    reg_set_value(&folders, "Desktop", &RegValue::Sz(String::from("C:\\Users\\Administrator\\Desktop")))?;
    reg_set_value(&folders, "Personal", &RegValue::Sz(String::from("C:\\Users\\Administrator\\Documents")))?;
    reg_set_value(&folders, "My Music", &RegValue::Sz(String::from("C:\\Users\\Administrator\\Music")))?;
    reg_set_value(&folders, "My Pictures", &RegValue::Sz(String::from("C:\\Users\\Administrator\\Pictures")))?;
    reg_set_value(&folders, "My Video", &RegValue::Sz(String::from("C:\\Users\\Administrator\\Videos")))?;

    Ok(())
}

fn populate_classes_root() -> Result<(), super::RegError> {
    let txt = reg_create_key(Hive::ClassesRoot, ".txt")?;
    reg_set_value(&txt, "", &RegValue::Sz(String::from("txtfile")))?;

    let cmd = reg_create_key(Hive::ClassesRoot, "txtfile/shell/open/command")?;
    reg_set_value(&cmd, "", &RegValue::Sz(String::from("notepad.exe %1")))?;

    let exe = reg_create_key(Hive::ClassesRoot, ".exe")?;
    reg_set_value(&exe, "", &RegValue::Sz(String::from("exefile")))?;

    let exefile = reg_create_key(Hive::ClassesRoot, "exefile/shell/open/command")?;
    reg_set_value(&exefile, "", &RegValue::Sz(String::from("\"%1\" %*")))?;

    Ok(())
}
