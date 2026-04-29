use std::sync::Arc;

use hypercolor_driver_api::CredentialStore;
use hypercolor_driver_builtin::build_driver_module_registry;
use hypercolor_network::DriverModuleRegistry;
use hypercolor_types::config::HypercolorConfig;
#[cfg(feature = "hal")]
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
    #[cfg(not(any(feature = "network", feature = "hal")))]
    let _ = &ids;

    #[cfg(feature = "network")]
    {
        assert!(ids.contains(&"wled".to_owned()));
        assert!(ids.contains(&"govee".to_owned()));
        assert!(ids.contains(&"hue".to_owned()));
        assert!(ids.contains(&"nanoleaf".to_owned()));
    }

    #[cfg(feature = "hal")]
    {
        assert!(ids.contains(&"nollie".to_owned()));
        assert!(ids.contains(&"prismrgb".to_owned()));
    }

    #[cfg(feature = "network")]
    for driver_id in ["wled", "govee", "hue", "nanoleaf"] {
        let driver = registry
            .get(driver_id)
            .expect("network driver should be registered");
        let descriptor = driver.module_descriptor();
        assert!(descriptor.capabilities.config);
        assert!(descriptor.capabilities.controls);
        driver
            .config()
            .expect("config provider should be present")
            .validate_config(&driver.config().expect("config provider").default_config())
            .expect("default config should validate");
    }

    #[cfg(feature = "hal")]
    {
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

    #[cfg(feature = "network")]
    assert!(registry.get("wled").is_some());
    #[cfg(feature = "hal")]
    assert!(registry.get("nollie").is_some());
}
