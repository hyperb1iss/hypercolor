//! Native-first conflict guard for bridge routes.
//!
//! Ownership is decided per physical device: when a native driver renders
//! a device and the bridge also holds an output-enabled route to the same
//! silicon, the bridge route is output-disabled with a reason and stops
//! rendering. Native never yields automatically. The user hands a device
//! to the bridge by disabling it natively, at which point the guard lifts
//! its own lock so the bridge route can connect through the normal
//! layout-driven path.
//!
//! The guard runs after every discovery pass and whenever a bridge device
//! connects. Its locks live in the daemon, not in the bridge driver's
//! metadata, so a rediscovery that rewrites metadata cannot silently
//! re-enable a route the guard turned off.

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};

use hypercolor_types::device::{DeviceId, DeviceState};
use hypercolor_types::event::HypercolorEvent;
use tracing::{info, warn};

use super::DiscoveryRuntime;
use super::coverage::{JoinedCoverageRow, collect_device_coverage, is_openrgb_bridge_device};
use super::device_helpers::sync_registry_state;
use super::lifecycle::execute_lifecycle_actions;

/// Prefix of every reason the guard writes; the driver id follows in
/// parentheses.
pub const NATIVE_OWNER_REASON_PREFIX: &str = "native driver owns this device";

/// The disable reason for a bridge route shadowed by `driver_id`.
#[must_use]
pub fn native_owner_reason(driver_id: &str) -> String {
    format!("{NATIVE_OWNER_REASON_PREFIX} ({driver_id})")
}

/// Why one bridge route is output-disabled by the daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BridgeOutputLock {
    /// Human-readable reason, always built by [`native_owner_reason`].
    pub reason: String,
    /// The native device that owns the hardware.
    pub native_device_id: DeviceId,
    /// The native driver rendering it.
    pub native_driver_id: String,
}

/// Every bridge route the guard currently holds output-disabled.
///
/// Cloning shares the same map, so the discovery runtime, the API, and the
/// diagnostics all read one set of locks.
#[derive(Debug, Clone, Default)]
pub struct BridgeOutputLocks {
    inner: Arc<StdMutex<HashMap<DeviceId, BridgeOutputLock>>>,
}

impl BridgeOutputLocks {
    /// The lock held for one bridge device, if any.
    #[must_use]
    pub fn get(&self, device_id: &DeviceId) -> Option<BridgeOutputLock> {
        self.lock().get(device_id).cloned()
    }

    /// Whether the guard holds a lock for one bridge device.
    #[must_use]
    pub fn is_locked(&self, device_id: &DeviceId) -> bool {
        self.lock().contains_key(device_id)
    }

    /// Every held lock keyed by bridge device.
    #[must_use]
    pub fn snapshot(&self) -> HashMap<DeviceId, BridgeOutputLock> {
        self.lock().clone()
    }

    /// Hold a lock, replacing any previous one for the same device.
    pub fn insert(&self, device_id: DeviceId, lock: BridgeOutputLock) {
        self.lock().insert(device_id, lock);
    }

    /// Release a lock; returns what was held.
    pub fn remove(&self, device_id: &DeviceId) -> Option<BridgeOutputLock> {
        self.lock().remove(device_id)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<DeviceId, BridgeOutputLock>> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// One change the guard intends to make.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuardDecision {
    /// Output-disable a bridge route because a native driver renders the
    /// same hardware.
    Lock {
        bridge_device_id: DeviceId,
        native_device_id: DeviceId,
        native_driver_id: String,
    },
    /// Release a lock because the native side was disabled or removed.
    Unlock { bridge_device_id: DeviceId },
}

/// Decide, from joined coverage rows, which bridge routes to lock or unlock.
///
/// Lock when the row has a renderable native device and a bridge route the
/// daemon does not already hold and the bridge still advertises as
/// writable. Unlock a held route only when the native device is gone or
/// the user disabled it; a native device that is merely reconnecting keeps
/// its claim, so the bridge does not grab the hardware mid-blip.
#[must_use]
pub fn plan_conflict_guard(rows: &[JoinedCoverageRow]) -> Vec<GuardDecision> {
    let mut decisions = Vec::new();
    for row in rows {
        let Some(bridge) = &row.bridge else {
            continue;
        };
        match (&row.native, &bridge.lock) {
            (Some(native), None)
                if native.state.is_renderable() && bridge.advertised_output_enabled =>
            {
                decisions.push(GuardDecision::Lock {
                    bridge_device_id: bridge.device_id,
                    native_device_id: native.device_id,
                    native_driver_id: native.driver_id.clone(),
                });
            }
            (None, Some(_)) => decisions.push(GuardDecision::Unlock {
                bridge_device_id: bridge.device_id,
            }),
            (Some(native), Some(_)) if native.state == DeviceState::Disabled => {
                decisions.push(GuardDecision::Unlock {
                    bridge_device_id: bridge.device_id,
                });
            }
            _ => {}
        }
    }
    decisions
}

/// What one guard pass changed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConflictGuardReport {
    /// Bridge devices newly output-disabled.
    pub locked: Vec<DeviceId>,
    /// Bridge devices released back to normal connect gating.
    pub unlocked: Vec<DeviceId>,
}

