//! The competing-software catalog only names drivers this daemon ships.

use std::sync::Arc;

use hypercolor_core::device::{SoftwareCatalog, UnclaimedDeviceStore, UsbProtocolConfigStore};
use hypercolor_daemon::network;
use hypercolor_driver_support::CredentialStore;
use hypercolor_types::config::HypercolorConfig;

#[test]
fn every_catalog_driver_id_is_a_registered_driver() {
    let credential_store = Arc::new(
        CredentialStore::open_blocking(&std::env::temp_dir().join(format!(
            "hypercolor-test-credentials-{}",
            uuid::Uuid::now_v7()
        )))
        .expect("test credential store"),
    );
    let registry = network::build_builtin_driver_module_registry(
        &HypercolorConfig::default(),
        credential_store,
        UsbProtocolConfigStore::new(),
        UnclaimedDeviceStore::new(),
    )
    .expect("builtin driver registry");
    let registered = registry.ids();

    for driver_id in SoftwareCatalog::builtin().driver_ids() {
        assert!(
            registered.iter().any(|id| id == driver_id),
            "the catalog names `{driver_id}`, which is not a registered driver: {registered:?}"
        );
    }
}
