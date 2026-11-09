//! Spec 89 stability labels on driver descriptors.

use hypercolor_driver_api::{DriverDescriptor, DriverModule};
use hypercolor_types::device::{DriverStability, DriverTransportKind};

static PROVEN: DriverDescriptor = DriverDescriptor::new(
    "proven-lights",
    "Proven Lights",
    DriverTransportKind::Network,
    true,
    false,
);

static UNTESTED: DriverDescriptor = DriverDescriptor::new(
    "untested-lights",
    "Untested Lights",
    DriverTransportKind::Network,
    true,
    false,
)
.experimental();

struct UntestedDriver;

impl DriverModule for UntestedDriver {
    fn descriptor(&self) -> &'static DriverDescriptor {
        &UNTESTED
    }
}

#[test]
fn new_descriptors_are_stable() {
    assert_eq!(PROVEN.stability, DriverStability::Stable);
    assert!(!PROVEN.is_experimental());

    let module = PROVEN.module_descriptor();
    assert_eq!(module.stability, DriverStability::Stable);
    assert!(module.default_enabled);
}

#[test]
fn experimental_builder_sets_only_the_label() {
    assert_eq!(UNTESTED.stability, DriverStability::Experimental);
    assert!(UNTESTED.is_experimental());
    assert_eq!(UNTESTED.id, "untested-lights");
    assert!(UNTESTED.supports_discovery);

    let module = UNTESTED.module_descriptor();
    assert_eq!(module.stability, DriverStability::Experimental);
    assert!(
        module.default_enabled,
        "an experimental driver is enabled by default like any other"
    );
}

#[test]
fn default_module_descriptor_carries_the_label() {
    let module = UntestedDriver.module_descriptor();

    assert_eq!(module.stability, DriverStability::Experimental);
    assert!(module.default_enabled);
}
