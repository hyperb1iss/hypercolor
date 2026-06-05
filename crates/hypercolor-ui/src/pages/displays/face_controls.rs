use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_icons::Icon;
use leptos_use::{use_debounce_fn, use_debounce_fn_with_arg};

use crate::api;
use crate::app::EffectsContext;
use crate::components::control_panel::ControlPanel;
use crate::icons::*;
use crate::optimistic_controls::{OptimisticControlSession, raw_control_updates_payload};
use crate::preferences::{EffectPreferences, PreferencesStore};
use crate::toasts;

use super::snapshot_scene_lock_message;

/// Live face controls panel.
///
/// Renders the assigned face's `ControlPanel` with an optimistic-update
/// model: local control values tick immediately on input change, and
/// PATCH requests are debounced before hitting the daemon. The server
/// response reconciles the optimistic state so normalized or rejected
/// values surface in the UI.
#[component]
pub(super) fn FaceControlsSection(
    selected_display: Memo<Option<api::DisplaySummary>>,
    display_face: ReadSignal<Option<Result<Option<api::DisplayFaceResponse>, String>>>,
    set_display_face: WriteSignal<Option<Result<Option<api::DisplayFaceResponse>, String>>>,
    set_face_refresh_tick: WriteSignal<u64>,
) -> impl IntoView {
    let effects_ctx = use_context::<EffectsContext>();
    let prefs_store = use_context::<PreferencesStore>();
    // Track the last seen face ID so preferences are re-checked exactly
    // once per face assignment. A naive "already checked" set would skip
    // the restore after switching away and back to the same face ID.
    let last_restored_face_id: StoredValue<Option<String>> = StoredValue::new(None);
    let face_controls = Signal::derive(move || match display_face.get() {
        Some(Ok(Some(face))) => face.effect.controls,
        _ => Vec::new(),
    });
    let has_controls = Signal::derive(move || !face_controls.get().is_empty());
    let control_count = Signal::derive(move || face_controls.get().len());
    // Bundled presets shipped by the face author. User-saved presets are
    // a future enhancement; the Face SDK currently exposes bundled only.
    let face_presets = Signal::derive(move || match display_face.get() {
        Some(Ok(Some(face))) => face.effect.presets,
        _ => Vec::new(),
    });
    let has_presets = Signal::derive(move || !face_presets.get().is_empty());

    // Local optimistic copy of control values. ControlPanel needs a
    // `HashMap<String, ControlValue>` signal; feeding it directly from
    // the face response would rebuild the whole map on every PATCH
    // round-trip instead of ticking immediately on local input.
    let (face_control_values, set_face_control_values) = signal(std::collections::HashMap::<
        String,
        hypercolor_types::effect::ControlValue,
    >::new());

    // Keep the local signal in sync when the face changes. Compare before
    // setting so our own PATCH response does not re-fire downstream
    // effects when the daemon returns identical values.
    Effect::new(move |_| {
        let next = match display_face.get() {
            Some(Ok(Some(face))) => face.group.controls,
            _ => std::collections::HashMap::new(),
        };
        set_face_control_values.update(|current| {
            if *current != next {
                *current = next;
            }
        });
    });

    // Per-face preferences: on first load of a given face ID, compare the
    // server-loaded controls against any stored preferences. If they
    // differ, PATCH the stored values back onto the daemon. On later
    // snapshots for the same face, save the daemon state so switching
    // away and back lands on the same tweaks.
    //
    // `last_restored_face_id` tracks the most recently-restored face so
    // switching A -> B -> A gets a fresh restore opportunity instead of
    // clobbering the stored A preferences with the daemon's fresh state.
    Effect::new(move |_| {
        let Some(Ok(Some(face))) = display_face.get() else {
            last_restored_face_id.set_value(None);
            return;
        };
        let Some(store) = prefs_store else {
            return;
        };
        let face_id = face.effect.id.clone();
        let daemon_controls = face.group.controls.clone();
        let is_fresh_assignment =
            last_restored_face_id.with_value(|last| last.as_deref() != Some(face_id.as_str()));
        if is_fresh_assignment {
            last_restored_face_id.set_value(Some(face_id.clone()));
            if let Some(prefs) = store.get(&face_id)
                && !prefs.control_values.is_empty()
                && prefs.control_values != daemon_controls
                && let Some(display) = selected_display.get_untracked()
            {
                let display_id = display.id;
                let stored_values = prefs.control_values.clone();
                set_face_control_values.set(stored_values.clone());
                spawn_local(async move {
                    let controls_json =
                        crate::components::preset_matching::bundled_preset_to_json(&stored_values);
                    match api::update_display_face_controls(&display_id, &controls_json).await {
                        Ok(face) => {
                            set_display_face.set(Some(Ok(Some(face))));
                        }
                        Err(error) => {
                            toasts::toast_error(&format!(
                                "Face preferences restore failed: {error}"
                            ));
                        }
                    }
                });
                return;
            }
        }
        // Only persist once the daemon has overrides to save. Saving an
        // empty map right after a swap would clobber real preferences
        // stored from earlier sessions on the same face.
        if !daemon_controls.is_empty() {
            store.save(
                face_id,
                EffectPreferences {
                    preset_id: None,
                    control_values: daemon_controls,
                },
            );
        }
    });

    // Pending updates are keyed by control name. Each input overwrites
    // the prior pending value for that control, so a slider drag only
    // sends the final position when the debounce fires.
    let control_session = OptimisticControlSession::new();
    let show_locked_toast = use_debounce_fn_with_arg(
        move |message: String| {
            toasts::toast_error(&message);
        },
        150.0,
    );

    let flush_updates = use_debounce_fn(
        move || {
            if let Some(message) =
                snapshot_scene_lock_message(effects_ctx, "changing display face controls")
            {
                set_face_control_values.set(match display_face.get_untracked() {
                    Some(Ok(Some(face))) => face.group.controls,
                    _ => std::collections::HashMap::new(),
                });
                control_session.clear_pending();
                toasts::toast_error(&message);
                return;
            }
            let Some(display) = selected_display.get_untracked() else {
                return;
            };
            let updates = control_session.take_pending();
            if updates.is_empty() {
                return;
            }
            let controls_json = raw_control_updates_payload(updates);
            let display_id = display.id;
            spawn_local(async move {
                match api::update_display_face_controls(&display_id, &controls_json).await {
                    Ok(face) => {
                        set_display_face.set(Some(Ok(Some(face))));
                        set_face_refresh_tick.update(|value| *value = value.wrapping_add(1));
                    }
                    Err(error) => {
                        toasts::toast_error(&format!("Face control update failed: {error}"));
                    }
                }
            });
        },
        75.0,
    );

    let on_control_change = Callback::new(move |(name, value): (String, serde_json::Value)| {
        if let Some(message) =
            snapshot_scene_lock_message(effects_ctx, "changing display face controls")
        {
            show_locked_toast(message);
            return;
        }
        let controls_snapshot = face_controls.get();
        control_session.apply_raw_update_to(
            set_face_control_values,
            &controls_snapshot,
            &name,
            &value,
        );
        control_session.queue_raw_update(name, value);
        flush_updates();
    });

    let apply_preset = Callback::new(
        move |preset_controls: std::collections::HashMap<
            String,
            hypercolor_types::effect::ControlValue,
        >| {
            let Some(display) = selected_display.get_untracked() else {
                return;
            };
            if let Some(message) =
                snapshot_scene_lock_message(effects_ctx, "applying display face presets")
            {
                toasts::toast_error(&message);
                return;
            }
            control_session.clear_pending();

            let previous_values = face_control_values.get_untracked();

            control_session.apply_values_to(set_face_control_values, &preset_controls);

            let controls_json =
                crate::components::preset_matching::bundled_preset_to_json(&preset_controls);
            let display_id = display.id;
            spawn_local(async move {
                match api::update_display_face_controls(&display_id, &controls_json).await {
                    Ok(face) => {
                        set_display_face.set(Some(Ok(Some(face))));
                        set_face_refresh_tick.update(|value| *value = value.wrapping_add(1));
                    }
                    Err(error) => {
                        set_face_control_values.set(previous_values);
                        toasts::toast_error(&format!("Preset apply failed: {error}"));
                    }
                }
            });
        },
    );

    view! {
        <Show when=move || has_controls.get() fallback=|| ()>
            <div class="rounded-xl border border-t-2 border-edge-subtle border-t-coral/20 bg-surface-raised/80 p-3 edge-glow">
                <div class="mb-3 flex items-center gap-2 border-b border-edge-subtle/50 pb-2">
                    <div class="flex h-6 w-6 items-center justify-center rounded-md bg-coral/10 text-coral/70">
                        <Icon icon=LuSettings2 width="13" height="13" />
                    </div>
                    <h3 class="text-[11px] font-semibold uppercase tracking-wide text-fg-secondary">
                        "Controls"
                    </h3>
                    <span class="text-[10px] text-fg-tertiary">
                        {move || format!("· {}", control_count.get())}
                    </span>
                </div>
                <Show when=move || has_presets.get() fallback=|| ()>
                    <FacePresetBar
                        presets=face_presets
                        control_values=Signal::from(face_control_values)
                        on_apply=apply_preset
                    />
                </Show>
                <ControlPanel
                    controls=face_controls
                    control_values=Signal::from(face_control_values)
                    accent_rgb=Signal::derive(|| "255, 106, 193".to_owned())
                    on_change=on_control_change
                />
            </div>
        </Show>
    }
}

