//! Spec 89 stability labels on HAL descriptors, modules, and protocols.

use hypercolor_hal::database::ProtocolDatabase;
use hypercolor_hal::registry::{DeviceDescriptor, ProtocolBinding};
use hypercolor_types::device::{DeviceFamily, DriverStability};

/// A copy of a registered descriptor moved into a fake family, so the
/// tests reuse a real protocol factory without touching the static
/// database.
fn fake_descriptor(
    protocol_id: &'static str,
    product_id: u16,
    stability: DriverStability,
) -> DeviceDescriptor {
    let template = ProtocolDatabase::all()
        .first()
        .expect("the HAL database registers descriptors")
        .clone();
    DeviceDescriptor {
        vendor_id: 0x1209,
        product_id,
        name: "Fake Controller",
        family: DeviceFamily::new_static("fakefam", "Fake Family"),
        protocol: ProtocolBinding {
            id: protocol_id,
            build: template.protocol.build,
        },
        stability,
        ..template
    }
}

#[test]
fn registered_descriptors_keep_the_stable_label() {
    let experimental = ProtocolDatabase::all()
        .iter()
        .filter(|descriptor| descriptor.stability.is_experimental())
        .map(|descriptor| descriptor.protocol.id)
        .collect::<Vec<_>>();
    assert!(
        experimental.is_empty(),
        "no shipped descriptor is labeled experimental yet: {experimental:?}"
    );

    for module in ProtocolDatabase::module_descriptors() {
        assert_eq!(module.stability, DriverStability::Stable, "{}", module.id);
        assert!(
            module.default_enabled,
            "{} stays enabled by default",
            module.id
        );
    }
    for protocol in ProtocolDatabase::protocol_descriptors() {
        assert_eq!(
            protocol.stability,
            DriverStability::Stable,
            "{}",
            protocol.protocol_id
        );
    }
}

#[test]
fn registered_protocol_ids_carry_one_label_each() {
    assert_eq!(
        ProtocolDatabase::stability_conflicts(ProtocolDatabase::all()),
        Vec::<&str>::new()
    );
}

#[test]
fn stability_conflicts_name_protocols_that_mix_labels() {
    let descriptors = [
        fake_descriptor("fakefam/shared", 0x0001, DriverStability::Stable),
        fake_descriptor("fakefam/shared", 0x0002, DriverStability::Experimental),
        fake_descriptor("fakefam/solo", 0x0003, DriverStability::Experimental),
    ];

    assert_eq!(
        ProtocolDatabase::stability_conflicts(&descriptors),
        vec!["fakefam/shared"]
    );
}

#[test]
fn a_family_of_only_experimental_descriptors_is_an_experimental_module() {
    let descriptors = [
        fake_descriptor("fakefam/one", 0x0001, DriverStability::Experimental),
        fake_descriptor("fakefam/two", 0x0002, DriverStability::Experimental),
    ];

    let modules = ProtocolDatabase::derive_module_descriptors(&descriptors);

    assert_eq!(modules.len(), 1);
    assert_eq!(modules[0].id, "fakefam");
    assert_eq!(modules[0].stability, DriverStability::Experimental);
    assert!(
        modules[0].default_enabled,
        "the label never changes whether a module runs"
    );
}

#[test]
fn one_stable_descriptor_keeps_a_mixed_family_stable() {
    let descriptors = [
        fake_descriptor("fakefam/proven", 0x0001, DriverStability::Stable),
        fake_descriptor("fakefam/untested", 0x0002, DriverStability::Experimental),
    ];

    let modules = ProtocolDatabase::derive_module_descriptors(&descriptors);

    assert_eq!(modules.len(), 1);
    assert_eq!(modules[0].stability, DriverStability::Stable);
    assert!(modules[0].default_enabled);
}

#[test]
fn protocol_descriptors_carry_the_descriptor_label() {
    let untested = fake_descriptor("fakefam/untested", 0x0002, DriverStability::Experimental);
    let proven = fake_descriptor("fakefam/proven", 0x0001, DriverStability::Stable);

    let untested = ProtocolDatabase::protocol_descriptor_for(&untested);
    let proven = ProtocolDatabase::protocol_descriptor_for(&proven);

    assert_eq!(untested.driver_id, "fakefam");
    assert_eq!(untested.protocol_id, "fakefam/untested");
    assert_eq!(untested.stability, DriverStability::Experimental);
    assert_eq!(proven.stability, DriverStability::Stable);
}
