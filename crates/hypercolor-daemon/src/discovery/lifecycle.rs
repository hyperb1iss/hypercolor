use std::collections::{HashSet, VecDeque};
use std::time::Duration;

use hypercolor_core::device::{
    AsyncWriteFailure, DeviceLifecycleManager, DiscoveryConnectBehavior, LifecycleAction,
};
use hypercolor_types::device::{DeviceError, DeviceId, DeviceState};
use hypercolor_types::event::{DisconnectReason, HypercolorEvent};
use tracing::{debug, warn};

use super::DiscoveryRuntime;
use super::auto_layout::sync_active_layout_for_renderable_devices;
use super::device_helpers::{
    active_layout_targets_enabled_device, connect_backend_device, desired_connect_behavior,
    device_log_label, disconnect_backend_device, ensure_default_logical_for_device,
    format_error_chain, publish_device_connected, refresh_connected_device_info,
    sync_logical_mappings_for_device, sync_registry_state,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserEnabledStateResult {
    /// Lifecycle transition ran and registry state was synced.
    Applied,
    /// Device exists in the registry but has no lifecycle entry to drive.
    MissingLifecycle,
}

/// Apply a user-requested enabled/disabled state transition to a tracked device.
///
/// This routes through the lifecycle executor so disable operations disconnect
/// hardware and tear down routing instead of only flipping registry state.
pub async fn apply_user_enabled_state(
    runtime: &DiscoveryRuntime,
    device_id: DeviceId,
    enabled: bool,
) -> anyhow::Result<UserEnabledStateResult> {
    let should_activate = if enabled {
        let Some(tracked) = runtime.device_registry.get(&device_id).await else {
            return Ok(UserEnabledStateResult::MissingLifecycle);
        };
        let backend = tracked.info.output_backend_id();
        let fingerprint = runtime.device_registry.fingerprint_for_id(&device_id).await;

        desired_connect_behavior(
            runtime,
            device_id,
            &tracked.info,
            backend,
            fingerprint.as_ref(),
            tracked.connect_behavior,
            true,
        )
        .await
        .should_auto_connect()
    } else {
        false
    };

    let actions = {
        let mut lifecycle = runtime.lifecycle_manager.lock().await;
        let mut transition = if enabled {
            lifecycle.on_user_enable(device_id)
        } else {
            lifecycle.on_user_disable(device_id)
        };

        if enabled
            && !should_activate
            && let Ok(actions) = transition.as_mut()
        {
            actions.clear();
        }

        match transition {
            Ok(actions) => actions,
            Err(DeviceError::NotFound { .. }) => {
                return Ok(UserEnabledStateResult::MissingLifecycle);
            }
            Err(error) => return Err(error.into()),
        }
    };

    execute_lifecycle_actions(runtime.clone(), actions).await;
    sync_registry_state(runtime, device_id).await;

    if !enabled {
        sync_active_layout_for_renderable_devices(runtime, None).await;
    }

    Ok(UserEnabledStateResult::Applied)
}

/// Attempt to activate a paired device immediately without waiting for the
/// next discovery-driven lifecycle reconciliation pass.
pub async fn activate_pairable_device(
    runtime: &DiscoveryRuntime,
    device_id: DeviceId,
    backend_id: &str,
) -> anyhow::Result<bool> {
    let Some(tracked) = runtime.device_registry.get(&device_id).await else {
        return Ok(false);
    };
    if !tracked.user_settings.enabled || tracked.state == DeviceState::Disabled {
        return Ok(false);
    }
    if tracked.state.is_renderable() {
        return Ok(true);
    }

    let fingerprint = runtime.device_registry.fingerprint_for_id(&device_id).await;
    let layout_device_id = {
        let mut lifecycle = runtime.lifecycle_manager.lock().await;
        if let Some(layout_device_id) = lifecycle.layout_device_id_for(device_id) {
            layout_device_id.to_owned()
        } else {
            let _ = lifecycle.on_discovered_with_behavior(
                device_id,
                &tracked.info,
                backend_id,
                fingerprint.as_ref(),
                DiscoveryConnectBehavior::Deferred,
            );
            lifecycle.layout_device_id_for(device_id).map_or_else(
                || {
                    DeviceLifecycleManager::canonical_layout_device_id(
                        backend_id,
                        &tracked.info,
                        fingerprint.as_ref(),
                    )
                },
                ToOwned::to_owned,
            )
        }
    };

    ensure_default_logical_for_device(
        runtime,
        device_id,
        &layout_device_id,
        &tracked.info.name,
        tracked.info.total_led_count(),
    )
    .await;

    if !active_layout_targets_enabled_device(runtime, device_id, &layout_device_id).await {
        return Ok(false);
    }

    connect_backend_device(runtime, backend_id, device_id, &layout_device_id).await?;

    if let Err(error) = refresh_connected_device_info(runtime, backend_id, device_id).await {
        let device_label = device_log_label(runtime, device_id).await;
        warn!(
            device = %device_label,
            device_id = %device_id,
            backend_id = %backend_id,
            error = %error,
            error_chain = %format_error_chain(&error),
            "failed to refresh device metadata after pairing activation"
        );
    }

    let follow_up = {
        let mut lifecycle = runtime.lifecycle_manager.lock().await;
        lifecycle.on_connected(device_id)
    };
    let actions = match follow_up {
        Ok(actions) => actions,
        Err(DeviceError::InvalidTransition { .. }) => Vec::new(),
        Err(DeviceError::NotFound { .. }) => return Ok(false),
        Err(error) => return Err(error.into()),
    };

    if !actions.is_empty() {
        execute_lifecycle_actions(runtime.clone(), actions).await;
    }
    sync_logical_mappings_for_device(runtime, device_id, backend_id, &layout_device_id).await;
    sync_registry_state(runtime, device_id).await;

    let activated_only = HashSet::from([device_id]);
    sync_active_layout_for_renderable_devices(runtime, Some(&activated_only)).await;
    publish_device_connected(runtime, backend_id, device_id).await;
    Ok(true)
}

/// Disconnect a known tracked device outside the standard discovery flow.
pub async fn disconnect_tracked_device(
    runtime: &DiscoveryRuntime,
    device_id: DeviceId,
    reason: DisconnectReason,
    will_retry: bool,
) -> anyhow::Result<bool> {
    let was_renderable = runtime
        .device_registry
        .get(&device_id)
        .await
        .is_some_and(|tracked| tracked.state.is_renderable());

    let actions = {
        let mut lifecycle = runtime.lifecycle_manager.lock().await;
        lifecycle.on_device_vanished(device_id)
    };
    if actions.is_empty() {
        return Ok(false);
    }

    execute_lifecycle_actions(runtime.clone(), actions).await;
    sync_registry_state(runtime, device_id).await;
    sync_active_layout_for_renderable_devices(runtime, None).await;

    if was_renderable {
        runtime
            .event_bus
            .publish(HypercolorEvent::DeviceDisconnected {
                device_id: device_id.to_string(),
                reason,
                will_retry,
            });
    }

    Ok(was_renderable)
}

/// Temporarily release every renderable device without disabling it.
pub async fn release_renderable_devices(runtime: &DiscoveryRuntime) -> usize {
    let tracked_device_ids = {
        let lifecycle = runtime.lifecycle_manager.lock().await;
        lifecycle
            .tracked_device_ids()
            .into_iter()
            .filter(|device_id| {
                lifecycle
                    .state(*device_id)
                    .is_some_and(|state| state.is_renderable())
            })
            .collect::<Vec<_>>()
    };

    let mut released = 0_usize;

    for device_id in tracked_device_ids {
        let actions = {
            let mut lifecycle = runtime.lifecycle_manager.lock().await;
            lifecycle.on_device_vanished(device_id)
        };

        if actions.is_empty() {
            continue;
        }

        execute_lifecycle_actions(runtime.clone(), actions).await;
        sync_registry_state(runtime, device_id).await;
        released = released.saturating_add(1);
    }

    sync_active_layout_for_renderable_devices(runtime, None).await;
    released
}

/// Clear and disconnect every renderable device during daemon shutdown.
pub async fn shutdown_renderable_devices(runtime: &DiscoveryRuntime) -> usize {
    let tracked_device_ids = {
        let lifecycle = runtime.lifecycle_manager.lock().await;
        lifecycle
            .tracked_device_ids()
            .into_iter()
            .filter(|device_id| {
                lifecycle
                    .state(*device_id)
                    .is_some_and(|state| state.is_renderable())
            })
            .collect::<Vec<_>>()
    };

    let mut disconnected = 0_usize;

    for device_id in tracked_device_ids {
        let actions = {
            let mut lifecycle = runtime.lifecycle_manager.lock().await;
            lifecycle.on_user_disable(device_id)
        };

        match actions {
            Ok(actions) => {
                execute_lifecycle_actions(runtime.clone(), actions).await;
                sync_registry_state(runtime, device_id).await;
                disconnected = disconnected.saturating_add(1);
            }
            Err(error) => {
                let device_label = device_log_label(runtime, device_id).await;
                warn!(
                    device = %device_label,
                    device_id = %device_id,
                    error = %error,
                    "failed to disable device during daemon shutdown cleanup"
                );
            }
        }
    }

    disconnected
}

#[allow(clippy::too_many_lines)]
pub(crate) async fn execute_lifecycle_actions(
    runtime: DiscoveryRuntime,
    actions: Vec<LifecycleAction>,
) {
    let mut pending: VecDeque<LifecycleAction> = actions.into();

    while let Some(action) = pending.pop_front() {
        match action {
            LifecycleAction::Connect {
                device_id,
                backend_id,
                layout_device_id,
            } => {
                let result =
                    connect_backend_device(&runtime, &backend_id, device_id, &layout_device_id)
                        .await;

                let (follow_up, connected) = match result {
                    Ok(()) => {
                        if let Err(error) =
                            refresh_connected_device_info(&runtime, &backend_id, device_id).await
                        {
                            let device_label = device_log_label(&runtime, device_id).await;
                            warn!(
                                device = %device_label,
                                device_id = %device_id,
                                backend_id = %backend_id,
                                error = %error,
                                error_chain = %format_error_chain(&error),
                                "failed to refresh device metadata after connect"
                            );
                        }
                        let mut lifecycle = runtime.lifecycle_manager.lock().await;
                        (lifecycle.on_connected(device_id), true)
                    }
                    Err(error) => {
                        let device_label = device_log_label(&runtime, device_id).await;
                        warn!(
                            device = %device_label,
                            device_id = %device_id,
                            backend_id = %backend_id,
                            layout_device_id = %layout_device_id,
                            error = %error,
                            error_chain = %format_error_chain(&error),
                            "lifecycle connect action failed"
                        );
                        let mut lifecycle = runtime.lifecycle_manager.lock().await;
                        (lifecycle.on_connect_failed(device_id), false)
                    }
                };

                match follow_up {
                    Ok(next_actions) => {
                        if connected {
                            sync_logical_mappings_for_device(
                                &runtime,
                                device_id,
                                &backend_id,
                                &layout_device_id,
                            )
                            .await;
                        }
                        pending.extend(next_actions);
                        sync_registry_state(&runtime, device_id).await;
                        if connected {
                            let connected_only = HashSet::from([device_id]);
                            sync_active_layout_for_renderable_devices(
                                &runtime,
                                Some(&connected_only),
                            )
                            .await;
                            publish_device_connected(&runtime, &backend_id, device_id).await;
                        }
                    }
                    Err(error) => {
                        let device_label = device_log_label(&runtime, device_id).await;
                        warn!(
                            device = %device_label,
                            device_id = %device_id,
                            error = %error,
                            "lifecycle state update failed after connect"
                        );
                    }
                }
            }
            LifecycleAction::Disconnect {
                device_id,
                backend_id,
            } => {
                let layout_device_id = {
                    let lifecycle = runtime.lifecycle_manager.lock().await;
                    lifecycle
                        .layout_device_id_for(device_id)
                        .map(ToOwned::to_owned)
                };

                let Some(_layout_device_id) = layout_device_id else {
                    warn!(
                        device_id = %device_id,
                        backend_id = %backend_id,
                        "missing lifecycle layout id during disconnect action"
                    );
                    continue;
                };

                let result = { disconnect_backend_device(&runtime, &backend_id, device_id).await };
                if let Err(error) = result {
                    warn!(
                        device_id = %device_id,
                        backend_id = %backend_id,
                        error = %error,
                        "lifecycle disconnect action failed"
                    );
                }
            }
            LifecycleAction::Map {
                layout_device_id,
                backend_id,
                device_id,
            } => {
                let mut manager = runtime.backend_manager.lock().await;
                manager.map_device(layout_device_id, backend_id, device_id);
            }
            LifecycleAction::Unmap { layout_device_id } => {
                let mut manager = runtime.backend_manager.lock().await;
                manager.unmap_device(&layout_device_id);
            }
            LifecycleAction::SpawnReconnect { device_id, delay } => {
                spawn_reconnect_task(&runtime, device_id, delay);
            }
            LifecycleAction::CancelReconnect { device_id } => {
                cancel_reconnect_task(&runtime, device_id);
            }
        }
    }
}

pub(crate) async fn handle_async_write_failures(
    runtime: &DiscoveryRuntime,
    failures: Vec<AsyncWriteFailure>,
) {
    let mut handled = HashSet::new();

    for failure in failures {
        if !handled.insert(failure.device_id) {
            continue;
        }

        let should_handle = {
            let lifecycle = runtime.lifecycle_manager.lock().await;
            lifecycle
                .state(failure.device_id)
                .is_some_and(|state| state.is_renderable())
        };

        if !should_handle {
            continue;
        }

        warn!(
            backend_id = %failure.backend_id,
            device_id = %failure.device_id,
            error = %failure.error,
            "async device write failed; entering reconnect flow"
        );

        let actions = {
            let mut lifecycle = runtime.lifecycle_manager.lock().await;
            lifecycle.on_comm_error(failure.device_id)
        };

        match actions {
            Ok(actions) => {
                execute_lifecycle_actions(runtime.clone(), actions).await;
                sync_registry_state(runtime, failure.device_id).await;
            }
            Err(error) => {
                warn!(
                    backend_id = %failure.backend_id,
                    device_id = %failure.device_id,
                    error = %error,
                    "failed to transition lifecycle after async device write error"
                );
            }
        }
    }
}

fn spawn_reconnect_task(runtime: &DiscoveryRuntime, device_id: DeviceId, delay: Duration) {
    debug!(
        device_id = %device_id,
        delay_ms = u64::try_from(delay.as_millis()).unwrap_or(u64::MAX),
        "scheduled reconnect attempt"
    );
    let runtime_for_task = runtime.clone();
    let task = runtime.task_spawner.spawn(async move {
        tokio::time::sleep(delay).await;

        // Remove our own handle before executing follow-up logic so reschedules
        // do not fight with this running task.
        runtime_for_task
            .reconnect_tasks
            .lock()
            .expect("reconnect task map lock poisoned")
            .remove(&device_id);

        let connect_action = {
            let lifecycle = runtime_for_task.lifecycle_manager.lock().await;
            lifecycle.on_reconnect_attempt(device_id)
        };
        let Some(LifecycleAction::Connect {
            backend_id,
            layout_device_id,
            ..
        }) = connect_action
        else {
            return;
        };

        debug!(
            device_id = %device_id,
            backend_id = %backend_id,
            layout_device_id = %layout_device_id,
            "starting reconnect attempt"
        );

        let connect_result = {
            connect_backend_device(&runtime_for_task, &backend_id, device_id, &layout_device_id)
                .await
        };
        let reconnected = connect_result.is_ok();

        let follow_up = if let Err(error) = connect_result {
            let device_label = device_log_label(&runtime_for_task, device_id).await;
            warn!(
                device = %device_label,
                device_id = %device_id,
                backend_id = %backend_id,
                layout_device_id = %layout_device_id,
                error = %error,
                error_chain = %format_error_chain(&error),
                "reconnect attempt failed"
            );
            let mut lifecycle = runtime_for_task.lifecycle_manager.lock().await;
            lifecycle.on_reconnect_failed(device_id)
        } else {
            sync_logical_mappings_for_device(
                &runtime_for_task,
                device_id,
                &backend_id,
                &layout_device_id,
            )
            .await;
            let mut lifecycle = runtime_for_task.lifecycle_manager.lock().await;
            lifecycle.on_connected(device_id)
        };

        match follow_up {
            Ok(actions) => {
                execute_lifecycle_actions(runtime_for_task.clone(), actions).await;
                sync_registry_state(&runtime_for_task, device_id).await;
                if reconnected {
                    let reconnect_only = HashSet::from([device_id]);
                    sync_active_layout_for_renderable_devices(
                        &runtime_for_task,
                        Some(&reconnect_only),
                    )
                    .await;
                    publish_device_connected(&runtime_for_task, &backend_id, device_id).await;
                }
            }
            Err(error) => {
                let device_label = device_log_label(&runtime_for_task, device_id).await;
                warn!(
                    device = %device_label,
                    device_id = %device_id,
                    error = %error,
                    "failed to update lifecycle state after reconnect attempt"
                );
            }
        }
    });

    let mut tasks = runtime
        .reconnect_tasks
        .lock()
        .expect("reconnect task map lock poisoned");
    if let Some(existing) = tasks.insert(device_id, task) {
        existing.abort();
    }
}

fn cancel_reconnect_task(runtime: &DiscoveryRuntime, device_id: DeviceId) {
    let mut tasks = runtime
        .reconnect_tasks
        .lock()
        .expect("reconnect task map lock poisoned");
    if let Some(handle) = tasks.remove(&device_id) {
        handle.abort();
    }
}
