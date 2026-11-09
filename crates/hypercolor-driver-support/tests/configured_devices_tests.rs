//! Live authorization of manually configured devices.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use hypercolor_driver_api::{DriverConfigSource, DriverConfigView, DriverError};
use hypercolor_driver_support::ConfiguredDevices;
use hypercolor_types::config::DriverConfigEntry;
use hypercolor_types::device::{
    DeviceError, DeviceFingerprint, DeviceId, ErrorRecoverability, FingerprintNamespace,
};
use serde_json::{Value, json};

fn device_id(name: &str) -> DeviceId {
    DeviceFingerprint::mint(FingerprintNamespace::Net, "fixture", name).stable_device_id()
}

/// Reads `devices = [[name, target]]`; a plan is the target.
fn resolve(config: DriverConfigView<'_>) -> Result<Vec<(DeviceId, String)>, DriverError> {
    let settings = config.parse_settings::<BTreeMap<String, Vec<[String; 2]>>>()?;
    Ok(settings
        .into_values()
        .flatten()
        .map(|[name, target]| (device_id(&name), target))
        .collect())
}

#[derive(Default)]
struct LiveConfig(Mutex<DriverConfigEntry>);

impl LiveConfig {
    fn set(&self, entry: DriverConfigEntry) {
        *self.0.lock().expect("config lock") = entry;
    }
}

impl DriverConfigSource for LiveConfig {
    fn current(&self) -> DriverConfigEntry {
        self.0.lock().expect("config lock").clone()
    }
}

fn entry(devices: &Value) -> DriverConfigEntry {
    DriverConfigEntry::enabled(
        [("devices".to_owned(), devices.clone())]
            .into_iter()
            .collect(),
    )
}

struct Rig {
    id: DeviceId,
    config: Arc<LiveConfig>,
    devices: ConfiguredDevices<String>,
}

impl Rig {
    fn new() -> Self {
        let config = Arc::new(LiveConfig::default());
        config.set(entry(&json!([["strip", "one"]])));
        Self {
            id: device_id("strip"),
            devices: ConfiguredDevices::new(
                "fixture",
                Arc::clone(&config) as Arc<dyn DriverConfigSource>,
                resolve,
            ),
            config,
        }
    }

    fn retarget(&self, target: &str) {
        self.config.set(entry(&json!([["strip", target]])));
    }
}

fn assert_not_found(error: &DeviceError, reason: &str) {
    assert!(matches!(error, DeviceError::NotFound { .. }), "{error:?}");
    assert!(error.to_string().contains(reason), "{error}");
    assert_eq!(error.recoverability(), ErrorRecoverability::Permanent);
}

#[test]
fn resolves_the_plan_the_config_holds_now() {
    let rig = Rig::new();
    let first = rig.devices.current(&rig.id).expect("configured");
    assert_eq!(**first.plan(), "one");

    rig.retarget("two");
    let second = rig.devices.current(&rig.id).expect("still configured");
    assert_eq!(**second.plan(), "two");
    assert_eq!(**first.plan(), "one", "a resolved plan never moves");
}

#[test]
fn a_device_the_config_does_not_name_is_refused() {
    let rig = Rig::new();
    rig.config.set(entry(&json!([])));
    assert_not_found(
        &rig.devices.current(&rig.id).err().expect("removed"),
        "no device in the current config",
    );
}

#[test]
fn a_disabled_driver_authorizes_nothing() {
    let rig = Rig::new();
    let mut disabled = rig.config.current();
    disabled.enabled = false;
    rig.config.set(disabled);
    assert_not_found(
        &rig.devices.current(&rig.id).err().expect("disabled"),
        "the fixture driver is disabled",
    );
}

#[test]
fn an_invalid_config_authorizes_nothing() {
    let rig = Rig::new();
    rig.config.set(entry(&json!("not a device list")));
    assert_not_found(
        &rig.devices.current(&rig.id).err().expect("invalid"),
        "the fixture config is invalid",
    );
}

#[test]
fn a_host_with_no_entry_authorizes_nothing() {
    let rig = Rig::new();
    rig.config.set(DriverConfigEntry::default());
    assert_not_found(
        &rig.devices.current(&rig.id).err().expect("no entry"),
        "no device in the current config",
    );
}

#[test]
fn confirm_passes_while_the_generation_holds() {
    let rig = Rig::new();
    let device = rig.devices.current(&rig.id).expect("configured");
    rig.devices.confirm(&rig.id, &device).expect("unchanged");

    rig.retarget("two");
    rig.retarget("one");
    rig.devices
        .confirm(&rig.id, &device)
        .expect("a config back at the same value still authorizes the plan");
}

#[test]
fn confirm_refuses_an_open_whose_device_was_removed() {
    let rig = Rig::new();
    let device = rig.devices.current(&rig.id).expect("configured");
    rig.config.set(entry(&json!([])));
    assert_not_found(
        &rig.devices.confirm(&rig.id, &device).expect_err("removed"),
        "no device in the current config",
    );
}

#[test]
fn confirm_sends_an_open_that_lost_the_race_back_to_the_lifecycle() {
    let rig = Rig::new();
    let device = rig.devices.current(&rig.id).expect("configured");
    rig.retarget("two");

    let error = rig
        .devices
        .confirm(&rig.id, &device)
        .expect_err("the config moved");
    assert!(
        matches!(error, DeviceError::ConnectionFailed { .. }),
        "{error:?}"
    );
    assert_eq!(error.recoverability(), ErrorRecoverability::Reconnect);
    let retry = rig.devices.current(&rig.id).expect("configured");
    assert_eq!(**retry.plan(), "two", "the retry resolves the new plan");
}
