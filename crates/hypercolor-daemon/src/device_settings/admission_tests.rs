use super::*;
use axum::extract::State;
use hypercolor_driver_api::{DiscoveredDevice, DiscoveryConnectBehavior};
use hypercolor_types::api::devices::UpdateDeviceRequest;
use hypercolor_types::device::DeviceFingerprint;
use std::future::Future;
use std::task::{Context, Poll, Waker};

use crate::api::extract::{ApiJson, ApiPath};

fn brightness(value: u8) -> ApiJson<UpdateDeviceRequest> {
    ApiJson(UpdateDeviceRequest {
        name: None,
        enabled: None,
        brightness: Some(value),
        display_rotation: None,
    })
}

#[tokio::test]
async fn restore_admission_spans_live_device_update_and_disk_commit() {
    let directory = tempfile::tempdir().expect("isolated data");
    let state = Arc::new(crate::app_state::AppState::new_with_data_dir(
        directory.path().to_path_buf(),
    ));
    let (fixture, id, key) = super::tests::claimed_registry("net:wled:admission").await;
    let tracked = fixture.get(&id).await.expect("fixture device");
    state
        .device_registry
        .add_discovered(DiscoveredDevice {
            info: tracked.info,
            fingerprint: DeviceFingerprint::from_persisted("net:wled:admission"),
            metadata: HashMap::new(),
            connect_behavior: DiscoveryConnectBehavior::Deferred,
            claim: fixture.claim_for_id(&id).await,
        })
        .await;

    // Block only the disk store. The ordinary API has already changed the live
    // registry when restore asks for its exclusive snapshot authority.
    let store = state.device_settings.store.write().await;
    let mut update = Box::pin(crate::api::devices::update_device(
        State(Arc::clone(&state)),
        ApiPath(id.to_string()),
        brightness(20),
    ));
    let mut context = Context::from_waker(Waker::noop());
    assert!(matches!(update.as_mut().poll(&mut context), Poll::Pending));
    assert_eq!(
        state
            .device_registry
            .get(&id)
            .await
            .expect("live device")
            .user_settings
            .brightness,
        0.2
    );
    let independently_obtained = state.output_power.device_settings();
    let mut restore = Box::pin(independently_obtained.begin_portable_restore());
    assert!(matches!(restore.as_mut().poll(&mut context), Poll::Pending));
    drop(store);
    assert!(update.await.status().is_success());
    let restore_guard = restore.await;
    assert_eq!(
        state
            .device_settings
            .device_settings_for_key(&key)
            .await
            .expect("persisted device")
            .brightness,
        0.2
    );

    let mut next = Box::pin(crate::api::devices::update_device(
        State(Arc::clone(&state)),
        ApiPath(id.to_string()),
        brightness(90),
    ));
    assert!(matches!(next.as_mut().poll(&mut context), Poll::Pending));
    assert_eq!(
        state
            .device_registry
            .get(&id)
            .await
            .expect("unchanged live device")
            .user_settings
            .brightness,
        0.2
    );
    drop(restore_guard);
    assert!(next.await.status().is_success());
    assert_eq!(
        state
            .device_settings
            .device_settings_for_key(&key)
            .await
            .expect("next settings")
            .brightness,
        0.9
    );
}

#[tokio::test]
async fn ordinary_device_mutations_keep_independent_admission() {
    let directory = tempfile::tempdir().expect("isolated settings");
    let power = crate::output_power::OutputPower::new(DeviceSettingsStore::new(
        directory.path().join("settings.json"),
    ));
    let first = power.device_settings().begin_device_mutation().await;
    let second_access = power.device_settings();
    let mut second = Box::pin(second_access.begin_device_mutation());
    let mut context = Context::from_waker(Waker::noop());
    let Poll::Ready(second) = second.as_mut().poll(&mut context) else {
        panic!("ordinary device operations must remain concurrent");
    };
    drop((first, second));
}
