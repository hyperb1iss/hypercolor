use hypercolor_types::config::{HypercolorConfig, InteractionRoutePolicy};
use leptos::prelude::*;

use super::{MacosCaptureOwnerRestartAction, read_config, validate_macos_restart_owner};
use crate::api::{self, InputSourcePlatformStatus, InputSourceStatus, InputStatus};
use crate::app::WsContext;
use crate::components::settings_controls::{
    AdvancedDisclosure, SectionHeader, SectionReset, SettingDropdown, SettingToggle,
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
        let owner_event = ws.last_macos_daemon_ownership_event.get();
        let epoch = config.with(|current| {
            input_status_epoch(connection_generation, source_event, current.as_ref())
        });
        async move {
            let _ = (epoch, owner_event);
            api::fetch_status().await
        }
    });
    let (authorizing, set_authorizing) = signal(false);
    let (authorization_error, set_authorization_error) = signal(None::<String>);
    let authorize_keyboard = move |_| {
        if authorizing.get_untracked() {
            return;
        }
        set_authorizing.set(true);
        set_authorization_error.set(None);
        leptos::task::spawn_local(async move {
            match api::authorize_input_monitoring().await {
                Ok(()) => input_status.refetch(),
                Err(error) => set_authorization_error.set(Some(error)),
            }
            set_authorizing.set(false);
        });
    };

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
            <AdvancedDisclosure>
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
            </AdvancedDisclosure>

            <Show when=move || {
                input_status
                    .get()
                    .and_then(Result::ok)
                    .is_some_and(|status| macos_keyboard_needs_authorization(&status.input))
            }>
                <div class="flex items-center justify-between gap-4 border-b border-edge-subtle/40 py-3">
                    <div class="min-w-0">
                        <div class="text-[13px] text-fg-primary">"Input Monitoring"</div>
                        <div class="text-xs text-fg-tertiary">
                            "Keyboard effects need this macOS permission. Pointer-only effects do not."
                        </div>
                    </div>
                    <button
                        type="button"
                        class="glow-ring inline-flex shrink-0 items-center rounded-md border border-accent-muted bg-accent-subtle px-2.5 py-1.5 text-xs text-accent hover:bg-accent-muted/20 disabled:opacity-50"
                        disabled=move || authorizing.get()
                        on:click=authorize_keyboard
                    >
                        {move || if authorizing.get() { "Requesting…" } else { "Authorize" }}
                    </button>
                </div>
            </Show>
            {move || input_status
                .get()
                .and_then(Result::ok)
                .and_then(|status| macos_keyboard_restart_coordinates(&status))
                .map(|(owner, epoch)| view! {
                    <MacosCaptureOwnerRestartAction
                        owner=owner
                        epoch=epoch
                        on_complete=Callback::new(move |()| input_status.refetch())
                    />
                })}
            {move || authorization_error.get().map(|error| view! {
                <div class="mt-2 rounded-lg border border-status-error/24 bg-status-error/8 px-3 py-2 text-xs text-status-error">
                    {error}
                </div>
            })}

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
                    Some(Ok(status)) => input_status_view(status.input).into_any(),
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
        .filter(|source| !source.retired && !is_screen_source(source))
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

pub(super) fn input_source_view(source: InputSourceStatus) -> impl IntoView {
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
    let platform = source.platform.clone();

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
            {platform.map(platform_status_view)}
        </div>
    }
}

