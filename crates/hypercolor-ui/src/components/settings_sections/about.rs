use leptos::prelude::*;
use leptos_icons::Icon;

use crate::api;
use crate::components::settings_controls::*;
use crate::icons::*;
use crate::live_uptime::{format_uptime, use_live_uptime};

// ── About ──────────────────────────────────────────────────────────────────

#[component]
pub fn AboutSection() -> impl IntoView {
    let status = api::daemon_resource(api::fetch_status);

    view! {
        <section id="section-about" class="pt-5 pb-3 space-y-0">
            <SectionHeader title="About" icon=LuInfo />

            {move || {
                let stat = status.get().and_then(|r| r.ok());
                let live_uptime = stat.as_ref().map(|s| use_live_uptime(s.uptime_seconds));
                let uptime = Signal::derive(move || {
                    live_uptime.map_or_else(|| "—".to_string(), |uptime| format_uptime(uptime.get()))
                });
                view! {
                    <div class="space-y-3">
                        <AboutRow label="Version" value=stat.as_ref().map(|s| s.version.clone()).unwrap_or_else(|| "—".to_string()) />
                        <AboutRow label="Uptime" value=uptime />
                        <AboutRow label="Devices" value=stat.as_ref().map(|s| s.device_count.to_string()).unwrap_or_else(|| "—".to_string()) />
                        <AboutRow label="Effects" value=stat.as_ref().map(|s| s.effect_count.to_string()).unwrap_or_else(|| "—".to_string()) />
                        <AboutRow label="Config" value=stat.as_ref().map(|s| s.config_path.clone()).unwrap_or_else(|| "—".to_string()) />
                    </div>
                }
            }}

            <div class="flex items-center gap-4 mt-4 pt-3 border-t border-edge-subtle/10">
                <a
                    href="https://github.com/hyperb1iss/hypercolor"
                    target="_blank"
                    rel="noopener"
                    class="flex items-center gap-1.5 text-xs text-fg-tertiary hover:text-accent transition-colors"
                >
                    <Icon icon=LuExternalLink width="11px" height="11px" />
                    "GitHub"
                </a>
                <span class="text-[10px] text-fg-tertiary/30">"Apache-2.0"</span>
            </div>
        </section>
    }
}

#[component]
fn AboutRow(label: &'static str, #[prop(into)] value: Signal<String>) -> impl IntoView {
    view! {
        <div class="flex items-center justify-between py-2 setting-row">
            <span class="text-sm text-fg-tertiary">{label}</span>
            <span class="text-sm text-fg-primary font-mono">{value}</span>
        </div>
    }
}
