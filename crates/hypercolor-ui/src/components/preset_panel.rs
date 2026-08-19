//! Preset toolbar — compact single-line preset selector with save/create/edit/delete.

use leptos::ev;
use leptos::prelude::*;
use leptos_use::{UseEventListenerOptions, use_event_listener_with_options};
use std::collections::HashMap;

use crate::api;
use crate::control_value_json::controls_to_json;
use crate::toasts;
use hypercolor_color::Hsl;
use hypercolor_leptos_ext::events::{document as browser_document, target_closest};
use hypercolor_types::effect::ControlValue;

mod actions;

use actions::{InlineNameInput, PresetActionButtons};

// ── Per-preset swatch colouring ──────────────────────────────────────────────

/// Derives a vivid, stable swatch colour from a preset's name so every
/// row in the dropdown is visually distinct. Real control-value extraction
/// was tried and rejected — most presets in an effect share one or two
/// "primary palette" colours, which meant every row in a dropdown landed
/// on the same shared hue. Name hashing gives guaranteed uniqueness across
/// the group while still being deterministic across reloads, so users
/// learn to recognise presets by their colour at a glance.
///
/// Returns an `"r, g, b"` string ready for interpolation into `rgb(...)`
/// / `rgba(...)` CSS (including as the `--item-rgb` custom property on
/// `.preset-option`).
fn preset_swatch(name: &str) -> String {
    // Two independent hashes so hue and saturation/lightness don't move in
    // lockstep — otherwise similar names would produce colour pairs that
    // sit suspiciously close in both hue and brightness.
    let h1 = djb2_hash(name);
    let h2 = djb2_hash_reversed(name);

    let hue = (h1 % 360) as f32;
    // Keep saturation vivid and lightness in the readable "neon" band so
    // every swatch reads cleanly against the dropdown's dark background.
    let saturation = 0.72 + ((h2 % 28) as f32) / 100.0; // 0.72 .. 0.99
    let lightness = 0.58 + (((h2 / 31) % 18) as f32) / 100.0; // 0.58 .. 0.75
    hsl_to_rgb_triplet(hue, saturation, lightness)
}

fn djb2_hash(s: &str) -> u32 {
    let mut h: u32 = 5381;
    for b in s.bytes() {
        h = h.wrapping_mul(33).wrapping_add(u32::from(b));
    }
    h
}

fn djb2_hash_reversed(s: &str) -> u32 {
    let mut h: u32 = 5381;
    for b in s.bytes().rev() {
        h = h.wrapping_mul(33).wrapping_add(u32::from(b));
    }
    h
}

/// Plain HSL → sRGB triplet string — vivid but still within a readable
/// neon band once the saturation/lightness are pre-clamped by caller.
///
/// The kernel clamps saturation and lightness and wraps hue, so a
/// caller that drifts out of range now degrades to the nearest legal
/// color instead of relying on the byte cast to saturate for it.
fn hsl_to_rgb_triplet(h: f32, s: f32, l: f32) -> String {
    let rgb = Hsl::new(h, s, l).to_rgb();
    format!("{}, {}, {}", rgb.r, rgb.g, rgb.b)
}

