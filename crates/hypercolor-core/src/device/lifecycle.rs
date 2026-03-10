//! Device lifecycle orchestration and action planning.
//!
//! [`DeviceLifecycleManager`] coordinates per-device state machines and emits
//! high-level actions for async runtime code (daemon/API) to execute.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use crate::types::device::{
    DeviceError, DeviceFingerprint, DeviceHandle, DeviceId, DeviceIdentifier, DeviceInfo,
    DeviceState,
};

use super::discovery::DiscoveryConnectBehavior;
use super::state_machine::{DeviceStateMachine, ReconnectPolicy};

const DEFAULT_MAX_RECONNECT_ATTEMPTS: u32 = 6;

/// Action emitted by [`DeviceLifecycleManager`] for runtime execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LifecycleAction {
    /// Connect a physical device and map it for frame routing.
    Connect {
        device_id: DeviceId,
        backend_id: String,
        layout_device_id: String,
    },

    /// Disconnect a physical device.
    Disconnect {
        device_id: DeviceId,
        backend_id: String,
    },

    /// Explicit map action (kept for future split connect/map runtimes).
    Map {
        layout_device_id: String,
        backend_id: String,
        device_id: DeviceId,
    },

    /// Remove a layout->device mapping.
    Unmap { layout_device_id: String },

    /// Spawn or reschedule reconnect task.
    SpawnReconnect {
        device_id: DeviceId,
        delay: Duration,
    },

    /// Cancel reconnect task if one exists.
    CancelReconnect { device_id: DeviceId },
}

struct ManagedDevice {
    state_machine: DeviceStateMachine,
    backend_id: String,
    layout_device_id: String,
    identifier: DeviceIdentifier,
    connect_behavior: DiscoveryConnectBehavior,
}

/// Pure lifecycle coordinator for discovered devices.
///
/// Owns one [`DeviceStateMachine`] per tracked device and returns actions for
/// async executors. This keeps the manager deterministic and easy to unit test.
pub struct DeviceLifecycleManager {
    devices: HashMap<DeviceId, ManagedDevice>,
    reconnect_scheduled: HashSet<DeviceId>,
    reconnect_policy: ReconnectPolicy,
}

impl Default for DeviceLifecycleManager {
    fn default() -> Self {
        let reconnect_policy = ReconnectPolicy {
            max_attempts: Some(DEFAULT_MAX_RECONNECT_ATTEMPTS),
            ..ReconnectPolicy::default()
        };
        Self {
            devices: HashMap::new(),
            reconnect_scheduled: HashSet::new(),
            reconnect_policy,
        }
    }
}

impl DeviceLifecycleManager {
    /// Create an empty lifecycle manager.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a lifecycle manager with a custom reconnect policy.
    #[must_use]
    pub fn with_reconnect_policy(reconnect_policy: ReconnectPolicy) -> Self {
        Self {
            devices: HashMap::new(),
            reconnect_scheduled: HashSet::new(),
            reconnect_policy,
        }
    }

    /// Number of tracked devices.
    #[must_use]
    pub fn tracked_device_count(&self) -> usize {
        self.devices.len()
    }

    /// Snapshot tracked device IDs.
    #[must_use]
    pub fn tracked_device_ids(&self) -> Vec<DeviceId> {
        self.devices.keys().copied().collect()
    }

    /// Return the current state for a tracked device.
    #[must_use]
    pub fn state(&self, device_id: DeviceId) -> Option<DeviceState> {
        self.devices
            .get(&device_id)
            .map(|managed| managed.state_machine.state().clone())
    }

    /// Return the deterministic layout device ID for a tracked device.
    #[must_use]
    pub fn layout_device_id_for(&self, device_id: DeviceId) -> Option<&str> {
        self.devices
            .get(&device_id)
            .map(|managed| managed.layout_device_id.as_str())
    }

