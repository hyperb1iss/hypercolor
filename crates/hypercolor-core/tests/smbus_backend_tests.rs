use std::fs;
use std::os::unix::fs as unix_fs;

use hypercolor_core::device::smbus_scanner::{
    dram_capable_pci_id, resolve_parent_pci_id_from_sysfs_path,
};
use hypercolor_core::device::{DeviceBackend, SmBusBackend, SmBusScanner, TransportScanner};
use tempfile::tempdir;

#[test]
fn smbus_scanner_name_is_stable() {
    let scanner = SmBusScanner::new();
    assert_eq!(scanner.name(), "SMBus HAL");
}

#[tokio::test]
async fn smbus_scanner_ignores_empty_dev_root() {
    let tempdir = tempdir().expect("tempdir should create");
    let mut scanner = SmBusScanner::with_dev_root(tempdir.path());

    let devices = scanner.scan().await.expect("scan should succeed");
    assert!(devices.is_empty());
}

#[tokio::test]
async fn smbus_scanner_ignores_non_device_i2c_nodes() {
    let tempdir = tempdir().expect("tempdir should create");
    let fake_bus = tempdir.path().join("i2c-0");
    fs::write(&fake_bus, b"not a real i2c bus").expect("fake i2c node should write");

    let mut scanner = SmBusScanner::with_dev_root(tempdir.path());
    let devices = scanner.scan().await.expect("scan should succeed");

    assert!(devices.is_empty());
}

#[test]
fn smbus_backend_info_reports_hal_transport() {
    let backend = SmBusBackend::new();
    let info = backend.info();

    assert_eq!(info.id, "smbus");
    assert_eq!(info.name, "SMBus (HAL)");
}

#[tokio::test]
async fn smbus_backend_discover_is_empty_on_empty_dev_root() {
    let tempdir = tempdir().expect("tempdir should create");
    let mut backend = SmBusBackend::with_scanner(SmBusScanner::with_dev_root(tempdir.path()));

    let devices = backend.discover().await.expect("discover should succeed");
    assert!(devices.is_empty());
}

#[test]
fn smbus_scanner_walks_up_sysfs_tree_for_parent_pci_id() {
    let tempdir = tempdir().expect("tempdir should create");
    let pci_root = tempdir.path().join("0000:00:15.0");
    let adapter_root = pci_root.join("i2c_designware.0").join("i2c-0");

    fs::create_dir_all(&adapter_root).expect("adapter tree should create");
    fs::write(pci_root.join("vendor"), "0x8086\n").expect("vendor file should write");
    fs::write(pci_root.join("device"), "0x7A4C\n").expect("device file should write");

    let sysfs_entry = tempdir.path().join("i2c-0-device");
    unix_fs::symlink(&adapter_root, &sysfs_entry).expect("symlink should create");

    let pci_id =
        resolve_parent_pci_id_from_sysfs_path(&sysfs_entry).expect("pci id should resolve");
    assert_eq!(pci_id, (0x8086, 0x7A4C));
}

#[test]
fn smbus_scanner_accepts_rpl_designware_system_buses_for_dram_scan() {
    assert!(dram_capable_pci_id(0x8086, 0x7A23));
    assert!(dram_capable_pci_id(0x8086, 0x7A4C));
    assert!(dram_capable_pci_id(0x8086, 0x7A4D));
    assert!(dram_capable_pci_id(0x8086, 0x7A4E));
    assert!(!dram_capable_pci_id(0x10DE, 0x2783));
}