pub(super) fn platform_status_view(platform: InputSourcePlatformStatus) -> impl IntoView {
    match platform {
        InputSourcePlatformStatus::MacosInput {
            keyboard,
            pointer,
            keyboard_tcc,
            keyboard_owner,
            pointer_owner,
            owner_conflict,
        } => {
            let state = format!(
                "Keyboard {} · pointer {} · Input Monitoring {}",
                humanize_optional(keyboard.as_deref()),
                humanize_optional(pointer.as_deref()),
                humanize_optional(keyboard_tcc.as_deref()),
            );
            let owners = format!(
                "Keyboard owner {} · pointer owner {}",
                humanize_optional(keyboard_owner.as_deref()),
                humanize_optional(pointer_owner.as_deref()),
            );
            view! {
                <div class="mt-2 border-t border-edge-subtle/40 pt-2 text-[11px] text-fg-secondary">
                    <div>{state}</div>
                    <div class="mt-0.5 text-fg-tertiary">{owners}</div>
                    {owner_conflict.map(|conflict| view! {
                        <div class="mt-1 text-status-warning">
                            {format!(
                                "Owner conflict: {} is active; {} also attempted startup.",
                                humanize_optional(conflict.active.as_deref()),
                                humanize_optional(conflict.contender.as_deref()),
                            )}
                        </div>
                    })}
                </div>
            }
            .into_any()
        }
        InputSourcePlatformStatus::MacosScreen {
            state,
            tcc,
            owner,
            selection,
            tahoe_selection,
            owner_conflict,
        } => {
            let selection = selection
                .as_ref()
                .map(screen_selection_label)
                .unwrap_or_else(|| "Unknown selection".to_owned());
            let range = tahoe_selection
                .and_then(|capabilities| capabilities.hdr_capture)
                .map_or(
                    "Dynamic range pending",
                    |hdr| if hdr { "HDR" } else { "SDR" },
                );
            view! {
                <div class="mt-2 border-t border-edge-subtle/40 pt-2 text-[11px] text-fg-secondary">
                    <div>{format!(
                        "Screen {} · Screen Recording {} · owner {}",
                        humanize_optional(state.as_deref()),
                        humanize_optional(tcc.as_deref()),
                        humanize_optional(owner.as_deref()),
                    )}</div>
                    <div class="mt-0.5 text-fg-tertiary">{format!("{selection} · {range}")}</div>
                    {owner_conflict.map(|conflict| view! {
                        <div class="mt-1 text-status-warning">
                            {format!(
                                "Owner conflict: {} is active; {} also attempted startup.",
                                humanize_optional(conflict.active.as_deref()),
                                humanize_optional(conflict.contender.as_deref()),
                            )}
                        </div>
                    })}
                </div>
            }
            .into_any()
        }
        InputSourcePlatformStatus::Unknown => view! {
            <div class="mt-2 border-t border-edge-subtle/40 pt-2 text-[11px] text-fg-tertiary">
                "Platform details require a newer Hypercolor app."
            </div>
        }
        .into_any(),
    }
}

fn screen_selection_label(selection: &crate::api::MacosSelectionStatus) -> String {
    match selection {
        crate::api::MacosSelectionStatus::None => "No source selected".to_owned(),
        crate::api::MacosSelectionStatus::Display { source_id } => {
            source_id.as_deref().map_or_else(
                || "Display selected".to_owned(),
                |id| format!("Display {id}"),
            )
        }
        crate::api::MacosSelectionStatus::SessionScoped { content_style } => {
            content_style.as_deref().map_or_else(
                || "Session-scoped source".to_owned(),
                |style| format!("Session-scoped {}", humanize(style)),
            )
        }
        crate::api::MacosSelectionStatus::Unknown => "Unknown selection".to_owned(),
    }
}

fn humanize_optional(value: Option<&str>) -> String {
    value.map_or_else(|| "unknown".to_owned(), humanize)
}

pub(super) fn macos_keyboard_needs_authorization(status: &InputStatus) -> bool {
    status.sources.iter().any(|source| {
        if source.retired {
            return false;
        }
        let Some(InputSourcePlatformStatus::MacosInput {
            keyboard,
            keyboard_tcc,
            ..
        }) = source.platform.as_ref()
        else {
            return false;
        };
        matches!(
            keyboard.as_deref(),
            Some("needs_user_action" | "permission_denied" | "revoked")
        ) || matches!(
            keyboard_tcc.as_deref(),
            Some("not_determined" | "denied" | "revoked")
        )
    })
}

fn macos_keyboard_restart_coordinates(status: &crate::api::SystemStatus) -> Option<(String, u64)> {
    let needs_restart = status.input.sources.iter().any(|source| {
        if source.retired {
            return false;
        }
        matches!(
            source.platform.as_ref(),
            Some(InputSourcePlatformStatus::MacosInput { keyboard, .. })
                if keyboard.as_deref() == Some("needs_process_restart")
        )
    });
    needs_restart
        .then_some(status.macos_daemon_ownership.as_ref())
        .flatten()
        .and_then(|ownership| {
            Some((
                validate_macos_restart_owner(ownership.active_owner.as_deref()?)?,
                ownership.owner_epoch?,
            ))
        })
}

fn is_screen_source(source: &InputSourceStatus) -> bool {
    source.kind == "screen"
        || matches!(
            source.platform,
            Some(InputSourcePlatformStatus::MacosScreen { .. })
        )
}

fn route_value(route: InteractionRoutePolicy) -> String {
    match route {
        InteractionRoutePolicy::Host => "host",
        InteractionRoutePolicy::Browser => "browser",
        InteractionRoutePolicy::Merge => "merge",
    }
    .to_owned()
}

pub(super) fn humanize(value: &str) -> String {
    let mut words = value.replace('_', " ");
    if let Some(first) = words.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    words
}

#[cfg(test)]
mod tests {
    use crate::api::{
        InputSourcePlatformStatus, InputSourceStatus, InputStatus, MacosDaemonOwnershipStatus,
        SystemStatus,
    };

    use super::{macos_keyboard_needs_authorization, macos_keyboard_restart_coordinates};

