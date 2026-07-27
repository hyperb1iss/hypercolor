use hypercolor_types::config::{HypercolorConfig, InteractionRoutePolicy};
use leptos::prelude::*;

use super::read_config;
use crate::api::{self, InputSourceStatus, InputStatus};
use crate::app::WsContext;
use crate::components::settings_controls::{
    SectionHeader, SectionReset, SettingDropdown, SettingToggle,
};
use crate::icons::LuKeyboard;
use crate::input_access::{
    InputPipelineState, input_pipeline_state, input_status_epoch, input_status_remediation,
    primary_input_source_issue,
};

#[component]
pub fn InputSection(
    #[prop(into)] config: Signal<Option<HypercolorConfig>>,
    on_change: Callback<(String, serde_json::Value)>,
    on_reset: Callback<String>,
) -> impl IntoView {
    let ws = expect_context::<WsContext>();
    let enabled = Signal::derive(move || read_config(config, |cfg| cfg.input.enabled));
    let keyboard = Signal::derive(move || read_config(config, |cfg| cfg.input.keyboard));
    let mouse = Signal::derive(move || read_config(config, |cfg| cfg.input.mouse));
    let daemon_route = Signal::derive(move || {
        route_value(config.with(|current| {
            current
                .as_ref()
                .map_or(InteractionRoutePolicy::Host, |cfg| cfg.input.daemon_route)
        }))
    });
    let preview_route = Signal::derive(move || {
        route_value(config.with(|current| {
            current
                .as_ref()
                .map_or(InteractionRoutePolicy::Browser, |cfg| {
                    cfg.input.preview_route
                })
        }))
    });
    let route_options = Signal::stored(vec![
        ("host".to_owned(), "Host only".to_owned()),
        ("browser".to_owned(), "Addressed browser".to_owned()),
        ("merge".to_owned(), "Merge host + browser".to_owned()),
    ]);

    let input_status = LocalResource::new(move || {
        let connection_generation = ws.connection_generation.get();
        let source_event = ws.last_input_source_status_event.get();
        let epoch = config.with(|current| {
            input_status_epoch(connection_generation, source_event, current.as_ref())
        });
        async move {
            let _ = epoch;
            api::fetch_status().await.map(|status| status.input)
        }
    });

    view! {
        <section id="section-input" class="pt-5 pb-3 space-y-0">
            <SectionHeader title="Input Access" icon=LuKeyboard />
            <SettingToggle
                label="Host input access"
                description="Allow explicitly input-reactive effects to receive host keyboard or pointer state"
                key="input.enabled"
                value=enabled
                on_change=on_change
            />
            <SettingToggle
                label="Keyboard"
                description="Capture physical key state and ordered key events while an effect demands them"
                key="input.keyboard"
                value=keyboard
                on_change=on_change
            />
            <SettingToggle
                label="Mouse"
                description="Capture pointer position, buttons, wheel travel, and motion while demanded"
                key="input.mouse"
                value=mouse
                on_change=on_change
            />
            <SettingDropdown
                label="Daemon effects"
                description="Choose which interaction source authoritative device output receives"
                key="input.daemon_route"
                value=daemon_route
                options=route_options
                on_change=on_change
            />
            <SettingDropdown
                label="Interactive previews"
                description="Route each preview to its addressed browser input, host input, or both"
                key="input.preview_route"
                value=preview_route
                options=route_options
                on_change=on_change
            />

            <div class="mt-3 rounded-xl border border-edge-subtle bg-surface-sunken/40 px-4 py-3">
                <div class="mb-2 text-[11px] font-semibold uppercase tracking-[0.14em] text-fg-tertiary">
                    "Source health"
                </div>
                {move || match input_status.get() {
                    None => view! {
                        <div class="text-xs text-fg-tertiary">"Reading input health..."</div>
                    }.into_any(),
                    Some(Err(error)) => view! {
                        <div class="text-xs text-status-error">{format!("Input health unavailable: {error}")}</div>
                    }.into_any(),
                    Some(Ok(status)) => input_status_view(status).into_any(),
                }}
            </div>
            <SectionReset
                section_label="Input"
                on_reset=Callback::new(move |()| on_reset.run("input".to_owned()))
            />
        </section>
    }
}