/// Run the guard over every coverage row and apply its decisions.
pub async fn enforce_native_ownership(runtime: &DiscoveryRuntime) -> ConflictGuardReport {
    let rows = collect_device_coverage(runtime).await;
    let decisions = plan_conflict_guard(&rows);
    apply_guard_decisions(runtime, decisions).await
}

/// Run the guard when one device changed, skipping the pass entirely for
/// native devices: only a bridge route connecting can create a conflict
/// the discovery-pass guard has not already seen.
pub async fn enforce_native_ownership_for_device(
    runtime: &DiscoveryRuntime,
    device_id: DeviceId,
) -> ConflictGuardReport {
    let Some(tracked) = runtime.device_registry.get(&device_id).await else {
        return ConflictGuardReport::default();
    };
    let metadata = runtime
        .device_registry
        .metadata_for_id(&device_id)
        .await
        .unwrap_or_default();
    if !is_openrgb_bridge_device(&tracked.info, &metadata) {
        return ConflictGuardReport::default();
    }
    enforce_native_ownership(runtime).await
}

async fn apply_guard_decisions(
    runtime: &DiscoveryRuntime,
    decisions: Vec<GuardDecision>,
) -> ConflictGuardReport {
    let mut report = ConflictGuardReport::default();
    for decision in decisions {
        match decision {
            GuardDecision::Lock {
                bridge_device_id,
                native_device_id,
                native_driver_id,
            } => {
                let reason = native_owner_reason(&native_driver_id);
                runtime.bridge_output_locks.insert(
                    bridge_device_id,
                    BridgeOutputLock {
                        reason: reason.clone(),
                        native_device_id,
                        native_driver_id: native_driver_id.clone(),
                    },
                );
                let actions = {
                    let mut lifecycle = runtime.lifecycle_manager.lock().await;
                    lifecycle.on_user_disable(bridge_device_id)
                };
                match actions {
                    Ok(actions) => {
                        execute_lifecycle_actions(runtime.clone(), actions).await;
                        sync_registry_state(runtime, bridge_device_id).await;
                    }
                    Err(error) => warn!(
                        device_id = %bridge_device_id,
                        error = %error,
                        "conflict guard could not disable the bridge route through lifecycle"
                    ),
                }
                publish_output_lock_change(runtime, bridge_device_id, Some(&reason));
                info!(
                    bridge_device_id = %bridge_device_id,
                    native_device_id = %native_device_id,
                    native_driver_id = %native_driver_id,
                    "conflict guard output-disabled a bridge route shadowed by a native driver"
                );
                report.locked.push(bridge_device_id);
            }
            GuardDecision::Unlock { bridge_device_id } => {
                if runtime
                    .bridge_output_locks
                    .remove(&bridge_device_id)
                    .is_none()
                {
                    continue;
                }
                let user_enabled = runtime
                    .device_registry
                    .get(&bridge_device_id)
                    .await
                    .is_some_and(|tracked| tracked.user_settings.enabled);
                if user_enabled {
                    let actions = {
                        let mut lifecycle = runtime.lifecycle_manager.lock().await;
                        lifecycle.on_user_enable(bridge_device_id)
                    };
                    match actions {
                        Ok(actions) => {
                            execute_lifecycle_actions(runtime.clone(), actions).await;
                            sync_registry_state(runtime, bridge_device_id).await;
                        }
                        Err(error) => warn!(
                            device_id = %bridge_device_id,
                            error = %error,
                            "conflict guard could not re-enable the bridge route through lifecycle"
                        ),
                    }
                }
                publish_output_lock_change(runtime, bridge_device_id, None);
                info!(
                    bridge_device_id = %bridge_device_id,
                    "conflict guard released a bridge route; native side disabled or gone"
                );
                report.unlocked.push(bridge_device_id);
            }
        }
    }

    if !report.locked.is_empty() || !report.unlocked.is_empty() {
        runtime
            .layout
            .sync_active_layout_for_renderable_devices(runtime.clone(), None)
            .await;
    }
    report
}

fn publish_output_lock_change(
    runtime: &DiscoveryRuntime,
    device_id: DeviceId,
    reason: Option<&str>,
) {
    let mut changes = HashMap::new();
    changes.insert(
        "output_enabled".to_owned(),
        serde_json::Value::Bool(reason.is_none()),
    );
    changes.insert(
        "disabled_reason".to_owned(),
        reason.map_or(serde_json::Value::Null, |reason| {
            serde_json::Value::String(reason.to_owned())
        }),
    );
    runtime
        .event_bus
        .publish(HypercolorEvent::DeviceStateChanged {
            device_id: device_id.to_string(),
            changes,
        });
}