    /// Handle a discovered or reappeared device.
    ///
    /// Creates lifecycle tracking for new devices and emits `Connect` when
    /// the device is in `Known` state.
    pub fn on_discovered(
        &mut self,
        device_id: DeviceId,
        device_info: &DeviceInfo,
        backend_id: &str,
        fingerprint: Option<&DeviceFingerprint>,
    ) -> Vec<LifecycleAction> {
        self.on_discovered_with_behavior(
            device_id,
            device_info,
            backend_id,
            fingerprint,
            DiscoveryConnectBehavior::AutoConnect,
        )
    }

    /// Handle a discovered or reappeared device with explicit connect behavior.
    pub fn on_discovered_with_behavior(
        &mut self,
        device_id: DeviceId,
        device_info: &DeviceInfo,
        backend_id: &str,
        fingerprint: Option<&DeviceFingerprint>,
        connect_behavior: DiscoveryConnectBehavior,
    ) -> Vec<LifecycleAction> {
        let backend_id = backend_id.to_ascii_lowercase();
        let layout_device_id =
            Self::layout_device_id_with_fingerprint(&backend_id, device_info, fingerprint);

        let managed = self.devices.entry(device_id).or_insert_with(|| {
            let identifier = Self::identifier_for_device(&backend_id, device_info, fingerprint);
            ManagedDevice {
                state_machine: DeviceStateMachine::with_policy(
                    identifier.clone(),
                    self.reconnect_policy.clone(),
                ),
                backend_id: backend_id.clone(),
                layout_device_id: layout_device_id.clone(),
                identifier,
                connect_behavior,
            }
        });

        managed.backend_id = backend_id;
        managed.layout_device_id = layout_device_id;
        managed.connect_behavior = connect_behavior;

        let mut actions = Vec::new();
        if self.reconnect_scheduled.remove(&device_id) {
            actions.push(LifecycleAction::CancelReconnect { device_id });
        }

        if *managed.state_machine.state() == DeviceState::Known
            && managed.connect_behavior.should_auto_connect()
        {
            actions.push(Self::connect_action(device_id, managed));
        }

        actions
    }

    /// Transition a device to connected after a successful backend connect.
    pub fn on_connected(
        &mut self,
        device_id: DeviceId,
    ) -> Result<Vec<LifecycleAction>, DeviceError> {
        let reconnect_canceled = self.reconnect_scheduled.remove(&device_id);
        let managed = self.managed_mut(device_id)?;

        let handle = DeviceHandle::new(managed.identifier.clone(), managed.backend_id.clone());
        managed.state_machine.on_connected(handle)?;

        let mut actions = Vec::new();
        if reconnect_canceled {
            actions.push(LifecycleAction::CancelReconnect { device_id });
        }
        Ok(actions)
    }

    /// Handle backend connect failure by entering reconnect mode.
    pub fn on_connect_failed(
        &mut self,
        device_id: DeviceId,
    ) -> Result<Vec<LifecycleAction>, DeviceError> {
        let delay = {
            let managed = self.managed_mut(device_id)?;
            managed.state_machine.on_connect_failed()?
        };
        self.reconnect_scheduled.insert(device_id);
        Ok(vec![LifecycleAction::SpawnReconnect { device_id, delay }])
    }

    /// Mark that at least one frame was successfully written.
    pub fn on_frame_success(&mut self, device_id: DeviceId) -> Result<(), DeviceError> {
        let managed = self.managed_mut(device_id)?;
        managed.state_machine.on_frame_success()
    }

    /// Handle a communication failure and schedule reconnect.
    pub fn on_comm_error(
        &mut self,
        device_id: DeviceId,
    ) -> Result<Vec<LifecycleAction>, DeviceError> {
        let (disconnect_action, unmap_action, next_retry_delay) = {
            let managed = self.managed_mut(device_id)?;
            managed.state_machine.on_comm_error()?;
            (
                Self::disconnect_action(device_id, managed),
                LifecycleAction::Unmap {
                    layout_device_id: managed.layout_device_id.clone(),
                },
                managed
                    .state_machine
                    .reconnect_status()
                    .map(|status| status.next_retry),
            )
        };

        let mut actions = vec![disconnect_action, unmap_action];

        if let Some(delay) = next_retry_delay {
            self.reconnect_scheduled.insert(device_id);
            actions.push(LifecycleAction::SpawnReconnect { device_id, delay });
        }

        Ok(actions)
    }

