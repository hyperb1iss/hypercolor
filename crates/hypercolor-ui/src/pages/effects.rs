//! Effects browse page — grid of effect cards with filtering and detail panel.

use leptos::prelude::*;
use leptos_icons::Icon;
use leptos_router::hooks::use_params_map;
use leptos_use::use_debounce_fn;

use crate::api;
use crate::app::{EffectsContext, WsContext};
use crate::components::calibration_guide::CalibrationGuide;
use crate::components::control_panel::ControlPanel;
use crate::components::effect_card::EffectCard;
use crate::components::install_effect_panel::InstallEffectPanel;
use crate::components::page_header::{HeaderToolbar, HeaderTrailing, PageAccent, PageHeader};
use crate::components::page_search_bar::PageSearchBar;
use crate::components::preview_cabinet::PreviewCabinet;
use crate::components::resize_handle::ResizeHandle;
use crate::components::section_label::{LabelSize, LabelTone, label_class};
use crate::icons::*;
use crate::toasts;
use hypercolor_types::effect::{ControlDefinition, ControlType, ControlValue};
use hypercolor_types::scene::{SceneKind, SceneMutationMode};

use crate::style_utils::{category_accent_rgb, filter_chips};

const EFFECTS_PREVIEW_FPS_CAP: u32 = 60;
const MIN_DETAIL_WIDTH: f64 = 260.0;
const MAX_DETAIL_WIDTH: f64 = 1200.0;
const MIN_CONTROLS_WIDTH: f64 = 220.0;
const MAX_CONTROLS_WIDTH: f64 = 800.0;

/// Category filter chips — (label, accent RGB).
///
/// `display` is deliberately absent here. Display faces are authored with
/// the Face SDK and rendered on LCD devices; they live on the `/displays`
/// page and never belong in the LED effects browser. The `filtered_effects`
/// memo below also strips any stray `display`-category effect so a
/// misclassified face can't leak into the gallery.
const CATEGORY_CHIPS: &[(&str, &str)] = &[
    ("all", "225, 53, 255"),
    ("ambient", "128, 255, 234"),
    ("gaming", "225, 53, 255"),
    ("reactive", "241, 250, 140"),
    ("generative", "80, 250, 123"),
    ("interactive", "130, 170, 255"),
    ("productivity", "255, 153, 255"),
    ("utility", "139, 133, 160"),
];

