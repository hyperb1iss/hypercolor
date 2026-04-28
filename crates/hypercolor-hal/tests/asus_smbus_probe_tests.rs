use std::fs;
use std::os::unix::fs as unix_fs;

use hypercolor_hal::drivers::asus::{
    SmBusControllerKind, build_aura_smbus_protocol, dram_capable_pci_id,
    probe_asus_smbus_devices_in_root, resolve_parent_pci_id_from_sysfs_path,
};
use hypercolor_hal::protocol::Protocol;
use tempfile::tempdir;

#[tokio::test]
async fn asus_smbus_probe_ignores_empty_dev_root() {
    let tempdir = tempdir().expect("tempdir should create");

    let devices = probe_asus_smbus_devices_in_root(tempdir.path())
        .await
        .expect("probe should succeed");

    assert!(devices.is_empty());
}

#[tokio::test]
async fn asus_smbus_probe_ignores_non_device_i2c_nodes() {
    let tempdir = tempdir().expect("tempdir should create");
    let fake_bus = tempdir.path().join("i2c-0");
    fs::write(&fake_bus, b"not a real i2c bus").expect("fake i2c node should write");

    let devices = probe_asus_smbus_devices_in_root(tempdir.path())
        .await
        .expect("probe should succeed");

    assert!(devices.is_empty());
}

#[test]
fn asus_smbus_probe_walks_up_sysfs_tree_for_parent_pci_id() {
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
fn asus_smbus_probe_matches_openrgb_dram_bus_allowlist() {
    assert!(dram_capable_pci_id(0x8086, 0x7A23));
    assert!(!dram_capable_pci_id(0x8086, 0x7A4C));
    assert!(!dram_capable_pci_id(0x8086, 0x7A4D));
    assert!(!dram_capable_pci_id(0x8086, 0x7A4E));
    assert!(!dram_capable_pci_id(0x10DE, 0x2783));
}

#[test]
fn asus_smbus_probe_names_controller_kinds() {
    assert_eq!(
        SmBusControllerKind::Motherboard.display_name(),
        "Motherboard"
    );
    assert_eq!(
        SmBusControllerKind::Motherboard.model_id(),
        "asus_aura_smbus_motherboard"
    );
    assert_eq!(SmBusControllerKind::Gpu.display_name(), "GPU");
    assert_eq!(SmBusControllerKind::Gpu.model_id(), "asus_aura_smbus_gpu");
    assert_eq!(SmBusControllerKind::Dram.display_name(), "DRAM");
    assert_eq!(SmBusControllerKind::Dram.model_id(), "asus_aura_smbus_dram");
}

#[test]
fn asus_smbus_protocol_factory_hides_concrete_protocol_type() {
    let protocol: Box<dyn Protocol> = build_aura_smbus_protocol();

    assert_eq!(protocol.name(), "ASUS Aura ENE SMBus");
}
