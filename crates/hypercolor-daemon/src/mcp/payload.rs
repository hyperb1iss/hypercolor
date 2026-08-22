use hypercolor_core::input::InteractionDegradation;
use serde_json::Value;

use super::device_payload::inventory_device_payload;
use super::results::{
    DeviceInventoryResult, DeviceInventorySummary, DeviceStatusSummary, EffectRef, FpsResult,
    InputAvailability, InputStatusResult, InteractionAvailability, StatusResult,
};
use super::tools::{brightness_percent, render_capacity_fps};
use crate::api::effects::active_effect_metadata;
use crate::app_state::AppState;
use crate::domain::input_status::input_status_snapshot;

#[derive(Debug, Default)]
pub(crate) struct DeviceInventoryFilter<'a> {
    status: Option<&'a str>,
    driver_id: Option<&'a str>,
    backend_id: Option<&'a str>,
}

impl<'a> DeviceInventoryFilter<'a> {
    pub(crate) fn from_params(params: &'a Value) -> Self {
        Self {
            status: params.get("status").and_then(Value::as_str),
            driver_id: params.get("driver_id").and_then(Value::as_str),
            backend_id: params.get("backend_id").and_then(Value::as_str),
        }
    }
}

pub(crate) async fn build_status_payload(state: &AppState) -> StatusResult {
    let render_stats = {
        let render_loop = state.render_loop.read().await;
        render_loop.stats()
    };
    let target_fps = render_stats.tier.fps();
    let capacity_fps = render_capacity_fps(&render_stats);
    let delivered_fps = if matches!(
        render_stats.state,
        hypercolor_core::engine::RenderLoopState::Running
    ) {
        state.performance.read().await.snapshot().delivered_fps
    } else {
        0.0
    };

    let brightness = brightness_percent(state.output_power.global_brightness());
    let active_effect = active_effect_metadata(state).await;
    let effect_count = state.domains.effects.len().await;
    let scene_count = state.scene_manager.snapshot().await.scene_count();
    let devices = state.device_registry.list().await;
    let connected_devices = devices
        .iter()
        .filter(|device| device.state.is_renderable())
        .count();
    let total_leds: u64 = devices
        .iter()
        .map(|device| u64::from(device.info.total_led_count()))
        .sum();

    let (audio_status, screen_status) = if let Some(config_manager) = state.config_manager.as_ref()
    {
        let config = config_manager.get();
        let availability = |enabled| {
            if enabled {
                InputAvailability::Enabled
            } else {
                InputAvailability::Disabled
            }
        };
        (
            availability(config.audio.enabled),
            availability(config.capture.enabled),
        )
    } else {
        (InputAvailability::Unknown, InputAvailability::Unknown)
    };
    let input = input_status_snapshot(state);
    let input_state = interaction_state(input.enabled, input.degraded.as_deref());

    let power = state.output_power.snapshot();
    let paused = power.reported_paused();

    StatusResult {
        running: !power.sleeping(),
        paused,
        brightness,
        fps: FpsResult {
            target: target_fps,
            capacity: capacity_fps,
            delivered: delivered_fps,
            actual: capacity_fps,
        },
        effect: active_effect.map(|metadata| EffectRef {
            id: metadata.id.to_string(),
            name: metadata.name,
        }),
        effect_count,
        scene_count,
        devices: DeviceStatusSummary {
            connected: connected_devices,
            total: devices.len(),
            total_leds,
        },
        inputs: InputStatusResult {
            audio: audio_status,
            screen: screen_status,
            input: input_state,
            input_devices_opened: input.devices_opened,
            input_devices_denied: input.devices_denied,
            input_degraded: input.degraded,
            source_graph_generation: input.source_graph_generation,
            sources: input.sources,
        },
        uptime_seconds: state.start_time.elapsed().as_secs(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
    }
}

pub(crate) async fn build_device_inventory_payload(
    state: &AppState,
    filter: DeviceInventoryFilter<'_>,
) -> DeviceInventoryResult {
    let devices = state.device_registry.list().await;
    let filtered = devices
        .into_iter()
        .filter(|device| match filter.status.unwrap_or("all") {
            "connected" => device.state.is_renderable(),
            "disconnected" => !device.state.is_renderable(),
            _ => true,
        })
        .filter(|device| {
            filter
                .driver_id
                .is_none_or(|expected| device.info.driver_id().eq_ignore_ascii_case(expected))
        })
        .filter(|device| {
            filter.backend_id.is_none_or(|expected| {
                device
                    .info
                    .output_backend_id()
                    .eq_ignore_ascii_case(expected)
            })
        })
        .collect::<Vec<_>>();

    let connected = filtered
        .iter()
        .filter(|device| device.state.is_renderable())
        .count();
    let total_leds: u64 = filtered
        .iter()
        .map(|device| u64::from(device.info.total_led_count()))
        .sum();
    let devices = filtered
        .iter()
        .map(|device| inventory_device_payload(state, &device.info, &device.state))
        .collect::<Vec<_>>();
    let total = devices.len();

    DeviceInventoryResult {
        devices,
        summary: DeviceInventorySummary {
            total,
            connected,
            total_leds,
        },
    }
}

fn interaction_state(enabled: bool, degraded: Option<&str>) -> InteractionAvailability {
    if enabled {
        match degraded {
            Some(code) if code == InteractionDegradation::AccessDenied.code() => {
                InteractionAvailability::BlockedPermissions
            }
            Some(code) if code == InteractionDegradation::NoInteractiveSession.code() => {
                InteractionAvailability::NoInteractiveSession
            }
            Some(code)
                if code == InteractionDegradation::InputMonitoringPermissionDenied.code()
                    || code == InteractionDegradation::InputMonitoringPermissionRevoked.code() =>
            {
                InteractionAvailability::BlockedPermissions
            }
            Some(_) => InteractionAvailability::Unavailable,
            None => InteractionAvailability::Enabled,
        }
    } else {
        InteractionAvailability::Disabled
    }
}

#[cfg(test)]
mod tests {
    use hypercolor_core::input::InteractionDegradation;

    use super::interaction_state;
    use crate::mcp::results::InteractionAvailability;

    #[test]
    fn macos_permission_failures_report_blocked_permissions() {
        for degradation in [
            InteractionDegradation::InputMonitoringPermissionDenied,
            InteractionDegradation::InputMonitoringPermissionRevoked,
        ] {
            assert_eq!(
                interaction_state(true, Some(degradation.code())),
                InteractionAvailability::BlockedPermissions
            );
        }
    }
}