/// Effects browse page with compact grid, search, category filtering, and live preview.
#[component]
pub fn EffectsPage() -> impl IntoView {
    let ws = expect_context::<WsContext>();
    let fx = expect_context::<EffectsContext>();
    let route_params = use_params_map();

    Effect::new(move |_| {
        ws.set_preview_cap.set(EFFECTS_PREVIEW_FPS_CAP);
        ws.set_preview_width_cap.set(0);
    });
    on_cleanup(move || {
        ws.set_preview_cap.set(crate::ws::DEFAULT_PREVIEW_FPS_CAP);
        ws.set_preview_width_cap.set(0);
    });

    // Restore persisted filter state from localStorage
    let (search, set_search) = signal(String::new());
    let (category_filter, set_category_filter) =
        signal(crate::storage::get("hc-fx-category").unwrap_or_else(|| "all".to_string()));
    let (selected_authors, set_selected_authors) = signal({
        crate::storage::get("hc-fx-authors")
            .map(|s| {
                s.split(',')
                    .filter(|a| !a.is_empty())
                    .map(String::from)
                    .collect::<std::collections::BTreeSet<_>>()
            })
            .unwrap_or_default()
    });
    let (favorites_only, set_favorites_only) =
        signal(crate::storage::get("hc-fx-favorites").as_deref() == Some("true"));
    let (audio_reactive_only, set_audio_reactive_only) =
        signal(crate::storage::get("hc-fx-audio").as_deref() == Some("true"));

    // Panel layout state (persisted to localStorage)
    let (detail_width, set_detail_width) = signal(crate::storage::get_clamped(
        "hc-fx-detail-width",
        380.0,
        MIN_DETAIL_WIDTH,
        MAX_DETAIL_WIDTH,
    ));
    let (controls_detached, set_controls_detached) =
        signal(crate::storage::get("hc-fx-controls-detached").as_deref() != Some("false"));
    let (controls_width, set_controls_width) = signal(crate::storage::get_clamped(
        "hc-fx-controls-width",
        320.0,
        MIN_CONTROLS_WIDTH,
        MAX_CONTROLS_WIDTH,
    ));
    let pending_control_updates =
        StoredValue::new(std::collections::HashMap::<String, serde_json::Value>::new());
    let control_request_epoch = StoredValue::new(0_u64);
    let preferences_store = use_context::<crate::preferences::PreferencesStore>();
    let flush_control_updates = use_debounce_fn(
        move || {
            let updates = pending_control_updates
                .try_update_value(std::mem::take)
                .unwrap_or_default();
            if updates.is_empty() {
                return;
            }

            let controls_json =
                serde_json::Value::Object(updates.into_iter().collect::<serde_json::Map<_, _>>());
            let request_epoch = control_request_epoch
                .try_update_value(|epoch| {
                    *epoch = epoch.wrapping_add(1);
                    *epoch
                })
                .unwrap_or_default();
            let active_effect_id = fx.active_effect_id.get_untracked();
            leptos::task::spawn_local(async move {
                match api::update_controls(&controls_json).await {
                    Ok(()) => {
                        let is_latest_request = control_request_epoch.get_value() == request_epoch;
                        let has_pending_updates =
                            pending_control_updates.with_value(|pending| !pending.is_empty());
                        let same_effect = fx.active_effect_id.get_untracked() == active_effect_id;
                        if is_latest_request
                            && !has_pending_updates
                            && same_effect
                            && let Some(store) = preferences_store
                            && let Some(effect_id) = active_effect_id
                        {
                            store.save(
                                effect_id,
                                crate::preferences::EffectPreferences {
                                    preset_id: fx.active_preset_id.get_untracked(),
                                    control_values: fx.active_control_values.get_untracked(),
                                },
                            );
                        }
                    }
                    Err(error) => {
                        let is_latest_request = control_request_epoch.get_value() == request_epoch;
                        let has_pending_updates =
                            pending_control_updates.with_value(|pending| !pending.is_empty());
                        let same_effect = fx.active_effect_id.get_untracked() == active_effect_id;
                        if is_latest_request && !has_pending_updates && same_effect {
                            fx.refresh_active_effect();
                            toasts::toast_error(&format!(
                                "Failed to update effect controls: {error}"
                            ));
                        }
                    }
                }
            });
        },
        75.0,
    );

    // Persist filter changes to localStorage
    Effect::new(move |_| {
        crate::storage::set("hc-fx-category", &category_filter.get());
        crate::storage::set("hc-fx-favorites", &favorites_only.get().to_string());
        crate::storage::set("hc-fx-audio", &audio_reactive_only.get().to_string());
        let authors_str: String = selected_authors
            .get()
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join(",");
        crate::storage::set("hc-fx-authors", &authors_str);
    });

    // Derive unique sorted author list from loaded effects.
    // Skip display-face authors — they're only relevant on `/displays`
    // and would pollute the effects-page author chip list.
    let authors = Memo::new(move |_| {
        let mut seen = std::collections::BTreeSet::new();
        fx.effects_index.with(|effects| {
            for entry in effects {
                if entry.effect.category.eq_ignore_ascii_case("display") {
                    continue;
                }
                if !entry.effect.author.is_empty() {
                    seen.insert(entry.effect.author.clone());
                }
            }
        });
        seen.into_iter().collect::<Vec<_>>()
    });

    // Count how many effects are favorited (for the badge)
    let favorites_count = Memo::new(move |_| fx.favorite_ids.get().len());

    let controls: Signal<Vec<ControlDefinition>> = fx.active_controls.into();
    let control_values: Signal<std::collections::HashMap<String, ControlValue>> =
        fx.active_control_values.into();
    let accent_rgb =
        Signal::derive(move || category_accent_rgb(&fx.active_effect_category.get()).to_string());
    let active_effect_id_signal = Signal::derive(move || fx.active_effect_id.get());

    let active_effect_summary = Memo::new(move |_| {
        let active_id = fx.active_effect_id.get()?;
        fx.effects_index.with(|effects| {
            effects
                .iter()
                .find(|entry| entry.effect.id == active_id)
                .map(|entry| entry.effect.clone())
        })
    });
    let routed_effect_id = Memo::new(move |_| {
        let route_effect_id = route_params.with(|params| params.get("id"))?;
        fx.effects_index.with(|effects| {
            effects
                .iter()
                .find(|entry| entry.effect.id == route_effect_id)
                .and_then(|entry| {
                    (entry.effect.runnable
                        && !entry.effect.category.eq_ignore_ascii_case("display"))
                    .then(|| entry.effect.id.clone())
                })
        })
    });
    Effect::new(move |previous_route_effect_id: Option<Option<String>>| {
        let current_route_effect_id = routed_effect_id.get();
        if previous_route_effect_id.as_ref() == Some(&current_route_effect_id) {
            return current_route_effect_id;
        }

        if let Some(effect_id) = current_route_effect_id.as_ref()
            && fx.active_effect_id.get_untracked().as_deref() != Some(effect_id.as_str())
        {
            fx.apply_effect(effect_id.clone());
        }

        current_route_effect_id
    });
    let show_calibration_guide = Memo::new(move |_| {
        active_effect_summary.get().is_some_and(|effect| {
            effect.name.eq_ignore_ascii_case("Calibration")
                || effect
                    .tags
                    .iter()
                    .any(|tag| tag.eq_ignore_ascii_case("calibration"))
        })
    });

    let has_active = Memo::new(move |_| fx.active_effect_id.get().is_some());
    let named_scene_warning = Memo::new(move |_| {
        (fx.active_scene_kind.get() == Some(SceneKind::Named)).then(|| {
            (
                fx.active_scene_name
                    .get()
                    .unwrap_or_else(|| "Active scene".to_owned()),
                fx.active_scene_mutation_mode.get() == Some(SceneMutationMode::Snapshot),
            )
        })
    });
    let degraded_effect = Memo::new(move |_| {
        let effect_error = fx.last_effect_error.get()?;
        let effect = fx.effects_index.with(|effects| {
            effects
                .iter()
                .find(|entry| entry.effect.id == effect_error.effect_id)
                .map(|entry| entry.effect.clone())
        })?;
        if effect.category.eq_ignore_ascii_case("display") {
            return None;
        }

        Some((
            effect.name,
            match effect_error.fallback.as_deref() {
                Some("clear_groups") => {
                    "The daemon cleared this effect from the active scene after a render failure."
                        .to_owned()
                }
                Some(fallback) if !fallback.is_empty() => {
                    format!("The daemon applied fallback \"{fallback}\" after a render failure.")
                }
                _ => "The daemon reported a render failure for this effect.".to_owned(),
            },
        ))
    });
    let (returning_to_default, set_returning_to_default) = signal(false);

    // Filter effects
    let filtered_effects = Memo::new(move |_| {
        let search_term = search.get().trim().to_lowercase();
        let cat = category_filter.get();
        let sel_authors = selected_authors.get();
        let fav_only = favorites_only.get();
        let audio_only = audio_reactive_only.get();
        let fav_ids = fx.favorite_ids.get();

        fx.effects_index.with(|effects| {
            effects
                .iter()
                .filter(|entry| {
                    let effect = &entry.effect;
                    // Display faces are authored with the Face SDK and
                    // assigned on the `/displays` page. They never
                    // belong in the LED effects gallery, regardless of
                    // whether the user flipped some URL-state or old
                    // persisted filter to `display`.
                    if effect.category.eq_ignore_ascii_case("display") {
                        return false;
                    }
                    if cat != "all" && effect.category != cat {
                        return false;
                    }
                    if !sel_authors.is_empty() && !sel_authors.contains(&effect.author) {
                        return false;
                    }
                    if fav_only && !fav_ids.contains(&effect.id) {
                        return false;
                    }
                    if audio_only && !effect.audio_reactive {
                        return false;
                    }
                    entry.matches_search(&search_term)
                })
                .map(|entry| entry.effect.clone())
                .collect::<Vec<_>>()
        })
    });
    let total_effects = Memo::new(move |_| {
        fx.effects_index.with(|effects| {
            effects
                .iter()
                .filter(|entry| !entry.effect.category.eq_ignore_ascii_case("display"))
                .count()
        })
    });

    // Apply effect handler — delegates to shared context
    let on_apply = Callback::new(move |id: String| {
        let is_display_face = fx.effects_index.with(|effects| {
            effects
                .iter()
                .find(|entry| entry.effect.id == id)
                .is_some_and(|entry| entry.effect.category == "display")
        });
        if is_display_face {
            toasts::toast_info("Display faces are assigned from the Displays page.");
            return;
        }
        fx.apply_effect(id);
    });

    let on_return_to_default = Callback::new(move |_| {
        if returning_to_default.get_untracked() {
            return;
        }

        set_returning_to_default.set(true);
        let ctx = fx;
        leptos::task::spawn_local(async move {
            if api::deactivate_scene().await.is_ok() {
                ctx.set_last_effect_error.set(None);
                ctx.refresh_active_scene();
                ctx.refresh_active_effect();
                toasts::toast_success("Returned to Default scene.");
            } else {
                toasts::toast_error("Couldn't return to Default scene.");
            }
            set_returning_to_default.set(false);
        });
    });

    // Toggle favorite handler
    let on_toggle_favorite = Callback::new(move |id: String| {
        fx.toggle_favorite(id);
    });

    // Control change handler
    let on_control_change = Callback::new(move |(name, value): (String, serde_json::Value)| {
        if fx.active_effect_id.get().is_none() {
            return;
        }

        let controls_snapshot = fx.active_controls.get();
        let current_values = fx.active_control_values.get();
        let updates = expand_control_updates(
            fx.active_effect_name.get().as_deref(),
            &current_values,
            &name,
            &value,
        );

        fx.set_active_control_values.update({
            let controls_snapshot = controls_snapshot.clone();
            let updates = updates.clone();
            move |values| {
                for (control_name, raw_value) in &updates {
                    if let Some(control_value) =
                        json_to_control_value(control_name, &controls_snapshot, raw_value)
                    {
                        values.insert(control_name.clone(), control_value);
                    }
                }
            }
        });

        pending_control_updates.update_value(|pending| {
            for (control_name, raw_value) in &updates {
                pending.insert(control_name.clone(), raw_value.clone());
            }
        });

        flush_control_updates();
    });

    // Resize callbacks for detail and controls panels
    let (on_detail_drag_start, on_detail_drag, on_detail_drag_end) = drag_callbacks(
        detail_width,
        set_detail_width,
        MIN_DETAIL_WIDTH,
        MAX_DETAIL_WIDTH,
        "hc-fx-detail-width",
    );
    let (on_controls_drag_start, on_controls_drag, on_controls_drag_end) = drag_callbacks(
        controls_width,
        set_controls_width,
        MIN_CONTROLS_WIDTH,
        MAX_CONTROLS_WIDTH,
        "hc-fx-controls-width",
    );

    // Track which filter dropdown section is expanded (if any)
    let (filter_dropdown_open, set_filter_dropdown_open) = signal(false);

    // Count active filters for badge
    let active_filter_count = Memo::new(move |_| {
        let mut count = 0usize;
        if category_filter.get() != "all" {
            count += 1;
        }
        if favorites_only.get() {
            count += 1;
        }
        if audio_reactive_only.get() {
            count += 1;
        }
        count += selected_authors.get().len();
        count
    });

    view! {
        <div class="flex h-full min-h-0 flex-col">
            <PageHeader
                icon=LuZap
                title="Effects"
                tagline="Browse and tune effects"
                accent=PageAccent::Purple
            >
                <HeaderTrailing slot>
                    <span class="shrink-0 text-[11px] font-mono text-fg-tertiary/55 tabular-nums">
                        {move || {
                            let total = total_effects.get();
                            let filtered = filtered_effects.get().len();
                            if filtered == total {
                                format!("{total} effects")
                            } else {
                                format!("{filtered}/{total} effects")
                            }
                        }}
                    </span>
                    <InstallEffectPanel />
                </HeaderTrailing>
                <HeaderToolbar slot>
                    <PageSearchBar
                        placeholder="Search effects..."
                        value=search
                        set_value=set_search
                    />

                    // Filters dropdown trigger
                    <div class="relative shrink-0">
                        <button
                            class="flex items-center gap-1.5 px-2.5 py-1.5 rounded-lg text-xs font-medium border transition-all duration-200"
                            style=move || {
                                if active_filter_count.get() > 0 {
                                    "background: rgba(225, 53, 255, 0.12); color: rgb(225, 53, 255); border-color: rgba(225, 53, 255, 0.25); box-shadow: 0 0 10px rgba(225, 53, 255, 0.15)"
                                } else {
                                    "color: rgba(139, 133, 160, 0.6); border-color: rgba(139, 133, 160, 0.12); background: rgba(139, 133, 160, 0.03)"
                                }
                            }
                            on:click=move |_| set_filter_dropdown_open.update(|v| *v = !*v)
                        >
                            <Icon icon=LuSlidersHorizontal width="13px" height="13px" />
                            "Filters"
                            {move || {
                                let count = active_filter_count.get();
                                (count > 0).then(|| view! {
                                    <span
                                        class="min-w-[16px] h-4 flex items-center justify-center rounded-full text-[9px] font-mono"
                                        style="background: rgba(225, 53, 255, 0.3); color: rgb(225, 53, 255)"
                                    >
                                        {count}
                                    </span>
                                })
                            }}
                            <span
                                class="w-3 h-3 flex items-center justify-center transition-transform duration-200"
                                class:rotate-180=move || filter_dropdown_open.get()
                            >
                                <Icon icon=LuChevronDown width="11px" height="11px" />
                            </span>
                        </button>

                        // Filter dropdown panel
                        {move || filter_dropdown_open.get().then(|| {
                            let author_list = authors.get();
                            view! {
                                // Backdrop
                                <div
                                    class="fixed inset-0 z-20"
                                    on:click=move |_| set_filter_dropdown_open.set(false)
                                />
                                <div
                                    class="absolute top-full right-0 mt-1 z-30 w-[240px] max-h-[360px] overflow-y-auto
                                           rounded-xl border border-edge-subtle bg-surface-overlay dropdown-glow
                                           py-1.5 animate-fade-in animate-glow-reveal scrollbar-dropdown"
                                >
                                    // ── Category section ──
                                    <div class="px-3 pt-1 pb-1.5">
                                        <div class=format!("{} mb-1.5", label_class(LabelSize::Small, LabelTone::Subtle))>"Category"</div>
                                        <div class="flex gap-1 flex-wrap">
                                            {filter_chips(CATEGORY_CHIPS, category_filter, set_category_filter)}
                                        </div>
                                    </div>

                                    <div class="h-px bg-border-subtle/30 mx-2 my-1" />

                                    // ── Toggles section ──
                                    <div class="px-3 py-1">
                                        // Favorites toggle
                                        <button
                                            class="w-full flex items-center gap-2.5 px-2 py-1.5 rounded-lg text-xs hover:bg-surface-hover/40 transition-colors group"
                                            on:click=move |_| set_favorites_only.update(|v| *v = !*v)
                                        >
                                            <div
                                                class="w-3.5 h-3.5 rounded border flex items-center justify-center shrink-0 transition-all duration-150"
                                                style=move || {
                                                    if favorites_only.get() {
                                                        "background: rgba(255, 106, 193, 0.8); border-color: rgb(255, 106, 193)"
                                                    } else {
                                                        "border-color: rgba(255, 255, 255, 0.1); background: transparent"
                                                    }
                                                }
                                            >
                                                {move || favorites_only.get().then(|| view! {
                                                    <Icon icon=LuCheck width="10px" height="10px" style="color: white" />
                                                })}
                                            </div>
                                            <Icon icon=LuHeart width="11px" height="11px" style="color: rgba(255, 106, 193, 0.6)" />
                                            <span class="text-fg-secondary group-hover:text-fg-primary transition-colors">"Favorites"</span>
                                            {move || {
                                                let count = favorites_count.get();
                                                (count > 0).then(|| view! {
                                                    <span class="text-[9px] font-mono text-fg-tertiary ml-auto">{count}</span>
                                                })
                                            }}
                                        </button>

                                        // Audio reactive toggle
                                        <button
                                            class="w-full flex items-center gap-2.5 px-2 py-1.5 rounded-lg text-xs hover:bg-surface-hover/40 transition-colors group"
                                            on:click=move |_| set_audio_reactive_only.update(|v| *v = !*v)
                                        >
                                            <div
                                                class="w-3.5 h-3.5 rounded border flex items-center justify-center shrink-0 transition-all duration-150"
                                                style=move || {
                                                    if audio_reactive_only.get() {
                                                        "background: rgba(128, 255, 234, 0.8); border-color: rgb(128, 255, 234)"
                                                    } else {
                                                        "border-color: rgba(255, 255, 255, 0.1); background: transparent"
                                                    }
                                                }
                                            >
                                                {move || audio_reactive_only.get().then(|| view! {
                                                    <Icon icon=LuCheck width="10px" height="10px" style="color: white" />
                                                })}
                                            </div>
                                            <Icon icon=LuAudioLines width="11px" height="11px" style="color: rgba(128, 255, 234, 0.6)" />
                                            <span class="text-fg-secondary group-hover:text-fg-primary transition-colors">"Audio reactive"</span>
                                        </button>
                                    </div>

                                    // ── Authors section ──
                                    {(author_list.len() > 1).then(|| {
                                        let list = author_list.clone();
                                        view! {
                                            <div class="h-px bg-border-subtle/30 mx-2 my-1" />
                                            <div class="px-3 pt-1 pb-1">
                                                <div class=format!("{} mb-1", label_class(LabelSize::Small, LabelTone::Subtle))>"Author"</div>
                                                // Clear all
                                                <button
                                                    class="w-full flex items-center gap-2.5 px-2 py-1 rounded-lg text-[11px] text-fg-tertiary hover:bg-surface-hover/40 transition-colors"
                                                    on:click=move |_| set_selected_authors.set(std::collections::BTreeSet::new())
                                                >
                                                    "All authors"
                                                </button>
                                                {list.into_iter().map(|author| {
                                                    let author_toggle = author.clone();
                                                    let author_check = author.clone();
                                                    let author_check2 = author.clone();
                                                    let author_label = author.clone();
                                                    view! {
                                                        <button
                                                            class="w-full flex items-center gap-2.5 px-2 py-1 rounded-lg text-xs hover:bg-surface-hover/40 transition-colors group"
                                                            on:click=move |_| {
                                                                let a = author_toggle.clone();
                                                                set_selected_authors.update(move |set| {
                                                                    if !set.remove(&a) {
                                                                        set.insert(a);
                                                                    }
                                                                });
                                                            }
                                                        >
                                                            <div
                                                                class="w-3.5 h-3.5 rounded border flex items-center justify-center shrink-0 transition-all duration-150"
                                                                style=move || {
                                                                    if selected_authors.get().contains(&author_check) {
                                                                        "background: rgba(255, 106, 193, 0.8); border-color: rgb(255, 106, 193)"
                                                                    } else {
                                                                        "border-color: rgba(255, 255, 255, 0.1); background: transparent"
                                                                    }
                                                                }
                                                            >
                                                                {move || selected_authors.get().contains(&author_check2).then(|| view! {
                                                                    <Icon icon=LuCheck width="10px" height="10px" style="color: white" />
                                                                })}
                                                            </div>
                                                            <span class="text-fg-secondary group-hover:text-fg-primary transition-colors truncate">
                                                                {author_label.clone()}
                                                            </span>
                                                        </button>
                                                    }
                                                }).collect_view()}
                                            </div>
                                        }
                                    })}

                                    // ── Clear all filters ──
                                    {move || (active_filter_count.get() > 0).then(|| view! {
                                        <div class="h-px bg-border-subtle/30 mx-2 my-1" />
                                        <button
                                            class="w-full text-left px-5 py-1.5 text-[11px] text-fg-tertiary hover:text-fg-secondary hover:bg-surface-hover/40 transition-colors"
                                            on:click=move |_| {
                                                set_category_filter.set("all".to_string());
                                                set_favorites_only.set(false);
                                                set_audio_reactive_only.set(false);
                                                set_selected_authors.set(std::collections::BTreeSet::new());
                                            }
                                        >
                                            "Clear all filters"
                                        </button>
                                    })}
                                </div>
                            }
                        })}
                    </div>
                </HeaderToolbar>
            </PageHeader>

            {move || named_scene_warning.get().map(|(scene_name, snapshot_locked)| view! {
                <div class="px-6 pt-4">
                        <div class="rounded-xl border border-[rgba(241,250,140,0.24)] bg-[rgba(241,250,140,0.08)] px-4 py-3 shadow-[0_0_24px_rgba(241,250,140,0.08)]">
                            <div class="flex items-start gap-3">
                                <div class="mt-0.5 shrink-0 text-[rgba(241,250,140,0.9)]">
                                    <Icon icon=LuTriangleAlert width="14px" height="14px" />
                                </div>
                                <div class="min-w-0 flex-1">
                                    <div class="text-[11px] font-semibold uppercase tracking-[0.16em] text-[rgba(241,250,140,0.82)]">
                                        {if snapshot_locked { "Snapshot Scene Locked" } else { "Named Scene Active" }}
                                    </div>
                                    <div class="mt-1 text-sm leading-5 text-fg-secondary">
                                        <span class="text-fg-primary">{scene_name.clone()}</span>
                                        {if snapshot_locked {
                                            " is snapshot-locked. Return to Default before applying an effect or changing its controls."
                                        } else {
                                            " is active. Applying an effect here rewrites that scene’s primary effect."
                                        }}
                                    </div>
                                </div>
                                <button
                                    class="shrink-0 rounded-lg border border-[rgba(241,250,140,0.28)] px-3 py-1.5 text-[11px] font-medium text-[rgba(241,250,140,0.92)] transition-all duration-200 hover:bg-[rgba(241,250,140,0.08)] disabled:cursor-wait disabled:opacity-60"
                                    disabled=move || returning_to_default.get()
                                    on:click=move |_| on_return_to_default.run(())
                                >
                                    {move || if returning_to_default.get() {
                                        "Returning..."
                                    } else {
                                        "Return to Default"
                                    }}
                                </button>
                            </div>
                        </div>
                    </div>
                })}
            {move || degraded_effect.get().map(|(effect_name, detail)| view! {
                <div class="px-6 pt-3">
                    <div class="rounded-xl border border-[rgba(255,99,99,0.28)] bg-[rgba(255,99,99,0.10)] px-4 py-3 shadow-[0_0_24px_rgba(255,99,99,0.10)]">
                        <div class="flex items-start gap-3">
                            <div class="mt-0.5 shrink-0 text-[rgba(255,99,99,0.94)]">
                                <Icon icon=LuTriangleAlert width="14px" height="14px" />
                            </div>
                            <div class="min-w-0 flex-1">
                                <div class="text-[11px] font-semibold uppercase tracking-[0.16em] text-[rgba(255,99,99,0.84)]">
                                    "Degraded Effect"
                                </div>
                                <div class="mt-1 text-sm leading-5 text-fg-secondary">
                                    <span class="text-fg-primary">{effect_name}</span>
                                    " is degraded. "
                                    {detail}
                                </div>
                            </div>
                        </div>
                    </div>
                </div>
            })}

            <div class="flex-1 min-h-0 px-6 pb-6 pt-4 flex">
                // Left column — effect grid, independent from the detail panels
                <div class="flex-1 min-w-0 flex flex-col" style="min-width: 120px">

                // Effect grid — independently scrollable, below the locked header
                <div class="flex-1 min-h-0 overflow-y-auto">
                    <Suspense fallback=move || view! { <LoadingSkeleton /> }>
                        {move || {
                            let effects = filtered_effects.get();
                            if effects.is_empty() {
                                view! {
                                    <div class="text-center py-20">
                                        <div class="text-fg-tertiary text-sm">"No effects found"</div>
                                        <div class="text-fg-tertiary/50 text-xs mt-1">"Try a different search or category"</div>
                                    </div>
                                }.into_any()
                            } else {
                                let grid_class = if has_active.get() {
                                    "grid grid-cols-[repeat(auto-fill,minmax(240px,1fr))] gap-3"
                                } else {
                                    "grid grid-cols-[repeat(auto-fill,minmax(300px,1fr))] gap-4"
                                };
                                view! {
                                    <div class=grid_class>
                                        <For
                                            each=move || filtered_effects.get()
                                            key=|effect| effect.id.clone()
                                            children=move |effect| {
                                                let effect_id = effect.id.clone();
                                                let fav_effect_id = effect.id.clone();
                                                let is_active = Signal::derive(move || {
                                                    fx.active_effect_id.get().as_deref() == Some(effect_id.as_str())
                                                });
                                                let is_favorite = Signal::derive(move || {
                                                    fx.favorite_ids.get().contains(&fav_effect_id)
                                                });
                                                view! {
                                                    <EffectCard
                                                        effect=effect
                                                        is_active=is_active
                                                        is_favorite=is_favorite
                                                        on_apply=on_apply
                                                        on_toggle_favorite=on_toggle_favorite
                                                    />
                                                }
                                            }
                                        />
                                    </div>
                                }.into_any()
                            }
                        }}
                    </Suspense>
                </div>
            </div>

            // Detail panel(s) — right side, visible when an effect is selected.
            //
            // Gate this subtree on presence rather than the specific effect ID
            // so switching between effects doesn't remount CanvasPreview and
            // briefly drop the preview subscription.
            {move || {
                has_active.get().then(|| {
                    view! {
                        <div style="display: contents">
                            // Resize handle between grid and preview
                            <ResizeHandle
                                on_drag_start=on_detail_drag_start
                                on_drag=on_detail_drag
                                on_drag_end=on_detail_drag_end
                            />

                            // Preview panel (always visible when effect selected)
                            <aside
                                class="shrink-0 flex flex-col min-h-0 animate-slide-in-right"
                                style=move || format!("width: {}px", detail_width.get())
                            >
                                // Unified cinematic cabinet — canvas + preset strip, shared
                                // with the dashboard. The effects page owns no telemetry
                                // reporting, so `report_telemetry` stays at its default
                                // `false`; canvas sizes via its own aspect ratio here.
                                <div class="shrink-0 pb-3">
                                    <PreviewCabinet />
                                </div>

                                // Controls (docked mode — inside preview panel)
                                {move || (!controls_detached.get()).then(|| {
                                    view! {
                                        <div
                                            class="flex-1 min-h-0 overflow-y-auto"
                                            style="overscroll-behavior: contain"
                                        >
                                            <div class="space-y-3">
                                                {move || show_calibration_guide.get().then(|| view! {
                                                    <CalibrationGuide
                                                        effect_id=active_effect_id_signal
                                                        control_values=control_values
                                                        accent_rgb=accent_rgb
                                                    />
                                                })}
                                                <div
                                                    class="rounded-xl bg-surface-raised/80 border border-edge-subtle p-3 edge-glow"
                                                    style:border-top=move || format!("2px solid rgba({}, 0.2)", accent_rgb.get())
                                                >
                                                    <div class="flex items-center gap-2 mb-3 pb-2 border-b border-edge-subtle/50">
                                                        <div
                                                            class="w-6 h-6 rounded-md flex items-center justify-center"
                                                            style=move || format!(
                                                                "background: rgba({0}, 0.1); box-shadow: 0 0 8px rgba({0}, 0.08)",
                                                                accent_rgb.get()
                                                            )
                                                        >
                                                            <span style=move || format!("color: rgba({}, 0.7)", accent_rgb.get())>
                                                                <Icon icon=LuSettings2 width="13px" height="13px" />
                                                            </span>
                                                        </div>
                                                        <h3 class=label_class(LabelSize::Small, LabelTone::Strong)>
                                                            "Controls"
                                                        </h3>
                                                        <div class="flex-1" />
                                                        <button
                                                            class="p-1 rounded-md hover:bg-surface-hover/40 text-fg-tertiary/50 hover:text-fg-secondary transition-all duration-150"
                                                            title="Float controls into separate panel"
                                                            on:click=move |_| {
                                                                set_controls_detached.set(true);
                                                                persist_to_storage("hc-fx-controls-detached", "true");
                                                            }
                                                        >
                                                            <Icon icon=LuUnlink width="11px" height="11px" />
                                                        </button>
                                                    </div>
                                                    <ControlPanel
                                                        controls=controls
                                                        control_values=control_values
                                                        accent_rgb=accent_rgb
                                                        on_change=on_control_change
                                                    />
                                                </div>
                                            </div>
                                        </div>
                                    }
                                })}

                            </aside>

                            // Controls (detached mode — own column with resize handle)
                            {move || controls_detached.get().then(|| {
                                view! {
                                    <div style="display: contents">
                                        <ResizeHandle
                                            on_drag_start=on_controls_drag_start
                                            on_drag=on_controls_drag
                                            on_drag_end=on_controls_drag_end
                                        />
                                        <aside
                                            class="shrink-0 flex flex-col min-h-0 animate-slide-in-right"
                                            style=move || format!("width: {}px", controls_width.get())
                                        >
                                            <div
                                                class="flex-1 min-h-0 overflow-y-auto"
                                                style="overscroll-behavior: contain"
                                            >
                                                <div class="space-y-3">
                                                    {move || show_calibration_guide.get().then(|| view! {
                                                        <CalibrationGuide
                                                            effect_id=active_effect_id_signal
                                                            control_values=control_values
                                                            accent_rgb=accent_rgb
                                                        />
                                                    })}
                                                    <div
                                                        class="rounded-xl bg-surface-raised/80 border border-edge-subtle p-3 edge-glow"
                                                        style:border-top=move || format!("2px solid rgba({}, 0.2)", accent_rgb.get())
                                                    >
                                                        <div class="flex items-center gap-2 mb-3 pb-2 border-b border-edge-subtle/50">
                                                            <div
                                                                class="w-6 h-6 rounded-md flex items-center justify-center"
                                                                style=move || format!(
                                                                    "background: rgba({0}, 0.1); box-shadow: 0 0 8px rgba({0}, 0.08)",
                                                                    accent_rgb.get()
                                                                )
                                                            >
                                                                <span style=move || format!("color: rgba({}, 0.7)", accent_rgb.get())>
                                                                    <Icon icon=LuSettings2 width="13px" height="13px" />
                                                                </span>
                                                            </div>
                                                            <h3 class="text-[11px] font-semibold tracking-wide text-fg-secondary uppercase">
                                                                "Controls"
                                                            </h3>
                                                            <div class="flex-1" />
                                                            <button
                                                                class="p-1 rounded-md hover:bg-surface-hover/40 text-fg-tertiary/50 hover:text-fg-secondary transition-all duration-150"
                                                                title="Dock controls back"
                                                                on:click=move |_| {
                                                                    set_controls_detached.set(false);
                                                                    persist_to_storage("hc-fx-controls-detached", "false");
                                                                }
                                                            >
                                                                <Icon icon=LuLink width="11px" height="11px" />
                                                            </button>
                                                        </div>
                                                        <ControlPanel
                                                            controls=controls
                                                            control_values=control_values
                                                            accent_rgb=accent_rgb
                                                            on_change=on_control_change
                                                        />
                                                    </div>
                                                </div>
                                            </div>
                                        </aside>
                                    </div>
                                }
                            })}
                        </div>
                    }
                })
            }}
            </div>
        </div>
    }
}

