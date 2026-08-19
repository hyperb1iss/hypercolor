use hypercolor_types::config::{HypercolorConfig, InteractionRoutePolicy};
use leptos::prelude::*;

use super::{MacosCaptureOwnerRestartAction, read_config, validate_macos_restart_owner};
use crate::api::{self, InputStatus, SourceDiagnosticsDisplayField};
use crate::app::WsContext;
use crate::components::settings_controls::{
    AdvancedDisclosure, SectionHeader, SectionReset, SettingDropdown, SettingToggle,
};
use crate::icons::LuKeyboard;
use crate::input_access::{StatusLineTone, input_status_epoch, input_status_line};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct MacosSystemSettingsRemedy {
    label: &'static str,
    pane: crate::tauri_bridge::MacosSystemSettingsPane,
}

pub(super) const fn macos_system_settings_remedy(
    pane: crate::tauri_bridge::MacosSystemSettingsPane,
) -> MacosSystemSettingsRemedy {
    match pane {
        crate::tauri_bridge::MacosSystemSettingsPane::InputMonitoring => {
            MacosSystemSettingsRemedy {
                label: "Open Input Monitoring",
                pane,
            }
        }
        crate::tauri_bridge::MacosSystemSettingsPane::ScreenRecording => {
            MacosSystemSettingsRemedy {
                label: "Open Screen Recording",
                pane,
            }
        }
    }
}

#[component]
pub(super) fn MacosSystemSettingsButton(
    pane: crate::tauri_bridge::MacosSystemSettingsPane,
) -> impl IntoView {
    let remedy = macos_system_settings_remedy(pane);
    let native_available = crate::tauri_bridge::is_tauri_available();
    let open_settings = move |_| {
        leptos::task::spawn_local(async move {
            match crate::tauri_bridge::open_macos_system_settings(remedy.pane).await {
                Ok(true) => {}
                Ok(false) => {
                    leptos::logging::warn!("macOS System Settings opener is unavailable");
                }
                Err(error) => {
                    leptos::logging::warn!("macOS System Settings opener failed: {error}");
                }
            }
        });
    };

    view! {
        <button
            type="button"
            class="inline-flex shrink-0 items-center rounded-md border border-edge-subtle bg-surface-overlay/60 px-2.5 py-1.5 text-xs text-fg-secondary hover:border-accent-muted hover:text-accent disabled:opacity-50"
            disabled=move || !native_available
            on:click=open_settings
        >
            {remedy.label}
        </button>
    }
}

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
                {move || {
                    let status = input_status.get().and_then(Result::ok)?;
                    source_diagnostics_view(source_diagnostics_fields(
                        &status.input,
                        "interaction",
                    ))
                }}
            </AdvancedDisclosure>

            <Show when=move || {
                input_status
                    .get()
                    .and_then(Result::ok)
                    .is_some_and(|status| keyboard_needs_authorization(&status.input))
            }>
                <div class="flex items-center justify-between gap-4 border-b border-edge-subtle/40 py-3">
                    <div class="min-w-0">
                        <div class="text-[13px] text-fg-primary">"Input Monitoring"</div>
                        <div class="text-xs text-fg-tertiary">
                            "Open Input Monitoring in System Settings, enable Hypercolor, then return here."
                        </div>
                    </div>
                    <div class="flex shrink-0 items-center gap-2">
                        <MacosSystemSettingsButton
                            pane=crate::tauri_bridge::MacosSystemSettingsPane::InputMonitoring
                        />
                        <button
                            type="button"
                            class="glow-ring inline-flex shrink-0 items-center rounded-md border border-accent-muted bg-accent-subtle px-2.5 py-1.5 text-xs text-accent hover:bg-accent-muted/20 disabled:opacity-50"
                            disabled=move || authorizing.get()
                            on:click=authorize_keyboard
                        >
                            {move || if authorizing.get() { "Requesting…" } else { "Authorize" }}
                        </button>
                    </div>
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

            {move || {
                let status = input_status.get().and_then(Result::ok)?;
                if keyboard_needs_authorization(&status.input) {
                    return None;
                }
                input_status_line(&status.input)
                    .map(|(tone, text)| status_line_view(tone, text))
            }}
            <SectionReset
                section_label="Input"
                on_reset=Callback::new(move |()| on_reset.run("input".to_owned()))
            />
        </section>
    }
}

