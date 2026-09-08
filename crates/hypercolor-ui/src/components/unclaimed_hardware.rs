//! Hardware coverage that starts with the host inventory, not browser probes.
use crate::{api, app::WsContext};
use hypercolor_types::api::devices::{CoverageIdentityKind, DeviceCoverageRow, UnclaimedDevice};
use leptos::prelude::*;

/// A serial match is the only supported basis for the bridge availability offer.
pub fn matching_bridge<'a>(
    device: &UnclaimedDevice,
    coverage: &'a [DeviceCoverageRow],
) -> Option<&'a str> {
    let serial = device.serial.as_deref()?.trim();
    if serial.is_empty() {
        return None;
    }
    coverage
        .iter()
        .find(|row| {
            row.identity.kind == CoverageIdentityKind::Serial
                && row.identity.value.trim().eq_ignore_ascii_case(serial)
        })
        .and_then(|row| row.bridge.as_ref())
        .map(|bridge| bridge.device_id.as_str())
}

/// Prefill the repository's device-support issue form without submitting it.
pub fn support_issue_url(device: &UnclaimedDevice, platform: &str, available: bool) -> String {
    let platform = match platform.to_ascii_lowercase().as_str() {
        "linux" => "Linux",
        "macos" | "darwin" => "macOS",
        "windows" => "Windows",
        _ => "",
    };
    let model = device.product.as_deref().unwrap_or("Unknown USB device");
    let vendor = device.manufacturer.as_deref().unwrap_or("Unknown vendor");
    let support = if available {
        "OpenRGB lists a controller with the same serial number.".to_owned()
    } else if let Some(driver) = &device.claimable_by {
        format!("Hypercolor has a native protocol in the disabled {driver} driver.")
    } else {
        "No enabled Hypercolor native driver claims this device. Other support has not been verified.".to_owned()
    };
    let fields = [
        ("template", "device-support.yml".to_owned()),
        ("title", format!("[device] {vendor} {model}")),
        ("vendor", vendor.to_owned()),
        ("model", model.to_owned()),
        (
            "vid-pid",
            format!("{:04X}:{:04X}", device.vendor_id, device.product_id),
        ),
        ("platform", platform.to_owned()),
        ("existing-support", support),
    ];
    let query = fields
        .into_iter()
        .map(|(key, value)| format!("{key}={}", crate::control_surface_api::path_segment(&value)))
        .collect::<Vec<_>>()
        .join("&");
    format!("https://github.com/hyperb1iss/hypercolor/issues/new?{query}")
}

pub fn unclaimed_resource() -> LocalResource<api::ApiResult<Vec<UnclaimedDevice>>> {
    let hint = expect_context::<WsContext>().last_device_event;
    let inventory = api::daemon_resource(api::fetch_unclaimed_devices);
    Effect::new(move |_| {
        if hint
            .get()
            .is_some_and(|hint| hint.event_type == "unclaimed_devices_changed")
        {
            inventory.refetch();
        }
    });
    inventory
}

#[component]
pub fn UnclaimedHardware() -> impl IntoView {
    let inventory = unclaimed_resource();
    let status = api::openrgb::status_resource();
    let devices_resource = expect_context::<crate::app::DevicesContext>().devices_resource;

    view! {
        <section class="mt-6 space-y-3" aria-label="Unclaimed hardware">
            {move || match inventory.get() {
                Some(Ok(devices)) if !devices.is_empty() => view! {
                    <h2 class="text-base font-semibold text-fg-primary">"Unclaimed hardware"</h2>
                    <p class="text-sm text-fg-secondary">"USB hardware without an enabled native driver. Some devices may not have lighting."</p>
                    <div class="space-y-2">
                        {devices.into_iter().map(|device| view! {
                            <UnclaimedRow device=device status=status devices=devices_resource />
                        }).collect_view()}
                    </div>
                }.into_any(),
                Some(Err(error)) => view! { <p class="text-sm text-status-warning">{format!("Hardware inventory unavailable: {error}")}</p> }.into_any(),
                _ => ().into_any(),
            }}
        </section>
    }
}

#[component]
fn UnclaimedRow(
    device: UnclaimedDevice,
    status: LocalResource<api::ApiResult<hypercolor_types::api::system::OpenRgbStatus>>,
    devices: LocalResource<api::ApiResult<Vec<api::DeviceSummary>>>,
) -> impl IntoView {
    let title = format!(
        "{} {}",
        device.manufacturer.as_deref().unwrap_or("Unknown vendor"),
        device.product.as_deref().unwrap_or("USB device")
    );
    let ids = format!("{:04X}:{:04X}", device.vendor_id, device.product_id);
    let device_for_match = device.clone();
    let available = Signal::derive(move || {
        let Some(Ok(status)) = status.get() else {
            return false;
        };
        let Some(Ok(devices)) = devices.get() else {
            return false;
        };
        bridge_available(&device_for_match, &status, &devices)
    });
    let device_for_link = device.clone();
    let support_url = Signal::derive(move || {
        let platform = status
            .get()
            .and_then(Result::ok)
            .map(|status| status.platform)
            .unwrap_or_default();
        support_issue_url(&device_for_link, &platform, available.get())
    });
    let open_support = move |event: leptos::ev::MouseEvent| {
        if !crate::tauri_bridge::is_tauri_available() {
            return;
        }
        event.prevent_default();
        let url = support_url.get_untracked();
        leptos::task::spawn_local(async move {
            if let Err(error) = crate::tauri_bridge::open_external_url(&url).await {
                crate::toasts::toast_error(&format!("Could not open support request: {error}"));
            }
        });
    };

    view! {
        <div class="flex flex-wrap items-center justify-between gap-3 rounded-lg border border-edge-subtle bg-surface-overlay p-3">
            <div class="min-w-0">
                <h3 class="text-sm font-medium text-fg-primary">{title}</h3>
                <p class="text-xs font-mono text-fg-tertiary">{ids}</p>
                {device.claimable_by.map(|driver| view! { <p class="mt-1 text-xs text-fg-secondary">{format!("Native support available: enable the {driver} driver in Settings.")}</p> })}
            </div>
            <Show when=move || available.get()>
                <span class="text-xs text-fg-secondary">"Available via OpenRGB"</span>
            </Show>
            <a class="rounded-md border border-edge-subtle px-3 py-2 text-xs text-accent-purple hover:bg-surface-hover focus-visible:outline"
                href=move || support_url.get() on:click=open_support target="_blank" rel="noopener noreferrer">"Request support"</a>
        </div>
    }
}

/// A matching serial is offered only while its own endpoint is reachable.
pub fn bridge_available(
    device: &UnclaimedDevice,
    status: &hypercolor_types::api::system::OpenRgbStatus,
    devices: &[api::DeviceSummary],
) -> bool {
    let Some(id) = matching_bridge(device, &status.coverage) else {
        return false;
    };
    let Some(bridge) = devices
        .iter()
        .find(|device| device.id == id)
        .and_then(|device| device.bridge.as_ref())
    else {
        return false;
    };
    status
        .probes
        .iter()
        .any(|probe| probe.reachable && bridge.endpoint.as_deref() == Some(probe.endpoint.as_str()))
}