    /// Build a reconnect connect action after retry delay elapses.
    #[must_use]
    pub fn on_reconnect_attempt(&self, device_id: DeviceId) -> Option<LifecycleAction> {
        let managed = self.devices.get(&device_id)?;
        if *managed.state_machine.state() != DeviceState::Reconnecting {
            return None;
        }
        Some(Self::connect_action(device_id, managed))
    }

    /// Update reconnect backoff after a failed retry attempt.
    pub fn on_reconnect_failed(
        &mut self,
        device_id: DeviceId,
    ) -> Result<Vec<LifecycleAction>, DeviceError> {
        let next_delay = {
            let managed = self.managed_mut(device_id)?;
            managed.state_machine.on_reconnect_failed()
        };

        let mut actions = Vec::new();
        match next_delay {
            Some(delay) => {
                self.reconnect_scheduled.insert(device_id);
                actions.push(LifecycleAction::SpawnReconnect { device_id, delay });
            }
            None => {
                if self.reconnect_scheduled.remove(&device_id) {
                    actions.push(LifecycleAction::CancelReconnect { device_id });
                }
            }
        }

        Ok(actions)
    }

    /// Handle disappearance from discovery scan or hot-unplug event.
    pub fn on_device_vanished(&mut self, device_id: DeviceId) -> Vec<LifecycleAction> {
        let Some(managed) = self.devices.get_mut(&device_id) else {
            return Vec::new();
        };

        let previous = managed.state_machine.state().clone();
        managed.state_machine.on_hot_unplug();

        let mut actions = vec![LifecycleAction::Unmap {
            layout_device_id: managed.layout_device_id.clone(),
        }];

        if previous.is_renderable() {
            actions.insert(0, Self::disconnect_action(device_id, managed));
        }

        if self.reconnect_scheduled.remove(&device_id) {
            actions.push(LifecycleAction::CancelReconnect { device_id });
        }

        actions
    }

    /// User-driven disable transition.
    pub fn on_user_disable(
        &mut self,
        device_id: DeviceId,
    ) -> Result<Vec<LifecycleAction>, DeviceError> {
        let reconnect_canceled = self.reconnect_scheduled.remove(&device_id);
        let managed = self.managed_mut(device_id)?;
        let previous = managed.state_machine.state().clone();
        managed.state_machine.on_user_disable();

        let mut actions = vec![LifecycleAction::Unmap {
            layout_device_id: managed.layout_device_id.clone(),
        }];
        if previous.is_renderable() {
            actions.insert(0, Self::disconnect_action(device_id, managed));
        }
        if reconnect_canceled {
            actions.push(LifecycleAction::CancelReconnect { device_id });
        }
        Ok(actions)
    }

    /// User-driven enable transition.
    pub fn on_user_enable(
        &mut self,
        device_id: DeviceId,
    ) -> Result<Vec<LifecycleAction>, DeviceError> {
        let managed = self.managed_mut(device_id)?;
        let was_disabled = *managed.state_machine.state() == DeviceState::Disabled;
        managed.state_machine.on_user_enable();

        let mut actions = Vec::new();
        if was_disabled
            && *managed.state_machine.state() == DeviceState::Known
            && managed.connect_behavior.should_auto_connect()
        {
            actions.push(Self::connect_action(device_id, managed));
        }
        Ok(actions)
    }

    /// Derive a deterministic layout device ID from backend and device metadata.
    ///
    /// Fallback format: `<backend>:<normalized_name>`.
    #[must_use]
    pub fn layout_device_id(backend_id: &str, device_info: &DeviceInfo) -> String {
        let backend = backend_id.trim().to_ascii_lowercase();
        let name = sanitize_component(&device_info.name);
        format!("{backend}:{name}")
    }

