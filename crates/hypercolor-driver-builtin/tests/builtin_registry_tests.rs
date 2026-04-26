use std::sync::Arc;

use hypercolor_core::device::net::CredentialStore;
use hypercolor_driver_builtin::build_driver_registry;
use hypercolor_network::DriverRegistry;
use hypercolor_types::config::HypercolorConfig;
use tempfile::tempdir;

#[test]
fn build_driver_registry_registers_compiled_in_drivers() {
    let tempdir = tempdir().expect("tempdir should be created");
    let credentials = Arc::new(
        CredentialStore::open_blocking(tempdir.path())
            .expect("credential store should open for registry test"),
    );
    let config = HypercolorConfig::default();

    let registry = build_driver_registry(&config, credentials).expect("registry should build");
    let ids = registry.ids();

    assert!(ids.contains(&"wled".to_owned()));
    #[cfg(feature = "hue")]
    assert!(ids.contains(&"hue".to_owned()));
    #[cfg(feature = "nanoleaf")]
    assert!(ids.contains(&"nanoleaf".to_owned()));
}

#[test]
fn register_drivers_appends_to_existing_registry() {
    let tempdir = tempdir().expect("tempdir should be created");
    let credentials = Arc::new(
        CredentialStore::open_blocking(tempdir.path())
            .expect("credential store should open for registry test"),
    );
    let config = HypercolorConfig::default();
    let mut registry = DriverRegistry::new();

    hypercolor_driver_builtin::register_drivers(&mut registry, &config, credentials)
        .expect("drivers should register");

    assert!(registry.get("wled").is_some());
}