pub(super) fn status_line_view(tone: StatusLineTone, text: String) -> impl IntoView {
    let (dot_class, text_class) = match tone {
        StatusLineTone::Active => ("bg-status-success", "text-fg-secondary"),
        StatusLineTone::Ready => ("bg-fg-tertiary/50", "text-fg-tertiary"),
        StatusLineTone::Warn => ("bg-status-warning", "text-status-warning"),
    };
    view! {
        <div class="mt-3 flex items-center gap-2 py-1 text-xs">
            <span class=format!("h-1.5 w-1.5 shrink-0 rounded-full {dot_class}") />
            <span class=text_class>{text}</span>
        </div>
    }
}

const AUTHORIZATION_ACTION_CODES: [&str; 3] = [
    "authorization_required",
    "authorization_denied",
    "authorization_revoked",
];

pub(super) fn source_needs_action(
    status: &InputStatus,
    source_kind: &str,
    action_codes: &[&str],
) -> bool {
    status.sources.iter().any(|source| {
        !source.retired
            && source.kind == source_kind
            && source
                .action_issue
                .as_ref()
                .is_some_and(|issue| action_codes.contains(&issue.code.as_str()))
    })
}

pub(super) fn keyboard_needs_authorization(status: &InputStatus) -> bool {
    source_needs_action(status, "interaction", &AUTHORIZATION_ACTION_CODES)
}

pub(super) fn source_diagnostics_fields(
    status: &InputStatus,
    source_kind: &str,
) -> Vec<SourceDiagnosticsDisplayField> {
    status
        .sources
        .iter()
        .filter(|source| !source.retired && source.kind == source_kind)
        .filter_map(|source| source.diagnostics.as_ref())
        .flat_map(|diagnostics| diagnostics.display().iter().cloned())
        .collect()
}

pub(super) fn source_diagnostics_view(
    fields: Vec<SourceDiagnosticsDisplayField>,
) -> Option<impl IntoView> {
    (!fields.is_empty()).then(|| {
        view! {
            <div class="mt-3 border-t border-edge-subtle/40 pt-3">
                <div class="text-sm font-medium text-fg-primary">"Source diagnostics"</div>
                <div class="mt-1">
                    {fields
                        .into_iter()
                        .map(|field| view! {
                            <div class="flex items-start justify-between gap-4 py-1.5 text-xs">
                                <span class="text-fg-tertiary">{field.label}</span>
                                <span class="text-right text-fg-secondary">{field.value}</span>
                            </div>
                        })
                        .collect_view()}
                </div>
            </div>
        }
    })
}

