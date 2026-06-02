use std::sync::Arc;

use hypercolor_daemon::extensions::{ExtensionRegistry, ExtensionRegistryError};

#[derive(Debug, PartialEq, Eq)]
struct TestExtensionState {
    label: &'static str,
}

#[test]
fn extension_registry_round_trips_typed_state() {
    let registry = ExtensionRegistry::default();
    let state = Arc::new(TestExtensionState { label: "studio" });

    registry
        .insert(Arc::clone(&state))
        .expect("state should register");

    let loaded = registry
        .get::<TestExtensionState>()
        .expect("state should load by concrete type");
    assert!(Arc::ptr_eq(&state, &loaded));
    assert_eq!(loaded.label, "studio");
    assert!(registry.contains::<TestExtensionState>());
}

#[test]
fn extension_registry_rejects_duplicate_state_type() {
    let registry = ExtensionRegistry::default();
    registry
        .insert(Arc::new(TestExtensionState { label: "first" }))
        .expect("initial state should register");

    let error = registry
        .insert(Arc::new(TestExtensionState { label: "second" }))
        .expect_err("duplicate state type should be rejected");

    assert!(matches!(
        error,
        ExtensionRegistryError::DuplicateState { type_name }
            if type_name.ends_with("TestExtensionState")
    ));
}

#[test]
fn extension_registry_lists_state_names_in_stable_order() {
    #[derive(Debug)]
    struct AnotherState;

    let registry = ExtensionRegistry::default();
    registry
        .insert(Arc::new(TestExtensionState { label: "first" }))
        .expect("test state should register");
    registry
        .insert(Arc::new(AnotherState))
        .expect("another state should register");

    let names = registry.state_names();
    assert_eq!(names.len(), 2);
    assert!(names[0] < names[1]);
}
