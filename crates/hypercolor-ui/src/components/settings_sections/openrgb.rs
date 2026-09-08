//! OpenRGB fallback guidance inside Discovery settings.
use crate::components::settings_controls::SettingDropdown;
use crate::{api, tauri_bridge};
use hypercolor_types::api::system::OpenRgbStatus;
use leptos::prelude::*;

#[component]
pub(super) fn OpenRgbCard(on_change: Callback<(String, serde_json::Value)>) -> impl IntoView {
    let status = api::openrgb::status_resource();
    let hints = api::daemon_resource(move || {
        let missing = status
            .get()
            .is_some_and(|result| result.is_ok_and(|status| status.binary_path.is_none()));
        async move {
            if missing {
                tauri_bridge::openrgb_install_hints().await
            } else {
                Ok(None)
            }
        }
    });
    let mode = Signal::derive(move || {
        status
            .get()
            .and_then(Result::ok)
            .and_then(|status| {
                status
                    .bridge_config
                    .settings
                    .get("ownership")
                    .and_then(|ownership| ownership.get("mode"))
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| "disabled".to_owned())
    });
    view! {
        <div class="my-4 rounded-xl border border-edge-subtle bg-surface-overlay p-4 space-y-3">
            <div class="flex items-center justify-between gap-3">
                <h3 class="text-sm font-semibold text-fg-primary">"OpenRGB fallback"</h3>
                <button type="button" class="rounded-md px-3 py-2 text-xs text-accent-purple hover:bg-surface-hover" on:click=move |_| { status.refetch(); hints.refetch(); }>"Check again"</button>
            </div>
            <p class="text-xs text-fg-secondary">"Native drivers own supported devices. Use OpenRGB for hardware they cannot drive, or devices you explicitly release."</p>
            {move || match status.get() {
                None => view! { <p class="text-xs text-fg-tertiary">"Checking OpenRGB…"</p> }.into_any(),
                Some(Err(error)) => view! { <p class="text-xs text-status-warning">{format!("OpenRGB status unavailable: {error}")}</p> }.into_any(),
                Some(Ok(status)) => view! {
                    <OpenRgbStatusView status=status desktop_hints_ready=Signal::derive(move || matches!(hints.get(), Some(Ok(Some(_))))) />
                }.into_any(),
            }}
            {move || match hints.get() {
                Some(Ok(Some(hints))) => view! {
                    <div class="space-y-2">
                        <p class="text-xs font-medium text-fg-primary">"Install on this computer"</p>
                        {hints.into_iter().map(|hint| view! {
                            <div><code class="block break-all text-xs text-fg-primary">{hint.command}</code><p class="text-xs text-fg-secondary">{hint.note}</p></div>
                        }).collect_view()}
                    </div>
                }.into_any(),
                Some(Err(error)) => view! { <p class="text-xs text-status-warning">{format!("Desktop install guidance unavailable: {error}")}</p> }.into_any(),
                _ => ().into_any(),
            }}
            <SettingDropdown label="Ownership" description="Partitioned output uses the configured detector allowlist. All bridge devices still yield to active native drivers."
                key="drivers.openrgb.ownership.mode" value=mode
                options=Signal::derive(|| vec![("disabled".to_owned(), "Off".to_owned()), ("detector_partitioned".to_owned(), "Partitioned".to_owned()), ("open_rgb_owned".to_owned(), "Bridge devices".to_owned())])
                on_change=on_change />
        </div>
    }
}

#[component]
fn OpenRgbStatusView(status: OpenRgbStatus, desktop_hints_ready: Signal<bool>) -> impl IntoView {
    let missing = status.binary_path.is_none();
    let host_hints = status.install_hints.clone();
    view! {
        <p class="text-xs text-fg-secondary">{if status.enabled { "Bridge driver enabled" } else { "Bridge driver disabled" }}</p>
        {status.binary_version.map(|version| view! { <p class="text-xs text-fg-secondary">{format!("Installed version: {version}")}</p> })}
        <div class="space-y-2">
            {status.probes.into_iter().map(|probe| view! {
                <div class="rounded-lg border border-edge-subtle p-3">
                    <p class="text-sm text-fg-primary break-all">{format!("{}: {}", probe.endpoint, if probe.reachable { "Reachable" } else { "Unreachable" })}</p>
                    <p class="text-xs text-fg-secondary">{format!("Protocol: {} · Controllers: {}", probe.protocol_version.map_or_else(|| "unknown".to_owned(), |v| v.to_string()), probe.controller_count.map_or_else(|| "unknown".to_owned(), |v| v.to_string()))}</p>
                    {probe.error.map(|error| view! { <p class="mt-1 text-xs text-status-warning break-words">{error}</p> })}
                </div>
            }).collect_view()}
        </div>
        <p class="text-xs text-fg-secondary">{format!("{} routes have output disabled", status.output_disabled_count)}</p>
        <Show when=move || missing && !desktop_hints_ready.get()>
            <div class="space-y-2"><p class="text-xs font-medium text-fg-primary">"Install on the daemon host"</p>
                {host_hints.iter().cloned().map(|hint| view! {
                    <div><code class="block break-all text-xs text-fg-primary">{hint.command}</code><p class="text-xs text-fg-secondary">{hint.note}</p></div>
                }).collect_view()}
            </div>
        </Show>
        {status.permission_checks.into_iter().filter(|check| !check.ok).map(|check| view! {
            <div class="text-xs"><p class="text-status-warning">{check.detail}</p>{check.remedy.map(|remedy| view! { <code class="block break-all text-fg-secondary">{remedy}</code> })}</div>
        }).collect_view()}
    }
}
