use hypercolor_driver_api::{DRIVER_API_SCHEMA_VERSION, DriverDescriptor};
use hypercolor_types::device::DriverTransportKind;

#[test]
fn schema_version_constant_is_stamped_onto_new_descriptors() {
    let descriptor = DriverDescriptor::new(
        "fixture-network",
        "Fixture Network",
        DriverTransportKind::Network,
        true,
        true,
    );
    assert_eq!(descriptor.schema_version, DRIVER_API_SCHEMA_VERSION);
}

#[test]
fn with_schema_version_accepts_explicit_value() {
    let descriptor = DriverDescriptor::with_schema_version(
        "legacy",
        "Legacy",
        DriverTransportKind::Network,
        true,
        false,
        0,
    );
    assert_eq!(descriptor.schema_version, 0);
}
