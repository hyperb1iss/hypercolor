//! Bridge identity and output state shared by the gallery and detail drawer.
use crate::api::DeviceSummary;
use leptos::prelude::*;

#[component]
pub fn BridgeStatus(device: DeviceSummary) -> impl IntoView {
    let bridge = device.presentation.icon.as_deref() == Some("bridge");
    let reason = device
        .bridge
        .filter(|bridge| !bridge.output_enabled)
        .map(|bridge| {
            bridge
                .disabled_reason
                .unwrap_or_else(|| "Bridge output is disabled".to_owned())
        });
    view! {
        {bridge.then(|| view! {
            <span class="inline-flex rounded border border-edge-subtle bg-surface-overlay px-1.5 py-0.5 text-[10px] font-medium text-fg-secondary">"OpenRGB bridge"</span>
        })}
        {reason.map(|reason| view! {
            <p class="mt-1 text-xs text-status-warning break-words">{reason}</p>
        })}
    }
}