/// Compact preset toolbar for the effect detail sidebar.
///
/// Single line: `[Preset dropdown] [Save] [+] [···]`
#[component]
pub fn PresetToolbar(
    /// The active effect's ID.
    #[prop(into)]
    effect_id: Signal<Option<String>>,
    /// Current live control values — snapshotted when saving.
    #[prop(into)]
    control_values: Signal<HashMap<String, ControlValue>>,
    /// Category accent color as "r, g, b" string. Drives the dropdown
    /// chrome (trigger border, popover border, group header glyphs) so
    /// the toolbar feels tied to whatever effect is currently rendering.
    #[prop(into)]
    accent_rgb: Signal<String>,
    /// Callback fired after a preset is applied (so parent can refresh controls).
    #[prop(into)]
    on_preset_applied: Callback<()>,
    /// The active preset ID from the engine (restored on effect switch).
    #[prop(into, optional)]
    active_preset_id_signal: Option<Signal<Option<String>>>,
) -> impl IntoView {
    let (presets, set_presets) = signal(Vec::<api::EffectPresetSummary>::new());
    let (selected_id, set_selected_id) = signal(Option::<String>::None);
    let (mode, set_mode) = signal(ToolbarMode::Idle);
    let fetch_generation = StoredValue::new(0_u64);
    let zones_ctx = expect_context::<crate::zones::ZonesContext>();
    let active_preset_modified = Signal::derive(move || {
        let Some(selected_id) = selected_id.get() else {
            return false;
        };
        presets
            .get()
            .iter()
            .find(|preset| preset.id == selected_id)
            .is_some_and(|preset| preset_controls_modified(&preset.controls, &control_values.get()))
    });

    // Fetch the effect-scoped preset stack whenever effect_id actually changes.
    //
    // Leptos signals always notify on `set()` (no PartialEq guard), so the
    // derived `effect_id` signal can re-fire even when the ID is unchanged
    // (e.g., after `refresh_active_effect`). We compare against the previous
    // value and skip the fetch+clear when nothing changed — this prevents
    // option recreation from resetting the <select> element.
    Effect::new(move |prev_eid: Option<Option<String>>| {
        let eid = effect_id.get();
        if prev_eid.as_ref() == Some(&eid) {
            return eid; // Same effect — skip everything
        }

        set_selected_id.set(None);
        let fetch_eid = eid.clone();
        let request_generation = fetch_generation.get_value().saturating_add(1);
        fetch_generation.set_value(request_generation);
        leptos::task::spawn_local(async move {
            let next_presets = if let Some(ref id) = fetch_eid {
                api::fetch_effect_presets(id).await.unwrap_or_default()
            } else {
                Vec::new()
            };

            if fetch_generation.get_value() == request_generation
                && effect_id.get_untracked() == fetch_eid
            {
                set_presets.set(next_presets);
            }
        });
        eid
    });

    // The daemon owns preset provenance for both bundled and saved entries.
    Effect::new(move |_| {
        let next_selected = active_preset_id_signal
            .map(|signal| signal.get())
            .unwrap_or_default();

        if selected_id.get_untracked() != next_selected {
            set_selected_id.set(next_selected);
        }
    });

    let selected_preset = Memo::new(move |_| {
        let sid = selected_id.get()?;
        presets.get().into_iter().find(|preset| preset.id == sid)
    });

    let has_editable_selection =
        Memo::new(move |_| selected_preset.get().is_some_and(|preset| preset.editable));

    // Refresh helper
    let refresh_presets = move || {
        let Some(request_effect_id) = effect_id.get_untracked() else {
            set_presets.set(Vec::new());
            return;
        };
        let request_generation = fetch_generation.get_value().saturating_add(1);
        fetch_generation.set_value(request_generation);
        leptos::task::spawn_local(async move {
            if let Ok(next_presets) = api::fetch_effect_presets(&request_effect_id).await
                && fetch_generation.get_value() == request_generation
                && effect_id.get_untracked().as_deref() == Some(request_effect_id.as_str())
            {
                set_presets.set(next_presets);
            }
        });
    };

    // Select preset by value string (replaces the old on_select that took web_sys::Event)
    let on_select_value = move |val: String| {
        if val.is_empty() {
            // "No preset" selected — reset controls to defaults
            let previous_selection = selected_id.get_untracked();
            set_selected_id.set(None);
            let on_applied = on_preset_applied;
            leptos::task::spawn_local(async move {
                match api::reset_controls().await {
                    Ok(()) => on_applied.run(()),
                    Err(error) => {
                        set_selected_id.set(previous_selection);
                        toasts::toast_error(&format!("Failed to reset controls: {error}"));
                    }
                }
            });
            return;
        }

        let previous_selection = selected_id.get_untracked();
        set_selected_id.set(Some(val.clone()));
        set_mode.set(ToolbarMode::Idle);
        let on_applied = on_preset_applied;
        let Some(active_effect_id) = effect_id.get_untracked() else {
            set_selected_id.set(previous_selection);
            return;
        };
        let target_zone = zones_ctx.focused_zone_id_untracked();
        leptos::task::spawn_local(async move {
            match api::apply_effect_preset(&active_effect_id, &val, target_zone.as_deref()).await {
                Ok(()) => {
                    if target_zone.is_some() {
                        zones_ctx.refresh.run(());
                    }
                    on_applied.run(());
                }
                Err(error) => {
                    set_selected_id.set(previous_selection);
                    toasts::toast_error(&format!("Failed to apply preset: {error}"));
                }
            }
        });
    };

    // Save over current preset
    let on_save = move |_: leptos::ev::MouseEvent| {
        let Some(preset) = selected_preset.get() else {
            return;
        };
        if !preset.editable {
            return;
        }
        let eid = effect_id.get().unwrap_or_default();
        let values = control_values.get();
        let controls_json = controls_to_json(&values);
        let name = preset.name.clone();
        let pid = preset.id.clone();
        let refresh = refresh_presets;
        leptos::task::spawn_local(async move {
            let req = api::SavePresetRequest {
                name,
                description: None,
                effect: eid.clone(),
                controls: Some(serde_json::Value::Object(controls_json)),
                tags: None,
            };
            if api::update_preset(&pid, &req).await.is_ok() {
                toasts::toast_success("Preset saved");
                refresh();
            }
        });
    };

    // Create new preset
    let on_create = move |name: String| {
        let eid = effect_id.get().unwrap_or_default();
        let values = control_values.get();
        let controls_json = controls_to_json(&values);
        let refresh = refresh_presets;
        let target_zone = zones_ctx.focused_zone_id_untracked();
        set_mode.set(ToolbarMode::Idle);
        leptos::task::spawn_local(async move {
            let req = api::SavePresetRequest {
                name,
                description: None,
                effect: eid.clone(),
                controls: Some(serde_json::Value::Object(controls_json)),
                tags: None,
            };
            match api::create_preset(&req).await {
                Ok(created) => {
                    let created_id = created.id.to_string();
                    match api::apply_effect_preset(&eid, &created_id, target_zone.as_deref()).await
                    {
                        Ok(()) => {
                            set_selected_id.set(Some(created_id));
                            toasts::toast_success("Preset created");
                            refresh();
                        }
                        Err(error) => {
                            toasts::toast_error(&format!(
                                "Preset created but could not be selected: {error}"
                            ));
                            refresh();
                        }
                    }
                }
                Err(error) => {
                    toasts::toast_error(&format!("Failed to create preset: {error}"));
                }
            }
        });
    };

    // Rename preset
    let on_rename = move |new_name: String| {
        let Some(preset) = selected_preset.get() else {
            return;
        };
        if !preset.editable {
            return;
        }
        let eid = effect_id.get().unwrap_or_default();
        let pid = preset.id.clone();
        let refresh = refresh_presets;
        set_mode.set(ToolbarMode::Idle);
        leptos::task::spawn_local(async move {
            let req = api::SavePresetRequest {
                name: new_name,
                description: None,
                effect: eid,
                controls: Some(serde_json::Value::Object(controls_to_json(
                    &preset.controls,
                ))),
                tags: None,
            };
            if api::update_preset(&pid, &req).await.is_ok() {
                toasts::toast_success("Preset renamed");
                refresh();
            }
        });
    };

    // Delete preset
    let on_delete = move |_: leptos::ev::MouseEvent| {
        let Some(preset) = selected_preset.get() else {
            return;
        };
        if !preset.editable {
            return;
        }
        let previous_selection = selected_id.get_untracked();
        let pid = preset.id.clone();
        let refresh = refresh_presets;
        set_selected_id.set(None);
        set_mode.set(ToolbarMode::Idle);
        leptos::task::spawn_local(async move {
            match api::delete_preset(&pid).await {
                Ok(()) => {
                    toasts::toast_info("Preset deleted");
                    refresh();
                }
                Err(error) => {
                    set_selected_id.set(previous_selection);
                    toasts::toast_error(&format!("Failed to delete preset: {error}"));
                }
            }
        });
    };

    view! {
        <div class="py-0.5">
            {move || {
                match mode.get() {
                    ToolbarMode::Idle => {
                        let on_save = on_save;
                        let on_delete = on_delete;
                        view! {
                            <div class="animate-swap-in">
                                <PresetSelectorRow
                                    presets=presets
                                    selected_id=selected_id
                                    active_preset_modified=active_preset_modified
                                    has_editable_selection=has_editable_selection
                                    accent_rgb=accent_rgb
                                    on_select=Callback::new(on_select_value)
                                    on_save=on_save
                                    on_new=move |_| set_mode.set(ToolbarMode::Creating)
                                    on_edit=move |_| set_mode.set(ToolbarMode::Renaming)
                                    on_delete=on_delete
                                />
                            </div>
                        }.into_any()
                    }
                    ToolbarMode::Creating => {
                        let on_create = on_create;
                        view! {
                            <div class="animate-swap-in">
                                <InlineNameInput
                                    placeholder="New preset name..."
                                    initial=""
                                    on_submit=Callback::new(move |name: String| on_create(name))
                                    on_cancel=Callback::new(move |()| set_mode.set(ToolbarMode::Idle))
                                />
                            </div>
                        }.into_any()
                    }
                    ToolbarMode::Renaming => {
                        let current_name = selected_preset
                            .get()
                            .map(|p| p.name.clone())
                            .unwrap_or_default();
                        let on_rename = on_rename;
                        view! {
                            <div class="animate-swap-in">
                                <InlineNameInput
                                    placeholder="Rename preset..."
                                    initial=current_name
                                    on_submit=Callback::new(move |name: String| on_rename(name))
                                    on_cancel=Callback::new(move |()| set_mode.set(ToolbarMode::Idle))
                                />
                            </div>
                        }.into_any()
                    }
                }
            }}
        </div>
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ToolbarMode {
    Idle,
    Creating,
    Renaming,
}

/// The main selector row: custom dropdown + action buttons.
#[component]
fn PresetSelectorRow(
    presets: ReadSignal<Vec<api::EffectPresetSummary>>,
    selected_id: ReadSignal<Option<String>>,
    active_preset_modified: Signal<bool>,
    has_editable_selection: Memo<bool>,
    accent_rgb: Signal<String>,
    on_select: Callback<String>,
    on_save: impl Fn(leptos::ev::MouseEvent) + 'static,
    on_new: impl Fn(leptos::ev::MouseEvent) + 'static,
    on_edit: impl Fn(leptos::ev::MouseEvent) + 'static,
    on_delete: impl Fn(leptos::ev::MouseEvent) + 'static,
) -> impl IntoView {
    let (is_open, set_is_open) = signal(false);

    // Build the display label for the currently selected item
    let selected_label = Memo::new(move |_| {
        let sid = selected_id.get();
        let Some(ref sid) = sid else {
            return "Default".to_string();
        };

        let name = presets
            .get()
            .iter()
            .find(|p| p.id == *sid)
            .map(|p| p.name.as_str().to_owned());
        preset_display_label(Some(sid), name.as_deref(), active_preset_modified.get())
    });

    // The swatch the trigger shows — pulled from whichever preset is
    // currently selected so the trigger button itself is tinted to match
    // the row that's "active" in the dropdown.
    let selected_swatch = Memo::new(move |_| {
        let sid = selected_id.get()?;
        presets
            .get()
            .iter()
            .find(|p| p.id == sid)
            .map(|p| preset_swatch(&p.name))
    });

    // Has a real (non-Default) preset been picked? Drives the trigger
    // indicator dot and the accent-leaning background gradient.
    let has_selection = Memo::new(move |_| selected_id.get().is_some());

    // Close on Escape
    let on_keydown = move |ev: web_sys::KeyboardEvent| {
        if ev.key() == "Escape" && is_open.get_untracked() {
            set_is_open.set(false);
            ev.prevent_default();
        }
    };

    view! {
        <div class="flex items-center gap-2" on:keydown=on_keydown>
            <Show when=move || is_open.get()>
                <PresetDropdownDismissHandler set_open=set_is_open />
            </Show>
            // Custom dropdown
            <div class="relative flex-1 min-w-0 preset-dropdown">
                // Trigger button — accent-tinted with the selected preset's
                // own swatch falling back to the effect category accent.
                <button
                    type="button"
                    class="w-full flex items-center gap-2 border pl-2.5 pr-2 py-[7px] \
                           text-xs cursor-pointer select-silk-trigger transition-all"
                    class=("rounded-t-lg", move || is_open.get())
                    class=("rounded-b-none", move || is_open.get())
                    class=("rounded-lg", move || !is_open.get())
                    style=move || {
                        let tint = selected_swatch.get().unwrap_or_else(|| accent_rgb.get());
                        let active = has_selection.get() || is_open.get();
                        let border_alpha = if active { 0.42 } else { 0.18 };
                        let glow_alpha = if active { 0.12 } else { 0.05 };
                        format!(
                            "background: linear-gradient(135deg, \
                               rgba({tint}, 0.08) 0%, \
                               rgba(10, 9, 16, 0.72) 60%, \
                               rgba(10, 9, 16, 0.82) 100%); \
                             border-color: rgba({tint}, {border_alpha}); \
                             box-shadow: 0 0 14px rgba({tint}, {glow_alpha}), \
                                         inset 0 1px 0 rgba(255, 255, 255, 0.04)"
                        )
                    }
                    on:click=move |_| set_is_open.update(|v| *v = !*v)
                >
                    // Leading accent dot — pulses when a real preset is active
                    <span
                        class="w-1.5 h-1.5 rounded-full shrink-0 transition-all"
                        class=("animate-pulse", move || has_selection.get())
                        style=move || {
                            let tint = selected_swatch.get().unwrap_or_else(|| accent_rgb.get());
                            let sat = if has_selection.get() { 1.0 } else { 0.4 };
                            format!(
                                "background: rgb({tint}); \
                                 box-shadow: 0 0 8px rgba({tint}, {sat}), \
                                             0 0 2px rgba({tint}, 1); \
                                 opacity: {}",
                                if has_selection.get() { "1" } else { "0.55" }
                            )
                        }
                    />

                    <span
                        class="flex-1 min-w-0 text-left truncate"
                        style=move || {
                            if has_selection.get() {
                                "color: var(--text-primary); font-weight: 500".to_string()
                            } else {
                                "color: var(--text-secondary)".to_string()
                            }
                        }
                    >
                        {move || selected_label.get()}
                    </span>

                    <svg
                        class="w-3 h-3 shrink-0 transition-transform duration-200"
                        class=("rotate-180", move || is_open.get())
                        viewBox="0 0 24 24"
                        fill="none"
                        stroke="currentColor"
                        stroke-width="2"
                        stroke-linecap="round"
                        stroke-linejoin="round"
                        style=move || {
                            let tint = selected_swatch.get().unwrap_or_else(|| accent_rgb.get());
                            format!("color: rgba({tint}, 0.85)")
                        }
                    >
                        <path d="m6 9 6 6 6-6" />
                    </svg>
                </button>

                // Dropdown popover — glass with category-tinted border.
                <Show when=move || is_open.get()>
                    <div
                        class="absolute left-0 right-0 top-full
                               rounded-b-lg
                               backdrop-blur-xl
                               border border-t-0
                               animate-enter-down
                               max-h-[340px] overflow-y-auto scrollbar-dropdown py-1"
                        style=move || {
                            let tint = accent_rgb.get();
                            format!(
                                "z-index: 9999; \
                                 margin-top: -1px; \
                                 background: linear-gradient(180deg, \
                                   rgba(14, 12, 22, 0.92) 0%, \
                                   rgba(10, 9, 16, 0.94) 100%); \
                                 border-color: rgba({tint}, 0.38); \
                                 box-shadow: 0 12px 40px rgba(0, 0, 0, 0.55), \
                                             0 0 32px rgba({tint}, 0.10), \
                                             inset 0 1px 0 rgba(255, 255, 255, 0.04)"
                            )
                        }
                        on:mousedown=|ev: leptos::ev::MouseEvent| ev.stop_propagation()
                    >
                        // Default preset option — resets controls to effect defaults
                        {
                            let default_swatch = preset_swatch("Default");
                            view! {
                                <DropdownItem
                                    value="".to_string()
                                    label="Default".to_string()
                                    swatch_rgb=default_swatch
                                    is_selected=Signal::derive(move || selected_id.get().is_none())
                                    on_click=Callback::new(move |val: String| {
                                        on_select.run(val);
                                        set_is_open.set(false);
                                    })
                                />
                            }
                        }

                        // Bundled presets group
                        {move || {
                            let all_presets = presets.get();
                            let bp = all_presets
                                .iter()
                                .filter(|preset| preset.origin == api::EffectPresetOrigin::Bundled)
                                .cloned()
                                .collect::<Vec<_>>();
                            let has_user = all_presets
                                .iter()
                                .any(|preset| preset.origin == api::EffectPresetOrigin::Saved);
                            if bp.is_empty() {
                                return {
                                    let _: () = view! { <></> };
                                    ().into_any()
                                };
                            }
                            let tint = accent_rgb.get();
                            view! {
                                <>
                                    {(has_user).then(|| {
                                        let label_style = format!(
                                            "color: rgba({tint}, 0.72); \
                                             text-shadow: 0 0 6px rgba({tint}, 0.3)"
                                        );
                                        view! {
                                            <div class="px-3 pt-2.5 pb-1 flex items-center gap-1.5">
                                                <div
                                                    class="h-px flex-1"
                                                    style=format!(
                                                        "background: linear-gradient(90deg, \
                                                           transparent 0%, \
                                                           rgba({tint}, 0.35) 50%, \
                                                           transparent 100%)"
                                                    )
                                                />
                                                <span
                                                    class="text-[8px] font-mono uppercase tracking-[0.18em]"
                                                    style=label_style
                                                >
                                                    "Built-in"
                                                </span>
                                                <div
                                                    class="h-px flex-1"
                                                    style=format!(
                                                        "background: linear-gradient(90deg, \
                                                           transparent 0%, \
                                                           rgba({tint}, 0.35) 50%, \
                                                           transparent 100%)"
                                                    )
                                                />
                                            </div>
                                        }
                                    })}
                                    {bp.into_iter().map(|p| {
                                        let val = p.id;
                                        let swatch = preset_swatch(&p.name);
                                        let option_value = val.clone();
                                        view! {
                                            <DropdownItem
                                                value=val
                                                label=p.name
                                                description=p.description.unwrap_or_default()
                                                swatch_rgb=swatch
                                                is_selected=Signal::derive(move || selected_id.get().as_deref() == Some(option_value.as_str()))
                                                on_click=Callback::new(move |val: String| {
                                                    on_select.run(val);
                                                    set_is_open.set(false);
                                                })
                                            />
                                        }
                                    }).collect_view()}
                                </>
                            }.into_any()
                        }}

                        // User presets group
                        {move || {
                            let all_presets = presets.get();
                            let user = all_presets
                                .iter()
                                .filter(|preset| preset.origin == api::EffectPresetOrigin::Saved)
                                .cloned()
                                .collect::<Vec<_>>();
                            let has_bundled = all_presets
                                .iter()
                                .any(|preset| preset.origin == api::EffectPresetOrigin::Bundled);
                            if user.is_empty() {
                                return {
                                    let _: () = view! { <></> };
                                    ().into_any()
                                };
                            }
                            let tint = accent_rgb.get();
                            view! {
                                <>
                                    {(has_bundled).then(|| {
                                        let label_style = format!(
                                            "color: rgba({tint}, 0.72); \
                                             text-shadow: 0 0 6px rgba({tint}, 0.3)"
                                        );
                                        view! {
                                            <div class="px-3 pt-2.5 pb-1 flex items-center gap-1.5">
                                                <div
                                                    class="h-px flex-1"
                                                    style=format!(
                                                        "background: linear-gradient(90deg, \
                                                           transparent 0%, \
                                                           rgba({tint}, 0.35) 50%, \
                                                           transparent 100%)"
                                                    )
                                                />
                                                <span
                                                    class="text-[8px] font-mono uppercase tracking-[0.18em]"
                                                    style=label_style
                                                >
                                                    "My Presets"
                                                </span>
                                                <div
                                                    class="h-px flex-1"
                                                    style=format!(
                                                        "background: linear-gradient(90deg, \
                                                           transparent 0%, \
                                                           rgba({tint}, 0.35) 50%, \
                                                           transparent 100%)"
                                                    )
                                                />
                                            </div>
                                        }
                                    })}
                                    {user.into_iter().map(|p| {
                                        let id = p.id.clone();
                                        let swatch = preset_swatch(&p.name);
                                        let option_value = id.clone();
                                        view! {
                                            <DropdownItem
                                                value=id
                                                label=p.name
                                                description=p.description.unwrap_or_default()
                                                swatch_rgb=swatch
                                                is_selected=Signal::derive(move || selected_id.get().as_deref() == Some(option_value.as_str()))
                                                on_click=Callback::new(move |val: String| {
                                                    on_select.run(val);
                                                    set_is_open.set(false);
                                                })
                                            />
                                        }
                                    }).collect_view()}
                                </>
                            }.into_any()
                        }}
                    </div>
                </Show>
            </div>

            // Action buttons
            <PresetActionButtons
                has_selection=has_editable_selection
                on_save=on_save
                on_new=on_new
                on_edit=on_edit
                on_delete=on_delete
            />
        </div>
    }
}

fn preset_display_label(
    selected_id: Option<&str>,
    selected_name: Option<&str>,
    modified: bool,
) -> String {
    let Some(_) = selected_id else {
        return "Default".to_owned();
    };
    let label = selected_name.unwrap_or("Preset unavailable");
    if modified {
        format!("{label} (Modified)")
    } else {
        label.to_owned()
    }
}

fn preset_controls_modified(
    preset: &HashMap<String, ControlValue>,
    live: &HashMap<String, ControlValue>,
) -> bool {
    preset != live
}

/// A single item in the custom dropdown. Painted with its preset's own
/// swatch colour via the `--item-rgb` custom property on `.preset-option`.
#[component]
fn DropdownItem(
    #[prop(into)] value: String,
    #[prop(into)] label: String,
    #[prop(optional, into)] description: String,
    #[prop(into)] swatch_rgb: String,
    #[prop(into)] is_selected: Signal<bool>,
    on_click: Callback<String>,
) -> impl IntoView {
    let val = value.clone();
    let swatch_for_dot = swatch_rgb.clone();
    let has_description = !description.trim().is_empty();
    view! {
        <button
            type="button"
            class="preset-option w-full text-left pl-4 pr-3 py-[9px] text-xs cursor-pointer \
                   flex items-center gap-2.5"
            class=("preset-option-active", move || is_selected.get())
            class=("text-fg-tertiary", move || !is_selected.get())
            style=format!("--item-rgb: {swatch_rgb}")
            on:click=move |_| on_click.run(val.clone())
        >
            <span class="flex-1 min-w-0">
                <span class="block truncate">{label}</span>
                {has_description.then(|| view! {
                    <span class="block text-[10px] leading-relaxed text-fg-tertiary/70 mt-0.5 whitespace-normal">
                        {description.clone()}
                    </span>
                })}
            </span>

            // Right-side "● Now" dot when selected — pulses in the item's
            // own colour, so the active row's accent bleeds all the way
            // across from the left bar to the right-side marker.
            {move || is_selected.get().then(|| {
                let rgb = swatch_for_dot.clone();
                view! {
                    <span
                        class="w-1.5 h-1.5 rounded-full shrink-0 animate-pulse"
                        style=format!(
                            "background: rgb({rgb}); \
                             box-shadow: 0 0 8px rgba({rgb}, 0.95), \
                                         0 0 2px rgba({rgb}, 1)"
                        )
                    />
                }
            })}
        </button>
    }
}

/// Install a one-time document-level mousedown listener that closes the
/// dropdown when clicking outside `.preset-dropdown`.
fn install_dropdown_outside_handler(set_open: WriteSignal<bool>) {
    let Some(doc) = browser_document() else {
        return;
    };

    let _ = use_event_listener_with_options(
        doc,
        ev::mousedown,
        move |ev: leptos::ev::MouseEvent| {
            let inside = ev
                .target()
                .is_some_and(|target| target_closest(Some(target), ".preset-dropdown"));

            if !inside {
                set_open.set(false);
            }
        },
        UseEventListenerOptions::default().capture(true),
    );
}

#[component]
fn PresetDropdownDismissHandler(set_open: WriteSignal<bool>) -> impl IntoView {
    install_dropdown_outside_handler(set_open);
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use hypercolor_types::effect::ControlValue;

    use super::{preset_controls_modified, preset_display_label};

    #[test]
    fn selected_preset_label_keeps_provenance_when_modified() {
        assert_eq!(
            preset_display_label(Some("preset-id"), Some("Night Drive"), true),
            "Night Drive (Modified)"
        );
        assert_eq!(
            preset_display_label(Some("missing-id"), None, true),
            "Preset unavailable (Modified)"
        );
        assert_eq!(preset_display_label(None, None, false), "Default");
    }

    #[test]
    fn preset_modification_compares_live_layer_controls() {
        let preset = HashMap::from([("speed".to_owned(), ControlValue::Float(0.5))]);
        assert!(!preset_controls_modified(&preset, &preset));

        let live = HashMap::from([("speed".to_owned(), ControlValue::Float(0.75))]);
        assert!(preset_controls_modified(&preset, &live));
    }
}