fn input_status_view(status: InputStatus) -> impl IntoView {
    let pipeline_state = input_pipeline_state(&status);
    let (label, detail, class) = match pipeline_state {
        InputPipelineState::ConsentOff => (
            "Consent off",
            "No host input backend opens until access is enabled.",
            "border-edge-subtle bg-surface-overlay/40 text-fg-tertiary",
        ),
        InputPipelineState::Live => (
            "Capturing",
            "A demanded input source is live.",
            "border-status-success/30 bg-status-success/10 text-status-success",
        ),
        InputPipelineState::Ready => (
            "Ready, idle",
            "Permission is granted; capture starts only when an effect demands it.",
            "border-status-info/30 bg-status-info/10 text-status-info",
        ),
        InputPipelineState::Degraded => (
            "Needs attention",
            "A configured or demanded source is degraded.",
            "border-status-warning/30 bg-status-warning/10 text-status-warning",
        ),
        InputPipelineState::Unavailable => (
            "Unavailable",
            "No host input backend is available in this session.",
            "border-status-error/30 bg-status-error/10 text-status-error",
        ),
    };
    let remediation = input_status_remediation(&status);
    let sources = status
        .sources
        .into_iter()
        .filter(|source| !source.retired)
        .collect::<Vec<_>>();

    view! {
        <div class="space-y-2.5">
            <div class="flex flex-wrap items-center gap-2">
                <span class=format!("rounded-md border px-2 py-1 text-[11px] font-medium {class}")>
                    {label}
                </span>
                <span class="text-xs text-fg-secondary">{detail}</span>
            </div>
            {remediation.map(|message| view! {
                <div class="rounded-lg border border-status-warning/24 bg-status-warning/8 px-3 py-2 text-xs text-status-warning">
                    {message}
                </div>
            })}
            {if sources.is_empty() {
                view! {
                    <div class="text-xs text-fg-tertiary">
                        "No input sources are registered for this platform session."
                    </div>
                }.into_any()
            } else {
                view! {
                    <div class="space-y-2">
                        {sources.into_iter().map(input_source_view).collect_view()}
                    </div>
                }.into_any()
            }}
        </div>
    }
}

fn input_source_view(source: InputSourceStatus) -> impl IntoView {
    let issue = primary_input_source_issue(&source);
    let issue_message = issue.map(|issue| issue.message.clone());
    let source_remediation = issue.and_then(|issue| issue.remediation.clone());
    let state_class = if issue.is_some()
        || matches!(source.state.as_str(), "failed" | "degraded" | "unavailable")
        || (source.demanded && source.freshness == "stale")
    {
        "text-status-warning"
    } else if source.demanded && source.state == "live" {
        "text-status-success"
    } else {
        "text-fg-tertiary"
    };
    let demand = if source.demanded { "demanded" } else { "idle" };
    let consent = if source.consented {
        "consented"
    } else {
        "not consented"
    };
    let configured = if source.configured {
        "configured"
    } else {
        "disabled"
    };
    let age = source
        .last_sample_age_ms
        .map(|age| format!(" · sample {age} ms ago"))
        .unwrap_or_default();

    view! {
        <div class="rounded-lg border border-edge-subtle/60 bg-surface-overlay/30 px-3 py-2.5">
            <div class="flex flex-wrap items-center justify-between gap-2">
                <div class="min-w-0">
                    <div class="truncate text-xs font-medium text-fg-primary">{source.source_id}</div>
                    <div class="text-[11px] text-fg-tertiary">
                        {format!("{} · {}", source.kind, source.backend)}
                    </div>
                </div>
                <span class=format!("text-[11px] font-medium {state_class}")>
                    {humanize(&source.state)}
                </span>
            </div>
            <div class="mt-1.5 text-[11px] text-fg-tertiary">
                {format!(
                    "{configured} · {consent} · {demand} · freshness {}{age}",
                    humanize(&source.freshness),
                )}
            </div>
            {issue_message.map(|message| view! {
                <div class="mt-1.5 text-[11px] text-status-warning">{message}</div>
            })}
            {source_remediation.map(|message| view! {
                <div class="mt-1 text-[11px] text-fg-secondary">{message}</div>
            })}
        </div>
    }
}

fn route_value(route: InteractionRoutePolicy) -> String {
    match route {
        InteractionRoutePolicy::Host => "host",
        InteractionRoutePolicy::Browser => "browser",
        InteractionRoutePolicy::Merge => "merge",
    }
    .to_owned()
}

fn humanize(value: &str) -> String {
    let mut words = value.replace('_', " ");
    if let Some(first) = words.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    words
}
