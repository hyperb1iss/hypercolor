//! Small release notice and Settings controls for the optional OSS checker.

use leptos::prelude::*;
use leptos_icons::Icon;

use crate::api::updates::{ReleaseUpdate, UpdatePreferencesPatch};
use crate::components::settings_controls::SettingToggle;
use crate::icons::{LuDownload, LuX};
use crate::release_notifications::{ReleaseNotificationsContext, should_notify};

const ACTION_CLASS: &str = "rounded-lg border border-edge-subtle px-3 py-1.5 text-xs \
    text-fg-secondary hover:bg-surface-hover hover:text-fg-primary transition-colors \
    disabled:opacity-50 disabled:cursor-wait";

#[component]
pub fn ReleaseNotice() -> impl IntoView {
    let context = expect_context::<ReleaseNotificationsContext>();
    view! {
        <Show when=move || context.enabled.get()>
            {move || {
                context.status.get().flatten().and_then(Result::ok)
                    .filter(should_notify)
                    .and_then(|status| status.release)
                    .map(|release| {
                        let version = release.version.clone();
                        view! {
                            <aside
                                class="flex shrink-0 flex-wrap items-center gap-3 border-b border-edge-subtle bg-surface-raised px-4 py-3"
                                aria-label="Hypercolor update"
                            >
                                <span class="text-accent">
                                    <Icon icon=LuDownload width="16px" height="16px" />
                                </span>
                                <p class="flex-1 text-sm text-fg-primary">
                                    {format!("Hypercolor {} is available", release.version)}
                                </p>
                                <ReleaseLinks release=release />
                                <button
                                    class=ACTION_CLASS
                                    title="Skip this version"
                                    aria-label="Dismiss update notice and skip this version"
                                    disabled=move || context.pending.get()
                                    on:click=move |_| context.preferences.run(UpdatePreferencesPatch {
                                        skipped_version: Some(version.clone()),
                                        ..Default::default()
                                    })
                                >
                                    <Icon icon=LuX width="14px" height="14px" />
                                </button>
                            </aside>
                        }
                    })
            }}
        </Show>
    }
}

#[component]
pub fn ReleaseSettings() -> impl IntoView {
    let context = expect_context::<ReleaseNotificationsContext>();
    view! {
        <Show when=move || context.enabled.get()>
            <div class="mt-4 border-t border-edge-subtle pt-4" aria-label="Release notifications">
                <div class="flex flex-wrap items-center justify-between gap-3">
                    <h3 class="text-sm font-medium text-fg-primary">"Updates"</h3>
                    <button
                        class=ACTION_CLASS
                        disabled=move || context.pending.get()
                        on:click=move |_| context.check.run(())
                    >
                        {move || if context.pending.get() { "Working..." } else { "Check for updates" }}
                    </button>
                </div>
                <p class="mt-2 text-xs text-fg-tertiary">
                    "Updates are for the computer running Hypercolor. If you installed with a package manager, update through it."
                </p>
                {move || context.error.get().map(|error| view! {
                    <p class="mt-3 text-xs text-status-error" role="alert">{error}</p>
                })}
                {move || {
                    match context.status.get().flatten() {
                        Some(Ok(status)) => {
                            let automatic_checks = status.automatic_checks;
                            let skipped = status.skipped_version;
                            view! {
                                <fieldset disabled=move || context.pending.get()>
                                    <SettingToggle
                                        label="Automatic checks"
                                        description="Check GitHub daily for a new stable release."
                                        key="release-notifications.automatic_checks"
                                        value=Signal::derive(move || automatic_checks)
                                        on_change=Callback::new(move |(_, value): (String, serde_json::Value)| {
                                            context.preferences.run(UpdatePreferencesPatch {
                                                automatic_checks: value.as_bool(),
                                                ..Default::default()
                                            });
                                        })
                                    />
                                </fieldset>
                                {status.release.map(|release| {
                                    let version = release.version.clone();
                                    let is_skipped = skipped.as_deref() == Some(version.as_str());
                                    let notes = release.notes.clone();
                                    view! {
                                        <div class="space-y-3 rounded-lg border border-edge-subtle bg-surface-raised p-3">
                                            <p class="text-sm text-fg-primary">
                                                {format!("Hypercolor {} is available", release.version)}
                                            </p>
                                            <div class="flex flex-wrap items-center gap-2">
                                                <ReleaseLinks release=release />
                                                <button
                                                    class=ACTION_CLASS
                                                    disabled=move || context.pending.get()
                                                    on:click=move |_| context.preferences.run(UpdatePreferencesPatch {
                                                        skipped_version: (!is_skipped).then(|| version.clone()),
                                                        clear_skipped_version: is_skipped,
                                                        ..Default::default()
                                                    })
                                                >
                                                    {if is_skipped { "Show notification again" } else { "Skip this version" }}
                                                </button>
                                            </div>
                                            {(!notes.trim().is_empty()).then(|| view! {
                                                <details class="text-xs text-fg-secondary">
                                                    <summary class="cursor-pointer hover:text-accent">"Release notes"</summary>
                                                    <p class="mt-2 max-h-64 overflow-y-auto whitespace-pre-wrap break-words leading-relaxed">{notes}</p>
                                                </details>
                                            })}
                                        </div>
                                    }
                                })}
                                {status.last_checked.map(|checked| view! {
                                    <p class="mt-3 text-xs text-fg-tertiary">{format!("Last checked: {checked}")}</p>
                                })}
                            }.into_any()
                        }
                        Some(Err(error)) => view! {
                            <p class="mt-3 text-xs text-status-error" role="alert">
                                {format!("Could not load release notifications: {error}")}
                            </p>
                        }.into_any(),
                        None => view! {
                            <p class="mt-3 text-xs text-fg-tertiary">"Loading release notifications..."</p>
                        }.into_any(),
                    }
                }}
            </div>
        </Show>
    }
}

#[component]
fn ReleaseLinks(release: ReleaseUpdate) -> impl IntoView {
    let download = release.download_url;
    view! {
        {download.map(|url| view! {
            <a href=url target="_blank" rel="noopener noreferrer" class=ACTION_CLASS>
                "Download"
            </a>
        })}
        <a href=release.release_url target="_blank" rel="noopener noreferrer" class=ACTION_CLASS>
            "View release on GitHub"
        </a>
    }
}