    fn managed_mut(&mut self, device_id: DeviceId) -> Result<&mut ManagedDevice, DeviceError> {
        self.devices
            .get_mut(&device_id)
            .ok_or_else(|| DeviceError::NotFound {
                device: device_id.to_string(),
            })
    }

    fn connect_action(device_id: DeviceId, managed: &ManagedDevice) -> LifecycleAction {
        LifecycleAction::Connect {
            device_id,
            backend_id: managed.backend_id.clone(),
            layout_device_id: managed.layout_device_id.clone(),
        }
    }

    fn disconnect_action(device_id: DeviceId, managed: &ManagedDevice) -> LifecycleAction {
        LifecycleAction::Disconnect {
            device_id,
            backend_id: managed.backend_id.clone(),
        }
    }

    fn layout_device_id_with_fingerprint(
        backend_id: &str,
        device_info: &DeviceInfo,
        fingerprint: Option<&DeviceFingerprint>,
    ) -> String {
        let Some(fingerprint) = fingerprint else {
            return Self::layout_device_id(backend_id, device_info);
        };
        let value = fingerprint.0.to_ascii_lowercase();

        if backend_id == "wled" {
            if let Some(value) = value.strip_prefix("net:") {
                if let Some(hostname) = value.strip_prefix("wled:") {
                    return format!("wled:{}", sanitize_component(hostname));
                }
                return format!("wled:{value}");
            }
        }

        if let Some(value) = value.strip_prefix("usb:") {
            return format!("usb:{}", sanitize_component(value));
        }

        if let Some(value) = value.strip_prefix("smbus:") {
            return format!("smbus:{}", sanitize_component(value));
        }

        let backend_prefix = format!("{backend_id}:");
        if let Some(value) = value.strip_prefix(&backend_prefix) {
            return format!("{backend_id}:{}", sanitize_component(value));
        }

        Self::layout_device_id(backend_id, device_info)
    }

    fn identifier_for_device(
        backend_id: &str,
        device_info: &DeviceInfo,
        fingerprint: Option<&DeviceFingerprint>,
    ) -> DeviceIdentifier {
        if let Some(fingerprint) = fingerprint {
            let value = fingerprint.0.clone();
            if backend_id == "smbus"
                && let Some(rest) = value.strip_prefix("smbus:")
            {
                let (bus_path, address) = rest.rsplit_once(':').map_or((rest, 0), |(bus, raw)| {
                    let address = u16::from_str_radix(raw, 16).unwrap_or(0);
                    (bus, address)
                });
                return DeviceIdentifier::SmBus {
                    bus_path: bus_path.to_owned(),
                    address,
                };
            }
            if backend_id == "wled"
                && let Some(rest) = value.strip_prefix("net:")
            {
                let mdns_hostname = rest
                    .strip_prefix("wled:")
                    .map(ToOwned::to_owned)
                    .or_else(|| Some(device_info.name.clone()));
                return DeviceIdentifier::Network {
                    mac_address: rest.to_owned(),
                    last_ip: None,
                    mdns_hostname,
                };
            }
        }

        DeviceIdentifier::Network {
            mac_address: format!("{backend_id}:{}", device_info.id),
            last_ip: None,
            mdns_hostname: Some(device_info.name.clone()),
        }
    }
}

