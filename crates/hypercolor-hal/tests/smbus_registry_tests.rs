use hypercolor_hal::{ASUS_AURA_SMBUS_PROTOCOL_ID, build_smbus_protocol};

#[test]
fn smbus_registry_builds_known_protocols_by_id() {
    let protocol = build_smbus_protocol(ASUS_AURA_SMBUS_PROTOCOL_ID)
        .expect("ASUS SMBus protocol should build");

    assert_eq!(protocol.name(), "ASUS Aura ENE SMBus");
}

#[test]
fn smbus_registry_rejects_unknown_protocol_ids() {
    assert!(build_smbus_protocol("vendor/unknown-smbus").is_none());
}
