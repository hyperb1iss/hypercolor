use leptos::prelude::*;

use hypercolor_types::config::HypercolorConfig;

use crate::components::settings_controls::*;
use crate::icons::*;
use crate::tauri_bridge;
use crate::toasts;

use super::read_config;

// ── Developer ──────────────────────────────────────────────────────────────

#[component]
pub fn DeveloperSection(
    #[prop(into)] config: Signal<Option<HypercolorConfig>>,
    on_change: Callback<(String, serde_json::Value)>,
    on_reset: Callback<String>,
) -> impl IntoView {
    let log_level = Signal::derive(move || {
        read_config(config, |cfg| {
            format!("{:?}", cfg.daemon.log_level).to_lowercase()
        })
    });
    let extra_dirs = Signal::derive(move || {
        read_config(config, |cfg| {
            cfg.effect_engine
                .extra_effect_dirs
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
        })
    });

    let log_options = vec![
        ("trace".to_string(), "Trace".to_string()),
        ("debug".to_string(), "Debug".to_string()),
        ("info".to_string(), "Info".to_string()),
        ("warn".to_string(), "Warn".to_string()),
        ("error".to_string(), "Error".to_string()),
    ];

    view! {
        <section id="section-developer" class="pt-5 pb-3 space-y-0">
            <SectionHeader title="Developer" icon=LuCode />
            <div class="text-xs text-fg-tertiary/50 -mt-2 mb-4">"Advanced options for development and debugging"</div>
            <SettingDropdown
                label="Log Level"
                description="Daemon logging verbosity"
                key="daemon.log_level"
                value=log_level
                options=Signal::stored(log_options)
                on_change=on_change
            />
            <SettingPathList
                label="Extra Effect Directories"
                description="Additional directories to scan for custom effects"
                key="effect_engine.extra_effect_dirs"
                paths=extra_dirs
                on_change=on_change
            />
            <ShowWelcomeAgainRow />
            <SectionReset section_label="Developer" on_reset=Callback::new(move |()| {
                for key in &[
                    "daemon.log_level",
                    "effect_engine.extra_effect_dirs",
                ] {
                    on_reset.run(key.to_string());
                }
            }) />
        </section>
    }
}

/// Developer-row affordance to re-trigger the first-run welcome
/// overlay on the next launch. Useful for testing the wizard without
/// reaching into LOCALAPPDATA manually. Hidden in the browser-only UI
/// mode since there's no native bridge to clear the marker through.
#[component]
fn ShowWelcomeAgainRow() -> impl IntoView {
    let native_available = tauri_bridge::is_tauri_available();
    let (resetting, set_resetting) = signal(false);
    let (done, set_done) = signal(false);

    let trigger = move |_| {
        if resetting.get_untracked() {
            return;
        }
        set_resetting.set(true);
        leptos::task::spawn_local(async move {
            let result = tauri_bridge::reset_first_run().await;
            set_resetting.set(false);
            match result {
                Ok(()) => {
                    set_done.set(true);
                    toasts::toast_success("Welcome overlay will show on next launch");
                }
                Err(error) => {
                    toasts::toast_error(&format!("Couldn't reset welcome: {error}"));
                }
            }
        });
    };

    view! {
        <Show when=move || native_available>
            <div class="flex items-start justify-between gap-4 py-3 setting-row">
                <div class="flex-1 min-w-0">
                    <div class="flex items-center gap-2">
                        <span class="text-sm text-fg-primary font-medium">"Show welcome again"</span>
                        <span
                            class="text-[9px] font-mono px-1.5 py-0.5 rounded"
                            style="color: rgba(241, 250, 140, 0.7); background: rgba(241, 250, 140, 0.08)"
                        >
                            "app"
                        </span>
                    </div>
                    <div class="text-xs text-fg-tertiary/70 mt-0.5">
                        {move || if done.get() {
                            "Reset queued. Restart the app to see the welcome overlay."
                        } else {
                            "Re-arm the first-run overlay so it shows on next launch."
                        }}
                    </div>
                </div>
                <button
                    type="button"
                    class="px-3 py-1.5 rounded-lg text-xs font-medium transition-all shrink-0 disabled:cursor-not-allowed"
                    style=move || if resetting.get() || done.get() {
                        "color: rgba(139, 133, 160, 0.55); background: rgba(139, 133, 160, 0.08); border: 1px solid rgba(139, 133, 160, 0.12)"
                    } else {
                        "color: rgba(241, 250, 140, 0.85); background: rgba(241, 250, 140, 0.07); border: 1px solid rgba(241, 250, 140, 0.18)"
                    }
                    disabled=move || resetting.get() || done.get()
                    on:click=trigger
                >
                    {move || if done.get() {
                        "Reset queued"
                    } else if resetting.get() {
                        "Resetting"
                    } else {
                        "Re-arm"
                    }}
                </button>
            </div>
        </Show>
    }
}