fn sanitize_component(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut prev_was_dash = false;

    for ch in input.trim().chars() {
        let mapped = if ch.is_ascii_alphanumeric() || ch == ':' || ch == '_' || ch == '-' {
            ch.to_ascii_lowercase()
        } else {
            '-'
        };

        if mapped == '-' {
            if prev_was_dash {
                continue;
            }
            prev_was_dash = true;
            out.push(mapped);
        } else {
            prev_was_dash = false;
            out.push(mapped);
        }
    }

    if out.ends_with('-') {
        out.pop();
    }

    if out.is_empty() {
        "device".to_owned()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::{DeviceLifecycleManager, LifecycleAction, ReconnectPolicy};
    use crate::types::device::{
        ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFamily, DeviceFingerprint,
        DeviceId, DeviceInfo, DeviceState, DeviceTopologyHint, ZoneInfo,
    };
    use std::time::Duration;

    fn device_info(name: &str, family: DeviceFamily) -> DeviceInfo {
        DeviceInfo {
            id: DeviceId::new(),
            name: name.to_owned(),
            vendor: "TestVendor".to_owned(),
            family,
            model: None,
            connection_type: ConnectionType::Network,
            zones: vec![ZoneInfo {
                name: "Main".to_owned(),
                led_count: 16,
                topology: DeviceTopologyHint::Strip,
                color_format: DeviceColorFormat::Rgb,
            }],
            firmware_version: None,
            capabilities: DeviceCapabilities::default(),
        }
    }

    #[test]
    fn discovered_known_device_requests_connect() {
        let mut lifecycle = DeviceLifecycleManager::new();
        let info = device_info("Case Strip", DeviceFamily::Wled);
        let actions = lifecycle.on_discovered(
            info.id,
            &info,
            "wled",
            Some(&DeviceFingerprint("net:aa:bb:cc:dd:ee:ff".to_owned())),
        );

        assert_eq!(actions.len(), 1);
        assert!(matches!(
            &actions[0],
            LifecycleAction::Connect {
                backend_id,
                layout_device_id,
                ..
            } if backend_id == "wled" && layout_device_id == "wled:aa:bb:cc:dd:ee:ff"
        ));
    }

    #[test]
    fn comm_error_emits_disconnect_unmap_and_reconnect() {
        let mut lifecycle = DeviceLifecycleManager::new();
        let info = device_info("Desk Strip", DeviceFamily::Wled);
        lifecycle.on_discovered(info.id, &info, "wled", None);
        lifecycle
            .on_connected(info.id)
            .expect("connect transition should work");
        lifecycle
            .on_frame_success(info.id)
            .expect("frame success should transition active");

        let actions = lifecycle
            .on_comm_error(info.id)
            .expect("comm error should transition");

        assert!(
            actions
                .iter()
                .any(|action| matches!(action, LifecycleAction::Disconnect { .. }))
        );
        assert!(
            actions
                .iter()
                .any(|action| matches!(action, LifecycleAction::Unmap { .. }))
        );
        assert!(actions.iter().any(|action| matches!(
            action,
            LifecycleAction::SpawnReconnect { delay, .. }
            if *delay == Duration::from_secs(1)
        )));
        assert_eq!(lifecycle.state(info.id), Some(DeviceState::Reconnecting));
    }

    #[test]
    fn connect_failure_schedules_reconnect() {
        let mut lifecycle = DeviceLifecycleManager::new();
        let info = device_info("Unreachable Device", DeviceFamily::Wled);
        lifecycle.on_discovered(info.id, &info, "wled", None);

        let actions = lifecycle
            .on_connect_failed(info.id)
            .expect("connect failure should be tracked");
        assert!(actions.iter().any(|action| matches!(
            action,
            LifecycleAction::SpawnReconnect { delay, .. }
            if *delay == Duration::from_secs(1)
        )));
        assert_eq!(lifecycle.state(info.id), Some(DeviceState::Reconnecting));
    }

    #[test]
    fn reconnect_failure_reschedules_and_exhaustion_cancels() {
        let mut lifecycle = DeviceLifecycleManager::with_reconnect_policy(ReconnectPolicy {
            initial_delay: Duration::from_millis(10),
            max_delay: Duration::from_millis(20),
            backoff_factor: 2.0,
            max_attempts: Some(2),
            jitter: 0.0,
        });
        let info = device_info("Kitchen Strip", DeviceFamily::Wled);
        lifecycle.on_discovered(
            info.id,
            &info,
            "wled",
            Some(&DeviceFingerprint("net:wled:office-strip".to_owned())),
        );
        lifecycle
            .on_connected(info.id)
            .expect("connect transition should work");
        lifecycle
            .on_comm_error(info.id)
            .expect("comm error should transition");

        let actions = lifecycle
            .on_reconnect_failed(info.id)
            .expect("first reconnect failure should schedule");
        assert!(
            actions
                .iter()
                .any(|action| matches!(action, LifecycleAction::SpawnReconnect { .. }))
        );

        let exhaustion_actions = lifecycle
            .on_reconnect_failed(info.id)
            .expect("second reconnect failure should exhaust");
        assert!(
            exhaustion_actions
                .iter()
                .any(|action| matches!(action, LifecycleAction::CancelReconnect { .. }))
        );

        assert_eq!(lifecycle.state(info.id), Some(DeviceState::Known));
    }

    #[test]
    fn default_policy_eventually_exhausts_reconnects() {
        let mut lifecycle = DeviceLifecycleManager::new();
        let info = device_info("Default Policy Device", DeviceFamily::Wled);
        lifecycle.on_discovered(info.id, &info, "wled", None);

        lifecycle
            .on_connect_failed(info.id)
            .expect("connect failure should transition to reconnecting");
        assert_eq!(lifecycle.state(info.id), Some(DeviceState::Reconnecting));

        for _ in 0..32 {
            lifecycle
                .on_reconnect_failed(info.id)
                .expect("reconnect failure update should succeed");
            if lifecycle.state(info.id) == Some(DeviceState::Known) {
                break;
            }
        }

        assert_eq!(
            lifecycle.state(info.id),
            Some(DeviceState::Known),
            "default reconnect policy should stop retrying after bounded attempts"
        );
    }

    #[test]
    fn disable_then_enable_reconnects_known_device() {
        let mut lifecycle = DeviceLifecycleManager::new();
        let info = device_info("Panel", DeviceFamily::Wled);
        lifecycle.on_discovered(info.id, &info, "wled", None);
        lifecycle
            .on_connected(info.id)
            .expect("connect transition should work");

        let disable_actions = lifecycle
            .on_user_disable(info.id)
            .expect("disable should be valid");
        assert!(
            disable_actions
                .iter()
                .any(|action| matches!(action, LifecycleAction::Disconnect { .. }))
        );
        assert_eq!(lifecycle.state(info.id), Some(DeviceState::Disabled));

        let enable_actions = lifecycle
            .on_user_enable(info.id)
            .expect("enable should be valid");
        assert!(
            enable_actions
                .iter()
                .any(|action| matches!(action, LifecycleAction::Connect { .. }))
        );
        assert_eq!(lifecycle.state(info.id), Some(DeviceState::Known));
    }

    #[test]
    fn vanished_active_device_requests_disconnect_and_unmap() {
        let mut lifecycle = DeviceLifecycleManager::new();
        let info = device_info("Vanishing Strip", DeviceFamily::Wled);
        lifecycle.on_discovered(info.id, &info, "wled", None);
        lifecycle
            .on_connected(info.id)
            .expect("connect transition should work");
        lifecycle
            .on_frame_success(info.id)
            .expect("frame success should transition active");

        let actions = lifecycle.on_device_vanished(info.id);
        assert!(
            actions
                .iter()
                .any(|action| matches!(action, LifecycleAction::Disconnect { .. }))
        );
        assert!(
            actions
                .iter()
                .any(|action| matches!(action, LifecycleAction::Unmap { .. }))
        );
        assert_eq!(lifecycle.state(info.id), Some(DeviceState::Known));
    }

    #[test]
    fn layout_id_falls_back_to_backend_prefix_and_normalized_name() {
        let info = device_info("My Test Device", DeviceFamily::Custom("Mock".to_owned()));
        let layout_id = DeviceLifecycleManager::layout_device_id("mock", &info);
        assert_eq!(layout_id, "mock:my-test-device");
    }
}
