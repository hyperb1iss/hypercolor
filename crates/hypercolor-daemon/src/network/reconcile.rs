//! Live driver output-backend reconciliation.
//!
//! `register_enabled_device_backends` runs once at startup, so flipping
//! `drivers.<id>.enabled` used to give discovery but no output until the
//! daemon restarted, while `requires_restart` reported false. The
//! reconciler closes that gap: given the current config it registers every
//! output provider the enabled driver set now selects and, for providers
//! the set no longer selects, disconnects their devices and unregisters
//! the backend. Devices of a disabled driver whose shared provider stays
//! up (one USB family among several) are disconnected the same way.

use std::collections::{BTreeSet, HashSet};

use anyhow::{Context, Result};
use hypercolor_driver_api::{DriverConfigView, DriverHost, OutputBinding};
use hypercolor_network::DriverModuleRegistry;
use hypercolor_types::config::HypercolorConfig;
use hypercolor_types::device::DeviceId;
use hypercolor_types::event::{DisconnectReason, HypercolorEvent};
use tracing::{debug, info, warn};

use crate::discovery::{DiscoveryRuntime, execute_lifecycle_actions, sync_registry_state};

/// What one reconciliation pass changed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DriverBackendReconcileReport {
    /// Output backends registered by this pass, by backend id.
    pub registered: Vec<String>,
    /// Driver ids whose output provider was newly registered; discovery
    /// for them is worth scheduling so their devices appear without
    /// waiting for the periodic scan.
    pub registered_driver_ids: Vec<String>,
    /// Output backends unregistered by this pass, by backend id.
    pub unregistered: Vec<String>,
    /// Devices disconnected because their driver or backend went away.
    pub disconnected_devices: Vec<DeviceId>,
}

impl DriverBackendReconcileReport {
    /// Whether the pass changed anything.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.registered.is_empty()
            && self.unregistered.is_empty()
            && self.disconnected_devices.is_empty()
    }
}

/// Whether a `ConfigChanged` key can change which drivers are enabled.
///
/// An empty key is a whole-document write and always qualifies.
#[must_use]
pub fn config_key_touches_drivers(key: &str) -> bool {
    key.is_empty() || key == "drivers" || key.starts_with("drivers.")
}

