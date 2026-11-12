//! Hardware coverage that starts with the host inventory, not browser probes.
use crate::{
    api,
    app::{DaemonStatusContext, WsContext},
};
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

/// `vendor model`, without repeating a vendor the product string already
/// leads with (Razer reports "Razer Kraken Ultimate", not "Kraken Ultimate").
pub fn vendor_and_model(vendor: &str, model: &str) -> String {
    let leads_with_vendor = model
        .get(..vendor.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(vendor))
        && model[vendor.len()..]
            .chars()
            .next()
            .is_none_or(char::is_whitespace);
    if leads_with_vendor {
        model.to_owned()
    } else {
        format!("{vendor} {model}")
    }
}

/// Prefill detected facts, leaving product identity and lighting questions to
/// the owner. Serial numbers and host paths never enter the public issue URL.
pub fn support_issue_url(
    device: &UnclaimedDevice,
    platform: &str,
    available: bool,
    daemon_version: Option<&str>,
) -> String {
    let platform = match platform.to_ascii_lowercase().as_str() {
        "linux" => "Linux",
        "macos" | "darwin" => "macOS",
        "windows" => "Windows",
        _ => "",
    };
    let native = if let Some(driver) = &device.claimable_by {
        format!(
            "Matched disabled driver: {driver} (experimental: {}).",
            device.experimental
        )
    } else {
        "No native driver match reported.".to_owned()
    };
    let bridge = if available {
        "A reachable OpenRGB server lists a controller with a matching serial. Lighting control has not been verified."
    } else {
        "No matching controller on a reachable OpenRGB server was confirmed. Support is unknown."
    };
    let interfaces = if device.interface_classes.is_empty() {
        "Not reported".to_owned()
    } else {
        device
            .interface_classes
            .iter()
            .map(|class| format!("0x{class:02X}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let detected = format!(
        "USB vendor label: {}\nUSB product string: {}\nUSB interface classes: {interfaces}\nNative driver: {native}\nOpenRGB: {bridge}\nWeb UI version: {}\n\nUSB detection does not establish lighting capability or the retail brand/model.",
        device.vendor_label(),
        device.product.as_deref().unwrap_or("Not reported"),
        env!("CARGO_PKG_VERSION"),
    );
    let mut fields = vec![
        ("template", "device-support.yml".to_owned()),
        (
            "vid-pid",
            format!("{:04X}:{:04X}", device.vendor_id, device.product_id),
        ),
        ("detected-device", detected),
    ];
    if !platform.is_empty() {
        fields.push(("platform", platform.to_owned()));
    }
    if let Some(version) = daemon_version
        .map(str::trim)
        .filter(|version| !version.is_empty())
    {
        fields.push(("version", version.to_owned()));
    }
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
                    <p class="text-sm text-fg-secondary">"USB detection does not confirm controllable lighting or a display. To request support, tell us the product's brand and model, what you want to control, and how you control it today."</p>
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
    let daemon = expect_context::<DaemonStatusContext>();
    let title = vendor_and_model(
        &device.vendor_label(),
        device.product.as_deref().unwrap_or("USB device"),
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
        daemon.status.with(|snapshot| {
            support_issue_url(
                &device_for_link,
                &platform,
                available.get(),
                snapshot.as_ref().map(|status| status.version.as_str()),
            )
        })
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
                href=move || support_url.get() on:click=open_support target="_blank" rel="noopener noreferrer">"Request device support"</a>
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