    fn system_status(
        input: InputStatus,
        macos_daemon_ownership: Option<MacosDaemonOwnershipStatus>,
    ) -> SystemStatus {
        SystemStatus {
            running: true,
            version: "test".to_owned(),
            config_path: String::new(),
            uptime_seconds: 1,
            device_count: 0,
            effect_count: 0,
            active_effect: None,
            active_scene: None,
            active_scene_snapshot_locked: false,
            global_brightness: 100,
            compositor_acceleration: crate::api::RenderAccelerationStatus::default(),
            render_loop: crate::api::RenderLoopStatus::default(),
            capabilities: Vec::new(),
            input,
            macos_daemon_ownership,
        }
    }

    #[test]
    fn keyboard_authorization_action_tracks_only_protected_keyboard_state() {
        let mut status = InputStatus {
            sources: vec![InputSourceStatus {
                platform: Some(InputSourcePlatformStatus::MacosInput {
                    keyboard: Some("needs_user_action".to_owned()),
                    pointer: Some("live".to_owned()),
                    keyboard_tcc: Some("not_determined".to_owned()),
                    keyboard_owner: Some("app_sidecar".to_owned()),
                    pointer_owner: Some("app_sidecar".to_owned()),
                    owner_conflict: None,
                }),
                ..InputSourceStatus::default()
            }],
            ..InputStatus::default()
        };
        assert!(macos_keyboard_needs_authorization(&status));

        status.sources[0].retired = true;
        assert!(!macos_keyboard_needs_authorization(&status));
        status.sources[0].retired = false;

        status.sources[0].platform = Some(InputSourcePlatformStatus::MacosInput {
            keyboard: Some("live".to_owned()),
            pointer: Some("live".to_owned()),
            keyboard_tcc: Some("authorized".to_owned()),
            keyboard_owner: Some("app_sidecar".to_owned()),
            pointer_owner: Some("app_sidecar".to_owned()),
            owner_conflict: None,
        });
        assert!(!macos_keyboard_needs_authorization(&status));
    }

    #[test]
    fn screen_authorization_action_tracks_only_screen_recording_state() {
        let mut status = InputStatus {
            sources: vec![InputSourceStatus {
                kind: "screen".to_owned(),
                platform: Some(InputSourcePlatformStatus::MacosScreen {
                    state: Some("permission_denied".to_owned()),
                    tcc: Some("denied".to_owned()),
                    owner: Some("app_sidecar".to_owned()),
                    selection: None,
                    tahoe_selection: None,
                    owner_conflict: None,
                }),
                ..InputSourceStatus::default()
            }],
            ..InputStatus::default()
        };
        assert!(super::super::macos_screen_needs_authorization(&status));

        status.sources[0].retired = true;
        assert!(!super::super::macos_screen_needs_authorization(&status));
        status.sources[0].retired = false;

        status.sources[0].platform = Some(InputSourcePlatformStatus::MacosScreen {
            state: Some("live".to_owned()),
            tcc: Some("authorized".to_owned()),
            owner: Some("app_sidecar".to_owned()),
            selection: None,
            tahoe_selection: None,
            owner_conflict: None,
        });
        assert!(!super::super::macos_screen_needs_authorization(&status));
    }

    #[test]
    fn restart_coordinates_require_exact_state_owner_and_epoch() {
        let mut status = system_status(
            InputStatus {
                sources: vec![InputSourceStatus {
                    platform: Some(InputSourcePlatformStatus::MacosInput {
                        keyboard: Some("needs_process_restart".to_owned()),
                        pointer: Some("live".to_owned()),
                        keyboard_tcc: Some("authorized".to_owned()),
                        keyboard_owner: Some("homebrew_service".to_owned()),
                        pointer_owner: Some("homebrew_service".to_owned()),
                        owner_conflict: None,
                    }),
                    ..InputSourceStatus::default()
                }],
                ..InputStatus::default()
            },
            Some(MacosDaemonOwnershipStatus {
                active_owner: Some("homebrew_service".to_owned()),
                owner_epoch: Some(29),
                ..MacosDaemonOwnershipStatus::default()
            }),
        );

        assert_eq!(
            macos_keyboard_restart_coordinates(&status),
            Some(("homebrew_service".to_owned(), 29))
        );
        status.input.sources[0].retired = true;
        assert_eq!(macos_keyboard_restart_coordinates(&status), None);
        status.input.sources[0].retired = false;
        status.input.sources[0].platform = Some(InputSourcePlatformStatus::MacosInput {
            keyboard: Some("permission_denied".to_owned()),
            pointer: Some("live".to_owned()),
            keyboard_tcc: Some("denied".to_owned()),
            keyboard_owner: Some("homebrew_service".to_owned()),
            pointer_owner: Some("homebrew_service".to_owned()),
            owner_conflict: None,
        });
        assert_eq!(macos_keyboard_restart_coordinates(&status), None);
    }
}