/// Bring registered output backends in line with `config`.
///
/// # Errors
///
/// Returns an error when the enabled driver set produces invalid output
/// bindings or a newly selected provider fails to build.
pub async fn reconcile_driver_output_backends(
    runtime: &DiscoveryRuntime,
    registry: &DriverModuleRegistry,
    host: &dyn DriverHost,
    config: &HypercolorConfig,
    previous: Option<&HypercolorConfig>,
) -> Result<DriverBackendReconcileReport> {
    let mut report = DriverBackendReconcileReport::default();
    let enabled_driver_ids = super::enabled_driver_module_ids(registry, config);
    let finalized = registry
        .finalize_output_bindings(&enabled_driver_ids)
        .context("failed to finalize driver output bindings")?;
    let desired: Vec<_> = finalized.providers().iter().collect();
    let desired_ids: BTreeSet<String> = desired
        .iter()
        .map(|provider| provider.backend_id().to_string())
        .collect();
    let driver_owned_ids = driver_owned_backend_ids(registry);
    let registered_ids: BTreeSet<String> = {
        let manager = runtime.backend_manager.lock().await;
        manager
            .backend_ids()
            .into_iter()
            .map(ToOwned::to_owned)
            .collect()
    };

    // Devices lose their route when their backend goes away or when their
    // own driver is disabled while a shared provider keeps running.
    let changed_backends: BTreeSet<String> = desired
        .iter()
        .filter_map(|provider| {
            let previous = previous?;
            let driver_id = provider.driver_id();
            let changed = super::driver_config_entry(previous, driver_id)
                != super::driver_config_entry(config, driver_id);
            (changed && registered_ids.contains(provider.backend_id().as_str()))
                .then(|| provider.backend_id().to_string())
        })
        .collect();
    let retiring_backends: BTreeSet<String> = registered_ids
        .iter()
        .filter(|id| {
            driver_owned_ids.contains(*id)
                && (!desired_ids.contains(*id) || changed_backends.contains(*id))
        })
        .cloned()
        .collect();
    let registered_driver_ids: HashSet<String> = registry.ids().into_iter().collect();
    let disabled_driver_ids: HashSet<String> = registered_driver_ids
        .iter()
        .filter(|id| !enabled_driver_ids.contains(*id))
        .cloned()
        .collect();
    // Construct replacements before retiring any working output routes.
    let mut prepared = Vec::new();
    for provider in desired {
        let backend_id = provider.backend_id().to_string();
        if registered_ids.contains(&backend_id) && !changed_backends.contains(&backend_id) {
            continue;
        }
        let provider_driver_id = provider.driver_id();
        let config_entry = super::driver_config_entry(config, provider_driver_id);
        let backend = provider
            .build(
                host,
                DriverConfigView {
                    driver_id: provider_driver_id,
                    entry: &config_entry,
                },
            )
            .with_context(|| {
                format!("failed to build output backend for driver '{provider_driver_id}'")
            })?;
        prepared.push((backend_id, provider_driver_id.to_owned(), backend));
    }

    let stranded_devices: Vec<DeviceId> = runtime
        .device_registry
        .list()
        .await
        .into_iter()
        .filter(|tracked| {
            retiring_backends.contains(tracked.info.output_backend_id())
                || disabled_driver_ids.contains(tracked.info.driver_id())
        })
        .filter(|tracked| {
            tracked.state.is_renderable()
                || tracked.state == hypercolor_types::device::DeviceState::Reconnecting
        })
        .map(|tracked| tracked.info.id)
        .collect();
    for device_id in stranded_devices {
        let actions = {
            let mut lifecycle = runtime.lifecycle_manager.lock().await;
            lifecycle.on_runtime_deactivate(device_id)
        };
        match actions {
            Ok(actions) => {
                execute_lifecycle_actions(runtime.clone(), actions).await;
                sync_registry_state(runtime, device_id).await;
                runtime
                    .event_bus
                    .publish(HypercolorEvent::DeviceDisconnected {
                        device_id: device_id.to_string(),
                        reason: DisconnectReason::User,
                        will_retry: false,
                    });
                report.disconnected_devices.push(device_id);
            }
            Err(error) => warn!(
                device_id = %device_id,
                error = %error,
                "driver reconciler could not deactivate a stranded device"
            ),
        }
    }

    {
        let mut manager = runtime.backend_manager.lock().await;
        for backend_id in &retiring_backends {
            if manager.unregister_backend(backend_id).is_some() {
                info!(backend_id = %backend_id, "unregistered output backend for disabled driver");
                report.unregistered.push(backend_id.clone());
            }
        }
        for (backend_id, provider_driver_id, backend) in prepared {
            manager.register_backend(backend);
            info!(backend_id = %backend_id, driver_id = %provider_driver_id,
                "registered output backend for enabled driver");
            report.registered.push(backend_id);
            report.registered_driver_ids.push(provider_driver_id);
        }
    }

    crate::discovery::release_disabled_driver_ownership(runtime, &disabled_driver_ids).await;

    runtime
        .unclaimed_devices
        .set_enabled_driver_ids(Some(enabled_driver_ids));

    if !report.disconnected_devices.is_empty() {
        runtime
            .layout
            .sync_active_layout_for_renderable_devices(runtime.clone(), None)
            .await;
    }
    if report.is_empty() {
        debug!("driver output backends already match the enabled driver set");
    }
    Ok(report)
}

/// Backend ids that belong to a registered driver's own output binding.
///
/// Only these may be unregistered: the simulator and mock backends are
/// registered by the daemon itself and never follow a driver flag.
fn driver_owned_backend_ids(registry: &DriverModuleRegistry) -> BTreeSet<String> {
    registry
        .ids()
        .into_iter()
        .filter_map(|id| registry.get(&id))
        .filter_map(|driver| match driver.output() {
            OutputBinding::Owned { id, .. } => Some(id.to_string()),
            OutputBinding::Shared(_) | OutputBinding::None => None,
        })
        .collect()
}