#[component]
fn FacePresetBar(
    presets: Signal<Vec<hypercolor_types::effect::PresetTemplate>>,
    control_values: Signal<
        std::collections::HashMap<String, hypercolor_types::effect::ControlValue>,
    >,
    on_apply: Callback<std::collections::HashMap<String, hypercolor_types::effect::ControlValue>>,
) -> impl IntoView {
    view! {
        <div class="mb-3 flex flex-wrap items-center gap-1.5">
            <span class="text-[10px] uppercase tracking-wider text-fg-tertiary">
                "Presets"
            </span>
            {move || {
                let current = control_values.get();
                presets
                    .get()
                    .into_iter()
                    .map(|preset| {
                        let is_active =
                            crate::components::preset_matching::bundled_preset_matches_controls(
                                &current,
                                &preset.controls,
                            );
                        let preset_controls = preset.controls.clone();
                        let name = preset.name;
                        let pill_class = if is_active {
                            "inline-flex items-center rounded-md border border-coral/50 bg-coral/12 px-2.5 py-1 text-[10px] font-medium text-coral transition"
                        } else {
                            "inline-flex items-center rounded-md border border-edge-subtle bg-surface-overlay/50 px-2.5 py-1 text-[10px] text-fg-secondary transition hover:border-coral/40 hover:text-fg-primary"
                        };
                        view! {
                            <button
                                type="button"
                                class=pill_class
                                aria-pressed=if is_active { "true" } else { "false" }
                                on:click=move |_| on_apply.run(preset_controls.clone())
                            >
                                {name}
                            </button>
                        }
                    })
                    .collect_view()
            }}
        </div>
    }
}
