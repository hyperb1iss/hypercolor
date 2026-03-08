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
