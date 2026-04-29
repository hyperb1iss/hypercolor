use std::sync::Arc;

use hypercolor_driver_api::CredentialStore;
use hypercolor_driver_builtin::build_driver_module_registry;
use hypercolor_network::DriverModuleRegistry;
use hypercolor_types::config::HypercolorConfig;
use hypercolor_types::device::DriverModuleKind;
use tempfile::tempdir;

#[test]
fn build_driver_module_registry_registers_compiled_in_drivers() {
    let tempdir = tempdir().expect("tempdir should be created");
    let credentials = Arc::new(
        CredentialStore::open_blocking(tempdir.path())
            .expect("credential store should open for registry test"),
    );
    let config = HypercolorConfig::default();

    let registry =
        build_driver_module_registry(&config, credentials).expect("registry should build");
    let ids = registry.ids();

    assert!(ids.contains(&"wled".to_owned()));
    #[cfg(feature = "hue")]
    assert!(ids.contains(&"hue".to_owned()));
    #[cfg(feature = "nanoleaf")]
    assert!(ids.contains(&"nanoleaf".to_owned()));
    assert!(ids.contains(&"nollie".to_owned()));
    assert!(ids.contains(&"prismrgb".to_owned()));

    for driver_id in ["wled", "govee", "hue", "nanoleaf"] {
        let Some(driver) = registry.get(driver_id) else {
            continue;
        };
        let descriptor = driver.module_descriptor();
        assert!(descriptor.capabilities.config);
        assert!(descriptor.capabilities.controls);
        driver
            .config()
            .expect("config provider should be present")
            .validate_config(&driver.config().expect("config provider").default_config())
            .expect("default config should validate");
    }

    let nollie = registry
        .get("nollie")
        .expect("HAL catalog module should resolve");
    let descriptor = nollie.module_descriptor();
    assert_eq!(descriptor.module_kind, DriverModuleKind::Hal);
    assert!(descriptor.capabilities.protocol_catalog);
    assert!(!descriptor.capabilities.output_backend);
    assert!(
        !nollie
            .protocol_catalog()
            .expect("HAL module should expose protocol catalog")
            .descriptors()
            .is_empty()
    );
}

#[test]
fn register_driver_modules_appends_to_existing_registry() {
    let tempdir = tempdir().expect("tempdir should be created");
    let credentials = Arc::new(
        CredentialStore::open_blocking(tempdir.path())
            .expect("credential store should open for registry test"),
    );
    let config = HypercolorConfig::default();
    let mut registry = DriverModuleRegistry::new();

    hypercolor_driver_builtin::register_driver_modules(&mut registry, &config, credentials)
        .expect("drivers should register");

    assert!(registry.get("wled").is_some());
}
