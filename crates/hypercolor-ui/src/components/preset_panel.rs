//! Preset toolbar — compact single-line preset selector with save/create/edit/delete.

use leptos::ev;
use leptos::prelude::*;
use leptos_icons::Icon;
use leptos_use::{UseEventListenerOptions, use_event_listener_with_options};
use std::collections::HashMap;
use wasm_bindgen::JsCast;

use hypercolor_types::effect::{ControlValue, PresetTemplate};

use super::preset_matching::{
    bundled_preset_matches_controls, bundled_preset_to_json, controls_to_json,
    user_preset_matches_controls,
};
use crate::api;
use crate::icons::*;
use crate::toasts;

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
fn hsl_to_rgb_triplet(h: f32, s: f32, l: f32) -> String {
    let c = (1.0 - (2.0f32 * l - 1.0).abs()) * s;
    let h_prime = h / 60.0;
    let x = c * (1.0 - ((h_prime % 2.0) - 1.0).abs());
    let (r1, g1, b1) = match h_prime as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    let to_byte = |v: f32| -> u8 { ((v + m) * 255.0).round() as u8 };
    format!("{}, {}, {}", to_byte(r1), to_byte(g1), to_byte(b1))
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
    let (presets, set_presets) = signal(Vec::<api::PresetSummary>::new());
    let (bundled_presets, set_bundled_presets) = signal(Vec::<PresetTemplate>::new());
    let (selected_id, set_selected_id) = signal(Option::<String>::None);
    let (mode, set_mode) = signal(ToolbarMode::Idle);
    let fetch_generation = StoredValue::new(0_u64);

    // Fetch user presets + bundled presets whenever effect_id *actually* changes.
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
            let next_presets = api::fetch_presets().await.unwrap_or_default();
            let next_bundled = if let Some(ref id) = fetch_eid {
                api::fetch_bundled_presets(id).await.unwrap_or_default()
            } else {
                Vec::new()
            };

            if fetch_generation.get_value() == request_generation
                && effect_id.get_untracked() == fetch_eid
            {
                set_presets.set(next_presets);
                set_bundled_presets.set(next_bundled);
            }
        });
        eid
    });

    // Keep the UI selection aligned with whichever preset the current control
    // values actually match. Built-in presets do not have engine-backed IDs,
    // so restoring from control values is the only reliable source of truth.
    Effect::new(move |_| {
        let eid = effect_id.get();
        let active_preset_id = active_preset_id_signal
            .map(|signal| signal.get())
            .unwrap_or_default();
        let current_values = control_values.get();
        let current_presets = presets.get();
        let current_bundled = bundled_presets.get();

        let next_selected = eid.as_ref().and_then(|active_effect_id| {
            active_preset_id
                .filter(|preset_id| {
                    current_presets.iter().any(|preset| {
                        preset.effect_id == *active_effect_id && preset.id == *preset_id
                    })
                })
                .or_else(|| {
                    current_presets
                        .iter()
                        .find(|preset| {
                            preset.effect_id == *active_effect_id
                                && user_preset_matches_controls(&current_values, &preset.controls)
                        })
                        .map(|preset| preset.id.clone())
                })
                .or_else(|| {
                    current_bundled
                        .iter()
                        .enumerate()
                        .find(|(_, preset)| {
                            bundled_preset_matches_controls(&current_values, &preset.controls)
                        })
                        .map(|(index, _)| format!("bundled:{index}"))
                })
        });

        if selected_id.get_untracked() != next_selected {
            set_selected_id.set(next_selected);
        }
    });

    // Filter presets to the active effect
    let effect_presets = Memo::new(move |_| {
        let eid = effect_id.get().unwrap_or_default();
        presets
            .get()
            .into_iter()
            .filter(|p| p.effect_id == eid)
            .collect::<Vec<_>>()
    });

    let selected_preset = Memo::new(move |_| {
        let sid = selected_id.get()?;
        effect_presets.get().into_iter().find(|p| p.id == sid)
    });

    let has_editable_selection = Memo::new(move |_| selected_preset.get().is_some());

    // Refresh helper
    let refresh_presets = move || {
        leptos::task::spawn_local(async move {
            if let Ok(all) = api::fetch_presets().await {
                set_presets.set(all);
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

        // Handle bundled preset selection (value = "bundled:<index>")
        if let Some(idx_str) = val.strip_prefix("bundled:") {
            if let Ok(idx) = idx_str.parse::<usize>() {
                let bp = bundled_presets.get();
                if let Some(template) = bp.get(idx) {
                    let controls_json = bundled_preset_to_json(&template.controls);
                    let previous_selection = selected_id.get_untracked();
                    set_selected_id.set(Some(val));
                    set_mode.set(ToolbarMode::Idle);
                    let on_applied = on_preset_applied;
                    leptos::task::spawn_local(async move {
                        match api::update_controls(&controls_json).await {
                            Ok(()) => on_applied.run(()),
                            Err(error) => {
                                set_selected_id.set(previous_selection);
                                toasts::toast_error(&format!(
                                    "Failed to apply bundled preset: {error}"
                                ));
                            }
                        }
                    });
                }
            }
            return;
        }

        let previous_selection = selected_id.get_untracked();
        set_selected_id.set(Some(val.clone()));
        set_mode.set(ToolbarMode::Idle);
        let on_applied = on_preset_applied;
        leptos::task::spawn_local(async move {
            match api::apply_preset(&val).await {
                Ok(()) => on_applied.run(()),
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
        let eid = effect_id.get().unwrap_or_default();
        let values = control_values.get();
        let controls_json = controls_to_json(&values);
        let name = preset.name.clone();
        let pid = preset.id.clone();
        let refresh = refresh_presets;
        leptos::task::spawn_local(async move {
            let req = api::CreatePresetRequest {
                name,
                description: None,
                effect: eid,
                controls: serde_json::Value::Object(controls_json),
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
        set_mode.set(ToolbarMode::Idle);
        leptos::task::spawn_local(async move {
            let req = api::CreatePresetRequest {
                name,
                description: None,
                effect: eid,
                controls: serde_json::Value::Object(controls_json),
                tags: None,
            };
            if let Ok(created) = api::create_preset(&req).await {
                set_selected_id.set(Some(created.id));
                toasts::toast_success("Preset created");
                refresh();
            }
        });
    };

    // Rename preset
    let on_rename = move |new_name: String| {
        let Some(preset) = selected_preset.get() else {
            return;
        };
        let eid = effect_id.get().unwrap_or_default();
        let pid = preset.id.clone();
        let refresh = refresh_presets;
        set_mode.set(ToolbarMode::Idle);
        leptos::task::spawn_local(async move {
            let req = api::CreatePresetRequest {
                name: new_name,
                description: None,
                effect: eid,
                controls: serde_json::Value::Object(
                    preset
                        .controls
                        .iter()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect(),
                ),
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
                                    effect_presets=effect_presets
                                    bundled_presets=bundled_presets
                                    selected_id=selected_id
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
    effect_presets: Memo<Vec<api::PresetSummary>>,
    bundled_presets: ReadSignal<Vec<PresetTemplate>>,
    selected_id: ReadSignal<Option<String>>,
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

        // Check bundled presets
        if let Some(idx_str) = sid.strip_prefix("bundled:")
            && let Ok(idx) = idx_str.parse::<usize>()
        {
            let bp = bundled_presets.get();
            if let Some(template) = bp.get(idx) {
                return template.name.clone();
            }
        }

        // Check user presets
        effect_presets
            .get()
            .iter()
            .find(|p| p.id == *sid)
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "Default".to_string())
    });

    // The swatch the trigger shows — pulled from whichever preset is
    // currently selected so the trigger button itself is tinted to match
    // the row that's "active" in the dropdown.
    let selected_swatch = Memo::new(move |_| {
        let sid = selected_id.get()?;
        if let Some(idx_str) = sid.strip_prefix("bundled:")
            && let Ok(idx) = idx_str.parse::<usize>()
        {
            let bp = bundled_presets.get();
            if let Some(template) = bp.get(idx) {
                return Some(preset_swatch(&template.name));
            }
        }
        effect_presets
            .get()
            .iter()
            .find(|p| p.id == sid)
            .map(|p| preset_swatch(&p.name))
    });

    // Has a real (non-Default) preset been picked? Drives the trigger
    // indicator dot and the accent-leaning background gradient.
    let has_selection = Memo::new(move |_| selected_id.get().is_some());

    // Click-outside handler — close dropdown when clicking outside
    install_dropdown_outside_handler(set_is_open);

    // Close on Escape
    let on_keydown = move |ev: web_sys::KeyboardEvent| {
        if ev.key() == "Escape" && is_open.get_untracked() {
            set_is_open.set(false);
            ev.prevent_default();
        }
    };

    view! {
        <div class="flex items-center gap-2" on:keydown=on_keydown>
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
                               animate-slide-down
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
                            let bp = bundled_presets.get();
                            let has_user = !effect_presets.get().is_empty();
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
                                    {bp.into_iter().enumerate().map(|(idx, p)| {
                                        let val = format!("bundled:{idx}");
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
                            let user = effect_presets.get();
                            let has_bundled = !bundled_presets.get().is_empty();
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
    let Some(doc) = web_sys::window().and_then(|w| w.document()) else {
        return;
    };

    let _ = use_event_listener_with_options(
        doc,
        ev::mousedown,
        move |ev: leptos::ev::MouseEvent| {
            let inside = ev
                .target()
                .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
                .map(|el| el.closest(".preset-dropdown").ok().flatten().is_some())
                .unwrap_or(false);

            if !inside {
                set_open.set(false);
            }
        },
        UseEventListenerOptions::default().capture(true),
    );
}

/// Action button group — extracted to keep tuple sizes manageable.
#[component]
fn PresetActionButtons(
    has_selection: Memo<bool>,
    on_save: impl Fn(leptos::ev::MouseEvent) + 'static,
    on_new: impl Fn(leptos::ev::MouseEvent) + 'static,
    on_edit: impl Fn(leptos::ev::MouseEvent) + 'static,
    on_delete: impl Fn(leptos::ev::MouseEvent) + 'static,
) -> impl IntoView {
    view! {
        <div class="flex items-center gap-0.5 shrink-0">
            // Save (overwrite current preset)
            <button
                class="p-1.5 rounded-lg text-fg-tertiary/40 toolbar-action
                       hover:text-success-green hover:bg-success-green/10"
                title="Save controls to preset"
                aria-label="Save controls to preset"
                disabled=move || !has_selection.get()
                on:click=on_save
            >
                <Icon icon=LuSave width="14px" height="14px" />
            </button>

            // New preset
            <button
                class="p-1.5 rounded-lg text-fg-tertiary/40 toolbar-action
                       hover:text-neon-cyan hover:bg-neon-cyan/10"
                title="Create new preset"
                aria-label="Create new preset"
                on:click=on_new
            >
                <Icon icon=LuPlus width="14px" height="14px" />
            </button>

            // Edit name
            <button
                class="p-1.5 rounded-lg text-fg-tertiary/40 toolbar-action
                       hover:text-electric-purple hover:bg-electric-purple/10"
                title="Rename preset"
                aria-label="Rename preset"
                disabled=move || !has_selection.get()
                on:click=on_edit
            >
                <Icon icon=LuSquarePen width="14px" height="14px" />
            </button>

            // Delete
            <button
                class="p-1.5 rounded-lg text-fg-tertiary/40 toolbar-action
                       hover:text-error-red hover:bg-error-red/10"
                title="Delete preset"
                aria-label="Delete preset"
                disabled=move || !has_selection.get()
                on:click=on_delete
            >
                <Icon icon=LuTrash2 width="14px" height="14px" />
            </button>
        </div>
    }
}

/// Inline text input for creating or renaming a preset.
#[component]
fn InlineNameInput(
    placeholder: &'static str,
    #[prop(into)] initial: String,
    on_submit: Callback<String>,
    on_cancel: Callback<()>,
) -> impl IntoView {
    let (value, set_value) = signal(initial);

    view! {
        <div class="flex items-center gap-2">
            <input
                type="text"
                placeholder=placeholder
                class="flex-1 bg-surface-sunken/60 border border-accent-muted/60 rounded-lg px-2.5 py-1.5
                       text-xs text-fg-primary placeholder-fg-tertiary/40
                       focus:outline-none focus:border-accent glow-ring
                       transition-all duration-200"
                prop:value=move || value.get()
                on:input=move |ev| {
                    let target = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                    if let Some(el) = target {
                        set_value.set(el.value());
                    }
                }
                on:keydown=move |ev| {
                    if ev.key() == "Enter" {
                        let name = value.get().trim().to_string();
                        if !name.is_empty() {
                            on_submit.run(name);
                        }
                    } else if ev.key() == "Escape" {
                        on_cancel.run(());
                    }
                }
            />
            <InlineNameButtons
                value=value
                on_submit=on_submit
                on_cancel=on_cancel
            />
        </div>
    }
}

/// Confirm/Cancel buttons for inline name input.
#[component]
fn InlineNameButtons(
    value: ReadSignal<String>,
    on_submit: Callback<String>,
    on_cancel: Callback<()>,
) -> impl IntoView {
    view! {
        <button
            class="p-1.5 rounded-lg text-fg-tertiary/40 toolbar-action
                   hover:text-success-green hover:bg-success-green/10"
            title="Confirm"
            disabled=move || value.get().trim().is_empty()
            on:click=move |_| {
                let name = value.get().trim().to_string();
                if !name.is_empty() {
                    on_submit.run(name);
                }
            }
        >
            <Icon icon=LuCheck width="14px" height="14px" />
        </button>
        <button
            class="p-1.5 rounded-lg text-fg-tertiary/40 toolbar-action
                   hover:text-error-red hover:bg-error-red/10"
            title="Cancel"
            on:click=move |_| on_cancel.run(())
        >
            <Icon icon=LuX width="14px" height="14px" />
        </button>
    }
}
