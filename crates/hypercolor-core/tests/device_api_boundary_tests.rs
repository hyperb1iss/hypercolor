#[test]
fn driver_contracts_have_one_public_import_path() {
    let device_module = include_str!("../src/device/mod.rs");

    assert!(!device_module.contains("pub use hypercolor_driver_api"));
    assert!(!device_module.contains("mod traits;"));
}
