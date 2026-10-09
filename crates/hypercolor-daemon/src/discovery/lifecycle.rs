use std::cmp::Reverse;
use std::collections::{HashSet, VecDeque};
use std::time::{Duration, Instant};

use hypercolor_core::device::{
    AsyncWriteFailure, DeviceLifecycleManager, FlapEscalation, LifecycleAction,
    RECONNECT_STABLE_AFTER,
};
use hypercolor_driver_api::{DeviceDeliveryId, DeviceLifecyclePolicy, DiscoveryConnectBehavior};
use hypercolor_types::device::{
    ConnectionType, DeviceError, DeviceId, DeviceState, ErrorRecoverability,
};
use hypercolor_types::event::{DisconnectReason, HypercolorEvent};
use tracing::{debug, warn};

use super::DiscoveryRuntime;
use super::device_helpers::{
    connect_backend_device_with_timeout as connect_backend_device_with_backend_timeout,
    desired_connect_behavior, device_log_label, disconnect_backend_device,
    ensure_default_logical_for_device, format_error_chain, lifecycle_policy_for_device,
    publish_device_connected, refresh_connected_device_info, sync_logical_mappings_for_device,
    sync_registry_state,
};

/// Transient write failures in a row before the daemon rebuilds the device
/// session.
///
/// The producing lane reported each one as transient and kept its session
/// running, so a single failure is a dropped frame, not a dead device, and
/// two can still be one bus hiccup straddling a frame boundary. Three failed
/// transport attempts in a row at the device's own cadence means the lane is
/// not recovering on its own and a reconnect beats dropping more frames. The
/// count is of deliveries, never of elapsed time, and failures the lane did
/// not survive still reconnect on the first one.
const TRANSIENT_WRITE_FAILURE_RECONNECT_THRESHOLD: u32 = 3;

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
        let fingerprint = runtime.device_registry.fingerprint_for_id(&device_id).await;

        desired_connect_behavior(
            runtime,
            device_id,
            &tracked.info,
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
        runtime
            .layout
            .sync_active_layout_for_renderable_devices(runtime.clone(), None)
            .await;
    }

    Box::pin(super::enforce_native_ownership_for_device(
        runtime, device_id,
    ))
    .await;

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
                fingerprint.as_ref(),
                DiscoveryConnectBehavior::Deferred,
            );
            lifecycle.layout_device_id_for(device_id).map_or_else(
                || {
                    DeviceLifecycleManager::canonical_layout_device_id(
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

    if !runtime
        .layout
        .active_layout_targets_enabled_device(runtime, device_id, &layout_device_id)
        .await
    {
        return Ok(false);
    }

    connect_backend_device_with_timeout(runtime, backend_id, device_id, &layout_device_id).await?;

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
    runtime
        .layout
        .sync_active_layout_for_renderable_devices(runtime.clone(), Some(activated_only))
        .await;
    publish_device_connected(runtime, backend_id, device_id).await;
    Box::pin(super::conflict_guard::enforce_native_ownership_for_device(
        runtime, device_id,
    ))
    .await;
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
    runtime
        .layout
        .sync_active_layout_for_renderable_devices(runtime.clone(), None)
        .await;

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

    runtime
        .layout
        .sync_active_layout_for_renderable_devices(runtime.clone(), None)
        .await;
    released
}

/// Temporarily release every renderable network device without disabling it.
pub async fn release_renderable_network_devices(runtime: &DiscoveryRuntime) -> usize {
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
        let is_network = runtime
            .device_registry
            .get(&device_id)
            .await
            .is_some_and(|tracked| tracked.info.connection_type == ConnectionType::Network);
        if !is_network {
            continue;
        }

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

    if released > 0 {
        runtime
            .layout
            .sync_active_layout_for_renderable_devices(runtime.clone(), None)
            .await;
    }
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
                let result = connect_backend_device_with_timeout(
                    &runtime,
                    &backend_id,
                    device_id,
                    &layout_device_id,
                )
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
                        let will_retry =
                            should_retry_connect_failure(&runtime, &backend_id, device_id, &error)
                                .await;
                        let device_label = device_log_label(&runtime, device_id).await;
                        warn!(
                            device = %device_label,
                            device_id = %device_id,
                            backend_id = %backend_id,
                            layout_device_id = %layout_device_id,
                            error = %error,
                            will_retry,
                            "lifecycle connect action failed"
                        );
                        let mut lifecycle = runtime.lifecycle_manager.lock().await;
                        let follow_up = if will_retry {
                            lifecycle.on_connect_failed(device_id)
                        } else {
                            lifecycle.on_connect_abandoned(device_id)
                        };
                        (follow_up, false)
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
                            runtime
                                .layout
                                .sync_active_layout_for_renderable_devices(
                                    runtime.clone(),
                                    Some(connected_only),
                                )
                                .await;
                            publish_device_connected(&runtime, &backend_id, device_id).await;
                            // A bridge route that just came up may shadow a
                            // native device; native wins immediately.
                            Box::pin(super::conflict_guard::enforce_native_ownership_for_device(
                                &runtime, device_id,
                            ))
                            .await;
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

pub(crate) fn handle_async_write_failures(
    runtime: &DiscoveryRuntime,
    failures: Vec<AsyncWriteFailure>,
) {
    if failures.is_empty() {
        return;
    }

    let actions = if let Ok(mut lifecycle) = runtime.lifecycle_manager.try_lock() {
        let failures = claim_current_async_write_failures(failures);
        async_write_failure_actions(&mut lifecycle, failures, Instant::now())
    } else {
        spawn_async_write_failure_worker(runtime.clone(), failures);
        return;
    };

    spawn_async_write_failure_actions(runtime.clone(), actions);
}

#[derive(Debug)]
struct LifecycleWriteFailure {
    backend_id: String,
    device_id: DeviceId,
    delivery_id: DeviceDeliveryId,
    error: DeviceError,
    transient: bool,
    consecutive_failures: u32,
}

impl LifecycleWriteFailure {
    fn recovery(&self) -> WriteFailureRecovery {
        WriteFailureRecovery::classify(&self.error, self.transient, self.consecutive_failures)
    }
}

/// What the lifecycle does about one claimed async write failure, ordered
/// from least to most severe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum WriteFailureRecovery {
    /// Leave the session up and let the next delivery decide.
    KeepLane,
    /// Tear the session down and schedule a reconnect.
    Reconnect,
    /// Stop driving the device until something changes.
    Deactivate,
}

impl WriteFailureRecovery {
    fn classify(error: &DeviceError, transient: bool, consecutive_failures: u32) -> Self {
        match error.recoverability() {
            ErrorRecoverability::Retry => Self::KeepLane,
            ErrorRecoverability::Reconnect
                if transient
                    && consecutive_failures < TRANSIENT_WRITE_FAILURE_RECONNECT_THRESHOLD =>
            {
                Self::KeepLane
            }
            ErrorRecoverability::Reconnect => Self::Reconnect,
            ErrorRecoverability::Permanent => Self::Deactivate,
        }
    }
}

/// Lifecycle work planned for one device after an async write failure.
#[derive(Debug)]
struct PlannedWriteRecovery {
    device_id: DeviceId,
    actions: Vec<LifecycleAction>,
    /// Published once the actions ran, when this failure escalated a flap.
    device_error: Option<HypercolorEvent>,
}

/// Keep the most severe failure for each device.
///
/// The backend manager orders failures by typed recoverability alone, which
/// ranks a transient failure the lane is riding out beside one that needs a
/// reconnect. A device with two output lanes (LEDs and an LCD) must not let
/// the kept lane shadow the failed one, so severity decides first and the
/// manager's order breaks ties.
fn most_severe_per_device<T>(
    mut failures: Vec<(WriteFailureRecovery, DeviceId, T)>,
) -> Vec<(WriteFailureRecovery, T)> {
    failures.sort_by_key(|(recovery, _, _)| Reverse(*recovery));
    let mut handled = HashSet::new();
    failures
        .into_iter()
        .filter(|(_, device_id, _)| handled.insert(*device_id))
        .map(|(recovery, _, failure)| (recovery, failure))
        .collect()
}

fn claim_current_async_write_failures(
    failures: Vec<AsyncWriteFailure>,
) -> Vec<LifecycleWriteFailure> {
    let classified = failures
        .into_iter()
        .map(|failure| {
            let recovery = WriteFailureRecovery::classify(
                &failure.error,
                failure.transient,
                failure.consecutive_failures,
            );
            (recovery, failure.device_id, failure)
        })
        .collect();

    most_severe_per_device(classified)
        .into_iter()
        .filter_map(|(recovery, failure)| {
            // A failure the lane rides out stays unacknowledged, so the next
            // failed delivery replaces it with a longer streak and the next
            // completed one clears it.
            let is_current = if recovery == WriteFailureRecovery::KeepLane
                && !failure.is_from_retired_generation()
            {
                failure.is_current()
            } else {
                failure.try_acknowledge()
            };

            is_current.then_some(LifecycleWriteFailure {
                backend_id: failure.backend_id,
                device_id: failure.device_id,
                delivery_id: failure.delivery_id,
                error: failure.error,
                transient: failure.transient,
                consecutive_failures: failure.consecutive_failures,
            })
        })
        .collect()
}

fn async_write_failure_actions(
    lifecycle: &mut DeviceLifecycleManager,
    failures: Vec<LifecycleWriteFailure>,
    now: Instant,
) -> Vec<PlannedWriteRecovery> {
    let mut handled = HashSet::new();
    let mut planned = Vec::new();

    for failure in failures {
        if !handled.insert(failure.device_id) {
            continue;
        }

        if !lifecycle
            .state(failure.device_id)
            .is_some_and(|state| state.is_renderable())
        {
            continue;
        }

        let recovery = failure.recovery();
        if recovery == WriteFailureRecovery::KeepLane {
            debug!(
                backend_id = %failure.backend_id,
                device_id = %failure.device_id,
                queue_generation = failure.delivery_id.queue_generation,
                sequence = failure.delivery_id.sequence,
                error = %failure.error,
                transient = failure.transient,
                consecutive_failures = failure.consecutive_failures,
                reconnect_threshold = TRANSIENT_WRITE_FAILURE_RECONNECT_THRESHOLD,
                "async device write failed; keeping output lane active"
            );
            continue;
        }

        let actions = match recovery {
            WriteFailureRecovery::KeepLane => unreachable!("kept lanes return above"),
            WriteFailureRecovery::Reconnect => lifecycle.on_comm_error_at(failure.device_id, now),
            WriteFailureRecovery::Deactivate => lifecycle.on_runtime_deactivate(failure.device_id),
        };
        let actions = match actions {
            Ok(actions) => actions,
            Err(error) => {
                warn!(
                    backend_id = %failure.backend_id,
                    device_id = %failure.device_id,
                    error = %error,
                    "failed to transition lifecycle after async device write error"
                );
                continue;
            }
        };

        let escalation = lifecycle.take_flap_escalation(failure.device_id);
        // Read after the transition: a fault that ended a streak has already
        // reset it, and deactivation clears it, so both still warn below.
        let flap_already_reported = escalation.is_none()
            && recovery == WriteFailureRecovery::Reconnect
            && lifecycle.is_flapping(failure.device_id);
        if let Some(escalation) = escalation {
            warn!(
                backend_id = %failure.backend_id,
                device_id = %failure.device_id,
                error = %failure.error,
                flaps = escalation.flaps,
                stable_after_secs = RECONNECT_STABLE_AFTER.as_secs(),
                next_retry_ms = u64::try_from(escalation.next_retry.as_millis())
                    .unwrap_or(u64::MAX),
                "device keeps failing writes right after reconnecting; reconnect backoff \
                 keeps growing until a connection holds"
            );
        } else if flap_already_reported {
            debug!(
                backend_id = %failure.backend_id,
                device_id = %failure.device_id,
                error = %failure.error,
                flaps = lifecycle.flap_count(failure.device_id).unwrap_or_default(),
                "flapping device failed again after reconnecting"
            );
        } else {
            warn!(
                backend_id = %failure.backend_id,
                device_id = %failure.device_id,
                queue_generation = failure.delivery_id.queue_generation,
                sequence = failure.delivery_id.sequence,
                error = %failure.error,
                transient = failure.transient,
                consecutive_failures = failure.consecutive_failures,
                recovery = ?recovery,
                "async device write failed; applying typed lifecycle recovery"
            );
        }

        planned.push(PlannedWriteRecovery {
            device_id: failure.device_id,
            actions,
            device_error: escalation
                .map(|escalation| flap_device_error(failure.device_id, &failure.error, escalation)),
        });
    }

    planned
}

/// The `DeviceError` event that tells clients a device is flapping.
fn flap_device_error(
    device_id: DeviceId,
    error: &DeviceError,
    escalation: FlapEscalation,
) -> HypercolorEvent {
    HypercolorEvent::DeviceError {
        device_id: device_id.to_string(),
        error: format!(
            "writes keep failing right after reconnecting ({} reconnects in a row): {error}",
            escalation.flaps
        ),
        recoverable: true,
    }
}

fn spawn_async_write_failure_worker(runtime: DiscoveryRuntime, failures: Vec<AsyncWriteFailure>) {
    let task_spawner = runtime.task_spawner.clone();
    std::mem::drop(task_spawner.spawn(async move {
        let actions = {
            let mut lifecycle = runtime.lifecycle_manager.lock().await;
            let failures = claim_current_async_write_failures(failures);
            async_write_failure_actions(&mut lifecycle, failures, Instant::now())
        };

        run_async_write_failure_actions(runtime, actions).await;
    }));
}

fn spawn_async_write_failure_actions(
    runtime: DiscoveryRuntime,
    actions: Vec<PlannedWriteRecovery>,
) {
    if actions.is_empty() {
        return;
    }

    let task_spawner = runtime.task_spawner.clone();
    std::mem::drop(task_spawner.spawn(async move {
        run_async_write_failure_actions(runtime, actions).await;
    }));
}

async fn run_async_write_failure_actions(
    runtime: DiscoveryRuntime,
    planned: Vec<PlannedWriteRecovery>,
) {
    for recovery in planned {
        execute_lifecycle_actions(runtime.clone(), recovery.actions).await;
        sync_registry_state(&runtime, recovery.device_id).await;
        if let Some(event) = recovery.device_error {
            runtime.event_bus.publish(event);
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
            let mut lifecycle = runtime_for_task.lifecycle_manager.lock().await;
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

        let connect_result = connect_backend_device_with_timeout(
            &runtime_for_task,
            &backend_id,
            device_id,
            &layout_device_id,
        )
        .await;
        let reconnected = connect_result.is_ok();

        let follow_up = if let Err(error) = connect_result {
            let will_retry =
                should_retry_connect_failure(&runtime_for_task, &backend_id, device_id, &error)
                    .await;
            let device_label = device_log_label(&runtime_for_task, device_id).await;
            warn!(
                device = %device_label,
                device_id = %device_id,
                backend_id = %backend_id,
                layout_device_id = %layout_device_id,
                error = %error,
                will_retry,
                "reconnect attempt failed"
            );
            let mut lifecycle = runtime_for_task.lifecycle_manager.lock().await;
            if will_retry {
                lifecycle.on_reconnect_failed(device_id)
            } else {
                lifecycle.on_connect_abandoned(device_id)
            }
        } else {
            if let Err(error) =
                refresh_connected_device_info(&runtime_for_task, &backend_id, device_id).await
            {
                let device_label = device_log_label(&runtime_for_task, device_id).await;
                warn!(
                    device = %device_label,
                    device_id = %device_id,
                    backend_id = %backend_id,
                    error = %error,
                    error_chain = %format_error_chain(&error),
                    "failed to refresh device metadata after reconnect"
                );
            }
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
                    runtime_for_task
                        .layout
                        .sync_active_layout_for_renderable_devices(
                            runtime_for_task.clone(),
                            Some(reconnect_only),
                        )
                        .await;
                    publish_device_connected(&runtime_for_task, &backend_id, device_id).await;
                    Box::pin(super::conflict_guard::enforce_native_ownership_for_device(
                        &runtime_for_task,
                        device_id,
                    ))
                    .await;
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

async fn connect_backend_device_with_timeout(
    runtime: &DiscoveryRuntime,
    backend_id: &str,
    device_id: DeviceId,
    layout_device_id: &str,
) -> Result<(), DeviceError> {
    let timeout = lifecycle_policy_for_device(runtime, backend_id, device_id)
        .await
        .connect_timeout();
    connect_backend_device_with_backend_timeout(
        runtime,
        backend_id,
        device_id,
        layout_device_id,
        timeout,
    )
    .await
}

async fn should_retry_connect_failure(
    runtime: &DiscoveryRuntime,
    backend_id: &str,
    device_id: DeviceId,
    error: &DeviceError,
) -> bool {
    let policy = lifecycle_policy_for_device(runtime, backend_id, device_id).await;
    connect_failure_is_retryable(policy, error)
}

fn connect_failure_is_retryable(policy: DeviceLifecyclePolicy, error: &DeviceError) -> bool {
    policy.should_retry_connect_failure(error)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;
    use std::sync::{Arc, Mutex as StdMutex};
    use std::time::Instant;

    use hypercolor_core::device::FLAP_ESCALATION_THRESHOLD;
    use hypercolor_types::device::{
        ConnectionType, DeviceCapabilities, DeviceFamily, DeviceInfo, DeviceOrigin,
    };
    use tracing_subscriber::fmt::writer::MakeWriter;

    fn active_lifecycle() -> (DeviceLifecycleManager, DeviceId) {
        let device_id = DeviceId::new();
        let info = DeviceInfo {
            id: device_id,
            name: "Async Output Fixture".to_owned(),
            vendor: "Hypercolor".to_owned(),
            family: DeviceFamily::new_static("fixture", "Fixture"),
            model: None,
            connection_type: ConnectionType::Network,
            origin: DeviceOrigin::native("fixture", "fixture", ConnectionType::Network),
            segments: Vec::new(),
            firmware_version: None,
            capabilities: DeviceCapabilities::default(),
        };
        let mut lifecycle = DeviceLifecycleManager::new();
        lifecycle.on_discovered(device_id, &info, None);
        lifecycle
            .on_connected(device_id)
            .expect("fixture should connect");
        lifecycle
            .on_frame_success(device_id)
            .expect("fixture should become active");
        (lifecycle, device_id)
    }

    fn async_failure(device_id: DeviceId, error: DeviceError) -> LifecycleWriteFailure {
        LifecycleWriteFailure {
            backend_id: "fixture".to_owned(),
            device_id,
            delivery_id: DeviceDeliveryId {
                queue_generation: 1,
                sequence: 1,
            },
            error,
            transient: false,
            consecutive_failures: 1,
        }
    }

    fn transient_failure(device_id: DeviceId, consecutive_failures: u32) -> LifecycleWriteFailure {
        LifecycleWriteFailure {
            transient: true,
            consecutive_failures,
            ..async_failure(
                device_id,
                DeviceError::write(device_id, "hid write reported a short count"),
            )
        }
    }

    #[test]
    fn connect_retry_policy_branches_on_typed_recoverability() {
        let no_timeout_retry = DeviceLifecyclePolicy::default().without_connect_timeout_retry();

        assert!(!connect_failure_is_retryable(
            no_timeout_retry,
            &DeviceError::Timeout {
                after: Duration::from_secs(1),
            }
        ));
        assert!(connect_failure_is_retryable(
            DeviceLifecyclePolicy::default(),
            &DeviceError::Timeout {
                after: Duration::from_secs(1),
            }
        ));
        assert!(connect_failure_is_retryable(
            no_timeout_retry,
            &DeviceError::connection("fixture", "connection refused")
        ));
        assert!(!connect_failure_is_retryable(
            DeviceLifecyclePolicy::default(),
            &DeviceError::NotAdopted {
                device_id: DeviceId::new(),
            }
        ));
    }

    #[test]
    fn async_timeout_keeps_active_lane_for_retry() {
        let (mut lifecycle, device_id) = active_lifecycle();

        let planned = async_write_failure_actions(
            &mut lifecycle,
            vec![async_failure(
                device_id,
                DeviceError::Timeout {
                    after: Duration::from_millis(25),
                },
            )],
            Instant::now(),
        );

        assert!(planned.is_empty());
        assert_eq!(lifecycle.state(device_id), Some(DeviceState::Active));
    }

    #[test]
    fn async_transient_failure_enters_reconnect_flow() {
        let (mut lifecycle, device_id) = active_lifecycle();

        let planned = async_write_failure_actions(
            &mut lifecycle,
            vec![async_failure(
                device_id,
                DeviceError::write(device_id, "connection reset"),
            )],
            Instant::now(),
        );

        assert_eq!(planned.len(), 1);
        assert!(
            planned[0]
                .actions
                .iter()
                .any(|action| matches!(action, LifecycleAction::SpawnReconnect { .. }))
        );
        assert_eq!(lifecycle.state(device_id), Some(DeviceState::Reconnecting));
    }

    #[test]
    fn async_permanent_failure_deactivates_without_reconnect() {
        let (mut lifecycle, device_id) = active_lifecycle();

        let planned = async_write_failure_actions(
            &mut lifecycle,
            vec![async_failure(
                device_id,
                DeviceError::PermissionDenied {
                    device: device_id.to_string(),
                    detail: "access revoked".to_owned(),
                },
            )],
            Instant::now(),
        );

        assert_eq!(planned.len(), 1);
        assert!(
            planned[0]
                .actions
                .iter()
                .any(|action| matches!(action, LifecycleAction::Disconnect { .. }))
        );
        assert!(
            !planned[0]
                .actions
                .iter()
                .any(|action| matches!(action, LifecycleAction::SpawnReconnect { .. }))
        );
        assert_eq!(lifecycle.state(device_id), Some(DeviceState::Known));
    }

    #[test]
    fn single_transient_failure_keeps_healthy_device_active() {
        let (mut lifecycle, device_id) = active_lifecycle();

        let planned = async_write_failure_actions(
            &mut lifecycle,
            vec![transient_failure(device_id, 1)],
            Instant::now(),
        );

        assert!(planned.is_empty());
        assert_eq!(lifecycle.state(device_id), Some(DeviceState::Active));
    }

    #[test]
    fn transient_failures_reconnect_once_the_streak_reaches_the_threshold() {
        let (mut lifecycle, device_id) = active_lifecycle();

        for consecutive in 1..TRANSIENT_WRITE_FAILURE_RECONNECT_THRESHOLD {
            let planned = async_write_failure_actions(
                &mut lifecycle,
                vec![transient_failure(device_id, consecutive)],
                Instant::now(),
            );
            assert!(
                planned.is_empty(),
                "streak of {consecutive} should keep the lane"
            );
        }
        assert_eq!(lifecycle.state(device_id), Some(DeviceState::Active));

        let planned = async_write_failure_actions(
            &mut lifecycle,
            vec![transient_failure(
                device_id,
                TRANSIENT_WRITE_FAILURE_RECONNECT_THRESHOLD,
            )],
            Instant::now(),
        );

        assert_eq!(planned.len(), 1);
        assert!(
            planned[0]
                .actions
                .iter()
                .any(|action| matches!(action, LifecycleAction::SpawnReconnect { .. }))
        );
        assert!(planned[0].device_error.is_none());
        assert_eq!(lifecycle.state(device_id), Some(DeviceState::Reconnecting));
    }

    #[test]
    fn write_failure_recovery_reconnects_lane_ending_failures_immediately() {
        let device_id = DeviceId::new();
        let write = DeviceError::write(device_id, "hid write reported a short count");

        assert_eq!(
            WriteFailureRecovery::classify(&write, true, 1),
            WriteFailureRecovery::KeepLane
        );
        assert_eq!(
            WriteFailureRecovery::classify(&write, false, 1),
            WriteFailureRecovery::Reconnect
        );
        assert_eq!(
            WriteFailureRecovery::classify(
                &DeviceError::Disconnected {
                    device: device_id.to_string(),
                },
                false,
                1
            ),
            WriteFailureRecovery::Reconnect
        );
        assert_eq!(
            WriteFailureRecovery::classify(
                &DeviceError::PermissionDenied {
                    device: device_id.to_string(),
                    detail: "access revoked".to_owned(),
                },
                true,
                1
            ),
            WriteFailureRecovery::Deactivate
        );
    }

    #[derive(Clone, Default)]
    struct CapturedLogs(Arc<StdMutex<Vec<u8>>>);

    impl CapturedLogs {
        fn count(&self, needle: &str) -> usize {
            let bytes = self
                .0
                .lock()
                .expect("captured log lock should not be poisoned")
                .clone();
            String::from_utf8(bytes)
                .expect("captured logs should be UTF-8")
                .matches(needle)
                .count()
        }
    }

    impl io::Write for CapturedLogs {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0
                .lock()
                .expect("captured log lock should not be poisoned")
                .extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for CapturedLogs {
        type Writer = Self;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    #[test]
    fn flapping_write_failures_escalate_once_with_growing_backoff() {
        let (mut lifecycle, device_id) = active_lifecycle();
        // The first fault follows a long healthy run, so it retries fresh.
        lifecycle
            .on_comm_error_at(device_id, Instant::now() + Duration::from_hours(1))
            .expect("first fault should enter reconnecting");

        let logs = CapturedLogs::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(logs.clone())
            .with_max_level(tracing::Level::WARN)
            .with_ansi(false)
            .finish();

        let flaps = FLAP_ESCALATION_THRESHOLD + 3;
        let (delays, device_errors) = tracing::subscriber::with_default(subscriber, || {
            let mut delays = Vec::new();
            let mut device_errors = Vec::new();
            for _ in 0..flaps {
                assert!(lifecycle.on_reconnect_attempt(device_id).is_some());
                lifecycle
                    .on_connected(device_id)
                    .expect("reconnect should succeed");
                let planned = async_write_failure_actions(
                    &mut lifecycle,
                    vec![transient_failure(
                        device_id,
                        TRANSIENT_WRITE_FAILURE_RECONNECT_THRESHOLD,
                    )],
                    Instant::now(),
                );
                assert_eq!(planned.len(), 1, "every flap should plan a reconnect");
                let recovery = planned.into_iter().next().expect("one planned recovery");
                delays.extend(recovery.actions.iter().find_map(|action| match action {
                    LifecycleAction::SpawnReconnect { delay, .. } => Some(*delay),
                    _ => None,
                }));
                device_errors.extend(recovery.device_error);
            }
            (delays, device_errors)
        });

        assert!(
            delays.windows(2).all(|pair| pair[1] > pair[0]),
            "backoff should keep growing across flaps: {delays:?}"
        );
        assert_eq!(
            delays.len(),
            usize::try_from(flaps).expect("flaps fit usize")
        );

        assert_eq!(device_errors.len(), 1, "exactly one DeviceError per streak");
        assert!(matches!(
            &device_errors[0],
            HypercolorEvent::DeviceError { device_id: id, error, recoverable: true }
                if *id == device_id.to_string() && error.contains("short count")
        ));

        assert_eq!(
            logs.count("device keeps failing writes right after reconnecting"),
            1,
            "the escalation warning should fire once per streak"
        );
        assert_eq!(
            logs.count("async device write failed; applying typed lifecycle recovery"),
            usize::try_from(FLAP_ESCALATION_THRESHOLD - 1).expect("threshold fits usize"),
            "flaps before the escalation keep their per-failure warning; later ones go quiet"
        );
    }

    #[test]
    fn most_severe_failure_per_device_wins_the_claim() {
        let led_and_lcd = DeviceId::new();
        let strip = DeviceId::new();

        let claimed = most_severe_per_device(vec![
            (WriteFailureRecovery::KeepLane, led_and_lcd, "led transient"),
            (WriteFailureRecovery::KeepLane, strip, "strip transient"),
            (
                WriteFailureRecovery::Reconnect,
                led_and_lcd,
                "lcd disconnected",
            ),
            (
                WriteFailureRecovery::KeepLane,
                strip,
                "strip older transient",
            ),
        ]);

        assert_eq!(
            claimed,
            vec![
                (WriteFailureRecovery::Reconnect, "lcd disconnected"),
                (WriteFailureRecovery::KeepLane, "strip transient"),
            ]
        );
    }

    /// Escalate a flap streak on an active fixture and leave it connected.
    fn escalated_flapping_lifecycle() -> (DeviceLifecycleManager, DeviceId) {
        let (mut lifecycle, device_id) = active_lifecycle();
        lifecycle
            .on_comm_error_at(device_id, Instant::now() + Duration::from_hours(1))
            .expect("first fault should enter reconnecting");
        for _ in 0..=FLAP_ESCALATION_THRESHOLD {
            assert!(lifecycle.on_reconnect_attempt(device_id).is_some());
            lifecycle
                .on_connected(device_id)
                .expect("reconnect should succeed");
            async_write_failure_actions(
                &mut lifecycle,
                vec![async_failure(
                    device_id,
                    DeviceError::write(device_id, "short count"),
                )],
                Instant::now(),
            );
        }
        assert!(lifecycle.is_flapping(device_id));
        assert!(lifecycle.on_reconnect_attempt(device_id).is_some());
        lifecycle
            .on_connected(device_id)
            .expect("reconnect should succeed");
        (lifecycle, device_id)
    }

    fn captured_warnings<R>(run: impl FnOnce() -> R) -> (R, CapturedLogs) {
        let logs = CapturedLogs::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(logs.clone())
            .with_max_level(tracing::Level::WARN)
            .with_ansi(false)
            .finish();
        let result = tracing::subscriber::with_default(subscriber, run);
        (result, logs)
    }

    #[test]
    fn fault_after_a_recovered_streak_warns_again() {
        let (mut lifecycle, device_id) = escalated_flapping_lifecycle();

        let (planned, logs) = captured_warnings(|| {
            async_write_failure_actions(
                &mut lifecycle,
                vec![async_failure(
                    device_id,
                    DeviceError::write(device_id, "cable bumped"),
                )],
                Instant::now() + RECONNECT_STABLE_AFTER + Duration::from_secs(1),
            )
        });

        assert_eq!(planned.len(), 1);
        assert_eq!(lifecycle.flap_count(device_id), Some(0));
        assert_eq!(
            logs.count("async device write failed; applying typed lifecycle recovery"),
            1,
            "a fault on a recovered device is news, not part of the old streak"
        );
    }

    #[test]
    fn deactivating_a_flapping_device_warns() {
        let (mut lifecycle, device_id) = escalated_flapping_lifecycle();

        let (planned, logs) = captured_warnings(|| {
            async_write_failure_actions(
                &mut lifecycle,
                vec![async_failure(
                    device_id,
                    DeviceError::PermissionDenied {
                        device: device_id.to_string(),
                        detail: "access revoked".to_owned(),
                    },
                )],
                Instant::now(),
            )
        });

        assert_eq!(planned.len(), 1);
        assert_eq!(lifecycle.state(device_id), Some(DeviceState::Known));
        assert_eq!(
            logs.count("async device write failed; applying typed lifecycle recovery"),
            1
        );
    }
}
