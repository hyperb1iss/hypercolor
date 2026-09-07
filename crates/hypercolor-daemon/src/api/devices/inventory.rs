//! Hardware inventory endpoints: `/devices/unclaimed`, `/devices/coverage`,
//! and the bridge facts folded into device summaries.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::State;
use axum::response::Response;

use hypercolor_types::api::devices::{
    BridgeDeviceSummary, DeviceCoverageListResponse, UnclaimedDeviceListResponse,
};
use hypercolor_types::device::{DeviceId, DeviceInfo};

use crate::api::envelope;
use crate::app_state::AppState;
use crate::discovery::{self as core_discovery, JoinedCoverageRow, is_openrgb_bridge_device};

/// `GET /api/v1/devices/unclaimed` — USB devices no enabled native driver
/// claims.
pub async fn list_unclaimed_devices(State(state): State<Arc<AppState>>) -> Response {
    let items = crate::api::discovery_runtime(&state)
        .unclaimed_devices
        .snapshot();
    let total = u64::try_from(items.len()).expect("unclaimed device count fits in u64");
    envelope::ok(UnclaimedDeviceListResponse {
        items,
        total,
        page: None,
    })
}

/// `GET /api/v1/devices/coverage` — native, bridge, and unclaimed views
/// joined per physical device.
pub async fn get_device_coverage(State(state): State<Arc<AppState>>) -> Response {
    let runtime = crate::api::discovery_runtime(&state);
    let items: Vec<_> = core_discovery::collect_device_coverage(&runtime)
        .await
        .into_iter()
        .map(JoinedCoverageRow::into_api_row)
        .collect();
    let total = u64::try_from(items.len()).expect("coverage row count fits in u64");
    envelope::ok(DeviceCoverageListResponse {
        items,
        total,
        page: None,
    })
}

/// The bridge facts for one device, or `None` for anything that is not an
/// OpenRGB bridge route.
///
/// The daemon's conflict-guard lock overrides what the bridge advertises:
/// a locked route reports `output_enabled: false` with the guard's reason.
pub(super) fn bridge_device_summary(
    state: &AppState,
    info: &DeviceInfo,
    metadata: Option<&HashMap<String, String>>,
) -> Option<BridgeDeviceSummary> {
    let empty = HashMap::new();
    let metadata = metadata.unwrap_or(&empty);
    if !is_openrgb_bridge_device(info, metadata) {
        return None;
    }
    let value = |key: &str| {
        metadata
            .get(key)
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    };
    let lock = state
        .driver_host()
        .discovery_runtime()
        .bridge_output_locks
        .get(&info.id);
    let advertised_enabled =
        value("output_enabled").is_none_or(|flag| !flag.eq_ignore_ascii_case("false"));

    Some(BridgeDeviceSummary {
        endpoint: value("endpoint"),
        controller_index: value("controller_index").and_then(|index| index.parse().ok()),
        identity_confidence: value("identity_confidence"),
        detector_class: value("detector_class"),
        output_enabled: advertised_enabled && lock.is_none(),
        disabled_reason: lock
            .map(|lock| lock.reason)
            .or_else(|| value("disabled_reason")),
        protocol_version: value("protocol_version").and_then(|version| version.parse().ok()),
        fingerprint: value("fingerprint"),
    })
}

/// Why output through a bridge route is off, when it is.
///
/// Used by identify so a caller learns the route is output-disabled (and
/// why) instead of a generic "not connected".
pub(super) async fn bridge_output_disabled_reason(
    state: &AppState,
    device_id: DeviceId,
    info: &DeviceInfo,
) -> Option<String> {
    let metadata = state.device_registry.metadata_for_id(&device_id).await;
    let summary = bridge_device_summary(state, info, metadata.as_ref())?;
    if summary.output_enabled {
        return None;
    }
    Some(
        summary
            .disabled_reason
            .unwrap_or_else(|| "the bridge reports this controller as output-disabled".to_owned()),
    )
}