fn macos_keyboard_restart_coordinates(status: &crate::api::SystemStatus) -> Option<(String, u64)> {
    let needs_restart =
        source_needs_action(&status.input, "interaction", &["process_restart_required"]);
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

fn route_value(route: InteractionRoutePolicy) -> String {
    match route {
        InteractionRoutePolicy::Host => "host",
        InteractionRoutePolicy::Browser => "browser",
        InteractionRoutePolicy::Merge => "merge",
    }
    .to_owned()
}

#[cfg(test)]
mod tests {
    use crate::api::{
        InputSourceIssueStatus, InputSourceStatus, InputStatus, MacosDaemonOwnershipStatus,
        SourceDiagnosticsDisplayField, SourceDiagnosticsEnvelope, SystemStatus,
    };
    use serde_json::json;

    use super::{
        keyboard_needs_authorization, macos_keyboard_restart_coordinates,
        macos_system_settings_remedy, source_diagnostics_fields,
    };

    fn action_issue(code: &str) -> InputSourceIssueStatus {
        InputSourceIssueStatus {
            code: code.to_owned(),
            message: "Action required".to_owned(),
            remediation: None,
            retryable: true,
        }
    }

    fn diagnostics(
        schema: &str,
        fields: &[(&str, &str, &str)],
        payload: serde_json::Value,
    ) -> SourceDiagnosticsEnvelope {
        SourceDiagnosticsEnvelope::try_new(
            schema,
            1,
            fields
                .iter()
                .map(|(key, label, value)| SourceDiagnosticsDisplayField::new(*key, *label, *value))
                .collect(),
            payload,
        )
        .expect("fixture diagnostics should be bounded")
    }

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
    fn keyboard_authorization_action_uses_only_neutral_kind_and_action_code() {
        let mut status = InputStatus {
            sources: vec![InputSourceStatus {
                kind: "interaction".to_owned(),
                action_issue: Some(action_issue("authorization_required")),
                ..InputSourceStatus::default()
            }],
            ..InputStatus::default()
        };
        assert!(keyboard_needs_authorization(&status));

        status.sources[0].retired = true;
        assert!(!keyboard_needs_authorization(&status));
        status.sources[0].retired = false;

        status.sources[0].kind = "screen".to_owned();
        assert!(!keyboard_needs_authorization(&status));
        status.sources[0].kind = "interaction".to_owned();

        for code in ["authorization_denied", "authorization_revoked"] {
            status.sources[0].action_issue = Some(action_issue(code));
            assert!(keyboard_needs_authorization(&status));
        }

        status.sources[0].action_issue = None;
        status.sources[0].diagnostics = Some(diagnostics(
            "fixture.input",
            &[("permission", "Permission", "Denied")],
            json!({"authorization": "denied"}),
        ));
        assert!(!keyboard_needs_authorization(&status));
    }

    #[test]
    fn screen_authorization_action_tracks_only_screen_recording_state() {
        let mut status = InputStatus {
            sources: vec![InputSourceStatus {
                kind: "screen".to_owned(),
                action_issue: Some(action_issue("authorization_denied")),
                ..InputSourceStatus::default()
            }],
            ..InputStatus::default()
        };
        assert!(super::super::macos_screen_needs_authorization(&status));

        status.sources[0].retired = true;
        assert!(!super::super::macos_screen_needs_authorization(&status));
        status.sources[0].retired = false;

        status.sources[0].action_issue = None;
        assert!(!super::super::macos_screen_needs_authorization(&status));
    }

    #[test]
    fn diagnostics_keep_source_and_display_order_while_filtering_retired_sources() {
        let status = InputStatus {
            sources: vec![
                InputSourceStatus {
                    kind: "interaction".to_owned(),
                    diagnostics: Some(diagnostics(
                        "fixture.first",
                        &[("first", "First", "1"), ("second", "Second", "2")],
                        json!({"ignored": true}),
                    )),
                    ..InputSourceStatus::default()
                },
                InputSourceStatus {
                    kind: "screen".to_owned(),
                    diagnostics: Some(diagnostics(
                        "fixture.screen",
                        &[("screen", "Screen", "excluded")],
                        json!({}),
                    )),
                    ..InputSourceStatus::default()
                },
                InputSourceStatus {
                    kind: "interaction".to_owned(),
                    diagnostics: Some(diagnostics(
                        "fixture.retired",
                        &[("retired", "Retired", "excluded")],
                        json!({}),
                    )),
                    retired: true,
                    ..InputSourceStatus::default()
                },
                InputSourceStatus {
                    kind: "interaction".to_owned(),
                    diagnostics: Some(diagnostics(
                        "fixture.last",
                        &[("third", "Third", "3")],
                        json!({}),
                    )),
                    ..InputSourceStatus::default()
                },
            ],
            ..InputStatus::default()
        };

        let fields = source_diagnostics_fields(&status, "interaction");
        assert_eq!(
            fields
                .iter()
                .map(|field| (
                    field.key.as_str(),
                    field.label.as_str(),
                    field.value.as_str()
                ))
                .collect::<Vec<_>>(),
            vec![
                ("first", "First", "1"),
                ("second", "Second", "2"),
                ("third", "Third", "3"),
            ]
        );
    }

    #[test]
    fn macos_permission_remedies_keep_exact_labels_and_deep_links() {
        let input = macos_system_settings_remedy(
            crate::tauri_bridge::MacosSystemSettingsPane::InputMonitoring,
        );
        assert_eq!(input.label, "Open Input Monitoring");
        assert_eq!(
            input.pane,
            crate::tauri_bridge::MacosSystemSettingsPane::InputMonitoring
        );

        let screen = macos_system_settings_remedy(
            crate::tauri_bridge::MacosSystemSettingsPane::ScreenRecording,
        );
        assert_eq!(screen.label, "Open Screen Recording");
        assert_eq!(
            screen.pane,
            crate::tauri_bridge::MacosSystemSettingsPane::ScreenRecording
        );
    }

    #[test]
    fn restart_coordinates_require_exact_state_owner_and_epoch() {
        let mut status = system_status(
            InputStatus {
                sources: vec![InputSourceStatus {
                    kind: "interaction".to_owned(),
                    action_issue: Some(action_issue("process_restart_required")),
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
        status.input.sources[0].action_issue = Some(action_issue("authorization_denied"));
        assert_eq!(macos_keyboard_restart_coordinates(&status), None);
    }
}