#[derive(Clone, Copy)]
struct PoisonousThemePalette {
    bg: &'static str,
    colors: [&'static str; 3],
}

fn poisonous_theme_palette(theme: &str) -> Option<PoisonousThemePalette> {
    match theme {
        "Poison" => Some(PoisonousThemePalette {
            bg: "#130032",
            colors: ["#6000fc", "#b300ff", "#8a42ff"],
        }),
        "Blacklight" => Some(PoisonousThemePalette {
            bg: "#06050d",
            colors: ["#ff58c8", "#30e5ff", "#f4f24e"],
        }),
        "Radioactive" => Some(PoisonousThemePalette {
            bg: "#060b05",
            colors: ["#7bff00", "#00ff9d", "#f3ff52"],
        }),
        "Nightshade" => Some(PoisonousThemePalette {
            bg: "#0b0615",
            colors: ["#8d5cff", "#ff4fd1", "#56d8ff"],
        }),
        "Cotton Candy" => Some(PoisonousThemePalette {
            bg: "#110816",
            colors: ["#ff74c5", "#79ecff", "#ffe869"],
        }),
        _ => None,
    }
}

fn is_poisonous_color_control(control_name: &str) -> bool {
    matches!(control_name, "bgColor" | "color1" | "color2" | "color3")
}

fn control_text_value(value: &ControlValue) -> Option<&str> {
    match value {
        ControlValue::Text(text) | ControlValue::Enum(text) => Some(text.as_str()),
        _ => None,
    }
}

fn parse_hex_rgb(hex: &str) -> Option<(u8, u8, u8)> {
    let hex = hex.strip_prefix('#').unwrap_or(hex);
    if hex.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some((r, g, b))
}

fn hex_to_rgba_json(hex: &str) -> Option<serde_json::Value> {
    let (r, g, b) = parse_hex_rgb(hex)?;
    Some(serde_json::json!([
        f32::from(r) / 255.0,
        f32::from(g) / 255.0,
        f32::from(b) / 255.0,
        1.0
    ]))
}

fn expand_control_updates(
    active_effect_name: Option<&str>,
    current_values: &std::collections::HashMap<String, ControlValue>,
    control_name: &str,
    value: &serde_json::Value,
) -> Vec<(String, serde_json::Value)> {
    let mut updates = vec![(control_name.to_owned(), value.clone())];

    if active_effect_name != Some("Poisonous") {
        return updates;
    }

    if control_name == "theme"
        && let Some(theme_name) = value.as_str()
        && let Some(palette) = poisonous_theme_palette(theme_name)
    {
        for (name, hex) in [
            ("bgColor", palette.bg),
            ("color1", palette.colors[0]),
            ("color2", palette.colors[1]),
            ("color3", palette.colors[2]),
        ] {
            if let Some(color_value) = hex_to_rgba_json(hex) {
                updates.push((name.to_owned(), color_value));
            }
        }
    }

    if is_poisonous_color_control(control_name) {
        let active_theme = current_values
            .get("theme")
            .and_then(control_text_value)
            .unwrap_or("Poison");
        if active_theme != "Custom" {
            updates.push(("theme".to_owned(), serde_json::json!("Custom")));
        }
    }

    updates
}

fn json_to_control_value(
    control_name: &str,
    controls: &[ControlDefinition],
    value: &serde_json::Value,
) -> Option<ControlValue> {
    if let Some(v) = value.as_bool() {
        return Some(ControlValue::Boolean(v));
    }
    if let Some(v) = value.as_i64() {
        let int = i32::try_from(v).ok()?;
        return Some(ControlValue::Integer(int));
    }
    if let Some(v) = value.as_f64() {
        let float = parse_f32(v)?;
        return Some(ControlValue::Float(float));
    }
    if let Some(v) = value.as_str() {
        let (is_dropdown, is_color_picker) = controls
            .iter()
            .find(|def| def.control_id().eq_ignore_ascii_case(control_name))
            .map(|def| {
                (
                    matches!(def.control_type, ControlType::Dropdown),
                    matches!(def.control_type, ControlType::ColorPicker),
                )
            })
            .unwrap_or((false, false));
        if is_dropdown {
            return Some(ControlValue::Enum(v.to_owned()));
        }
        if is_color_picker && let Some(color_value) = hex_to_color_value(v) {
            return Some(color_value);
        }
        return Some(ControlValue::Text(v.to_owned()));
    }
    if let Some(array) = value.as_array()
        && array.len() == 4
    {
        let mut color = [0.0f32; 4];
        for (idx, component) in array.iter().enumerate() {
            let parsed = component.as_f64()?;
            color[idx] = parse_f32(parsed)?;
        }
        return Some(ControlValue::Color(color));
    }
    None
}

fn parse_f32(value: f64) -> Option<f32> {
    if !value.is_finite() || value < f64::from(f32::MIN) || value > f64::from(f32::MAX) {
        return None;
    }
    Some(value as f32)
}

fn hex_to_color_value(hex: &str) -> Option<ControlValue> {
    let (r, g, b) = parse_hex_rgb(hex)?;
    Some(ControlValue::Color([
        f32::from(r) / 255.0,
        f32::from(g) / 255.0,
        f32::from(b) / 255.0,
        1.0,
    ]))
}

fn toggle_body_resizing(active: bool) {
    if let Some(body) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.body())
    {
        if active {
            let _ = body.class_list().add_1("resizing");
        } else {
            let _ = body.class_list().remove_1("resizing");
        }
    }
}

