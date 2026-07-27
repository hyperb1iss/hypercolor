use std::ffi::c_void;
use std::sync::Arc;

use windows::Win32::Foundation::HANDLE;

use super::{DeviceCache, DeviceMetadata};
use crate::shared::RawDeviceKind;

fn metadata(path: &str, label: &str, kind: RawDeviceKind) -> DeviceMetadata {
    DeviceMetadata {
        interface_path: Arc::from(path),
        label: Arc::from(label),
        kind,
    }
}

fn handle(value: usize) -> HANDLE {
    HANDLE(value as *mut c_void)
}

#[test]
fn null_devices_are_partitioned_by_kind_and_stable_within_one_session() {
    let mut cache = DeviceCache::new(41);

    let keyboard = cache.resolve_null(RawDeviceKind::Keyboard);
    let keyboard_again = cache.resolve_null(RawDeviceKind::Keyboard);
    let mouse = cache.resolve_null(RawDeviceKind::Mouse);

    assert!(keyboard.discovered);
    assert!(!keyboard_again.discovered);
    assert!(Arc::ptr_eq(&keyboard.device, &keyboard_again.device));
    assert_ne!(keyboard.device.source_id, mouse.device.source_id);
    assert_eq!(keyboard.device.session_generation, 41);
    assert_eq!(mouse.device.session_generation, 41);
    assert_eq!(keyboard.device.kind, RawDeviceKind::Keyboard);
    assert_eq!(mouse.device.kind, RawDeviceKind::Mouse);
    assert_eq!(cache.len(), 2);
}

#[test]
fn handle_reuse_retires_the_old_generation_before_installing_the_new_one() {
    let mut cache = DeviceCache::new(7);
    let native = handle(0x1234);
    let first = cache.install(
        native,
        metadata("first-interface", "first keyboard", RawDeviceKind::Keyboard),
    );
    let second = cache.install(
        native,
        metadata("second-interface", "second mouse", RawDeviceKind::Mouse),
    );

    let retired = second.retired.expect("the old native lifetime is retired");
    assert!(Arc::ptr_eq(&retired, &first.device));
    assert_ne!(first.device.source_id, second.device.source_id);
    assert!(second.device.device_generation > first.device.device_generation);
    assert_eq!(second.device.kind, RawDeviceKind::Mouse);
    assert_eq!(cache.len(), 1);
}

#[test]
fn removing_and_readding_the_same_interface_never_reuses_source_identity() {
    let mut cache = DeviceCache::new(9);
    let native = handle(0x5678);
    let first = cache.install(
        native,
        metadata("same-interface", "keyboard", RawDeviceKind::Keyboard),
    );
    assert!(Arc::ptr_eq(
        &cache.remove(native).expect("first lifetime exists"),
        &first.device
    ));
    let replacement = cache.install(
        native,
        metadata("same-interface", "keyboard", RawDeviceKind::Keyboard),
    );

    assert_ne!(first.device.source_id, replacement.device.source_id);
    assert!(replacement.device.device_generation > first.device.device_generation);
    assert!(replacement.retired.is_none());
}

#[test]
fn metadata_refresh_preserves_identity_and_device_generation() {
    let mut cache = DeviceCache::new(11);
    let native = handle(0x9012);
    let first = cache.install(
        native,
        metadata("same-interface", "generic mouse", RawDeviceKind::Mouse),
    );
    let refreshed = cache.refresh_metadata(
        native,
        metadata("same-interface", "friendly mouse", RawDeviceKind::Mouse),
    );

    assert!(!refreshed.discovered);
    assert!(refreshed.metadata_changed);
    assert!(refreshed.retired.is_none());
    assert_eq!(refreshed.device.source_id, first.device.source_id);
    assert_eq!(
        refreshed.device.device_generation,
        first.device.device_generation
    );
    assert_eq!(refreshed.device.label.as_ref(), "friendly mouse");
}