/// Build the three drag callbacks (start, move, end) for a resizable panel.
fn drag_callbacks(
    width: ReadSignal<f64>,
    set_width: WriteSignal<f64>,
    min: f64,
    max: f64,
    storage_key: &'static str,
) -> (Callback<()>, Callback<f64>, Callback<()>) {
    let drag_start = StoredValue::new(0.0_f64);

    let on_start = Callback::new(move |()| {
        drag_start.set_value(width.get_untracked());
        toggle_body_resizing(true);
    });
    let on_drag = Callback::new(move |delta_x: f64| {
        let new_w = (drag_start.get_value() - delta_x).clamp(min, max);
        set_width.set(new_w);
    });
    let on_end = Callback::new(move |()| {
        toggle_body_resizing(false);
        persist_to_storage(storage_key, &width.get_untracked().to_string());
    });

    (on_start, on_drag, on_end)
}

fn persist_to_storage(key: &str, value: &str) {
    crate::storage::set(key, value);
}

/// Loading skeleton for the effects grid.
#[component]
fn LoadingSkeleton() -> impl IntoView {
    view! {
        <div class="grid grid-cols-[repeat(auto-fill,minmax(300px,1fr))] gap-4">
            {(0..12).map(|_| {
                view! {
                    <div class="rounded-xl border border-edge-subtle bg-surface-overlay/40 px-4 py-3 animate-pulse space-y-3">
                        <div class="flex justify-between">
                            <div class="h-4 w-28 bg-surface-overlay/40 rounded" />
                            <div class="h-4 w-14 bg-surface-overlay/40 rounded-full" />
                        </div>
                        <div class="space-y-1.5">
                            <div class="h-3 w-full bg-surface-overlay/20 rounded" />
                            <div class="h-3 w-3/4 bg-surface-overlay/20 rounded" />
                        </div>
                        <div class="flex gap-1.5">
                            <div class="h-4 w-14 bg-surface-overlay/20 rounded" />
                            <div class="h-4 w-12 bg-surface-overlay/20 rounded" />
                        </div>
                        <div class="flex justify-between pt-1 border-t border-edge-subtle">
                            <div class="h-2.5 w-16 bg-surface-overlay/20 rounded" />
                            <div class="h-2.5 w-12 bg-surface-overlay/20 rounded" />
                        </div>
                    </div>
                }
            }).collect_view()}
        </div>
    }
}
