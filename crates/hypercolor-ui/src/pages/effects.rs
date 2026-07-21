//! Effects browse page — grid of effect cards with filtering and detail panel.

use leptos::prelude::*;
use leptos_icons::Icon;
use leptos_use::use_debounce_fn;

use crate::api;
use crate::app::{EffectsContext, WsContext};
use crate::apply_target::ApplyTarget;
use crate::components::calibration_guide::CalibrationGuide;
use crate::components::control_panel::capture_group::CaptureSharedControls;
use crate::components::effect_card::EffectCard;
use crate::components::install_effect_panel::InstallEffectPanel;
use crate::components::page_header::{HeaderToolbar, HeaderTrailing, PageAccent, PageHeader};
use crate::components::page_search_bar::PageSearchBar;
use crate::components::input_access_banner::InputAccessBanner;
use crate::components::preview_cabinet::PreviewCabinet;
use crate::components::resize_handle::ResizeHandle;
use crate::components::section_label::{LabelSize, LabelTone, label_class};
use crate::components::silk_select::SilkSelect;
use crate::components::status_banner::{StatusBanner, StatusBannerTone};
use crate::icons::*;
use crate::optimistic_controls::{OptimisticControlSession, raw_control_updates_payload};
use crate::toasts;
use crate::zones::{ZoneEffectState, ZonesContext};
use hypercolor_types::effect::{ControlDefinition, ControlValue};
use hypercolor_types::scene::{SceneKind, SceneMutationMode, ZoneRole};

mod support;
mod zone_controls;

use support::{LoadingSkeleton, drag_callbacks, expand_control_updates, persist_to_storage};
use zone_controls::{ZoneControlSchemaCache, ZoneScopedControls};

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

/// The apply-target selector. Picks which zone a quick effect-apply
/// lands in; reads and writes `EffectsContext::apply_target`. The empty value
/// is the scene's default zone.
#[component]
fn ApplyTargetSelect(
    #[prop(into)] scene: Signal<Option<api::ActiveSceneResponse>>,
) -> impl IntoView {
    let fx = expect_context::<EffectsContext>();
    let zones_ctx = expect_context::<ZonesContext>();

    let options = Signal::derive(move || {
        let mut opts = vec![(String::new(), "Default zone".to_owned())];
        let mut custom_count = 0_usize;
        scene.with(|scene| {
            let Some(scene) = scene else {
                return;
            };
            for group in &scene.groups {
                match group.role {
                    ZoneRole::Display => {}
                    // The Primary role is the empty-value default; a renamed
                    // default zone relabels that option in place.
                    ZoneRole::Primary => {
                        if group.name != "Primary"
                            && let Some(first) = opts.first_mut()
                        {
                            first.1 = group.name.clone();
                        }
                    }
                    ZoneRole::Custom => {
                        custom_count += 1;
                        opts.push((group.id.to_string(), group.name.clone()));
                    }
                }
            }
        });
        if custom_count > 0 {
            opts.push((
                crate::apply_target::ALL_ZONES_VALUE.to_owned(),
                "All zones".to_owned(),
            ));
        }
        opts
    });
    let value = Signal::derive(move || fx.apply_target.get().select_value());
    let on_change = Callback::new(move |val: String| {
        let target = ApplyTarget::from_select_value(val);
        // Mirror the picked zone into the shared focused zone so the
        // preset panel, sidebar player, and the controls panel below all
        // follow the same visible choice (Primary/All zones = primary).
        zones_ctx
            .focused_zone
            .set(target.zone_id().map(ToOwned::to_owned));
        fx.apply_target.set(target);
    });

    view! {
        <div class="flex shrink-0 items-center gap-1.5">
            <span class=label_class(LabelSize::Micro, LabelTone::Default)>"Apply to"</span>
            <div class="min-w-[130px]">
                <SilkSelect
                    value=value
                    options=options
                    on_change=on_change
                    class="border border-edge-subtle/70 bg-surface-overlay/40 px-2.5 py-1 text-[12px] text-fg-primary"
                />
            </div>
        </div>
    }
}

/// Effects browse page with compact grid, search, category filtering, and live preview.
#[component]
pub fn EffectsPage() -> impl IntoView {
    let ws = expect_context::<WsContext>();
    let fx = expect_context::<EffectsContext>();
    let zones_ctx = expect_context::<ZonesContext>();

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
        620.0,
        MIN_DETAIL_WIDTH,
        MAX_DETAIL_WIDTH,
    ));
    let (controls_detached, set_controls_detached) =
        signal(crate::storage::get("hc-fx-controls-detached").as_deref() == Some("true"));
    let (controls_width, set_controls_width) = signal(crate::storage::get_clamped(
        "hc-fx-controls-width",
        320.0,
        MIN_CONTROLS_WIDTH,
        MAX_CONTROLS_WIDTH,
    ));
    let control_session = OptimisticControlSession::new();
    let control_request_epoch = StoredValue::new(0_u64);
    let preferences_store = use_context::<crate::preferences::PreferencesStore>();
    let flush_control_updates = use_debounce_fn(
        move || {
            let updates = control_session.take_pending();
            if updates.is_empty() {
                return;
            }

            let controls_json = raw_control_updates_payload(updates);
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
                        let has_pending_updates = control_session.has_pending();
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
                        let has_pending_updates = control_session.has_pending();
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
    // Control schemas for non-primary zone tabs, cached per effect id.
    // Page-owned so docking/undocking the controls column keeps it warm.
    let zone_schema_cache: ZoneControlSchemaCache =
        StoredValue::new(std::collections::HashMap::new());
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
    let screen_reactive_active = Memo::new(move |_| {
        active_effect_summary.get().is_some_and(|effect| {
            effect
                .tags
                .iter()
                .any(|tag| tag.eq_ignore_ascii_case("screen-reactive"))
        })
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

    // Per-zone active state: a card lights up when its effect runs in ANY
    // LED zone of the active scene. The singular `active_effect_id`
    // (the primary-zone mirror) stays in the set so single-zone scenes —
    // including the ephemeral default before the scene resource resolves —
    // keep exactly their old behavior.
    let active_effect_ids = Memo::new(move |_| {
        let mut ids = fx.zone_effects.with(|zones| {
            zones
                .iter()
                .filter_map(|state| state.effect_id.clone())
                .collect::<std::collections::HashSet<_>>()
        });
        if let Some(primary) = fx.active_effect_id.get() {
            ids.insert(primary);
        }
        ids
    });

    // The detail/preview pane serves whichever zones are rendering: in a
    // multi-zone scene it opens when ANY zone is active, not just the
    // primary (a zone-2-only scene used to hide the pane entirely).
    let has_active = Memo::new(move |_| {
        fx.active_effect_id.get().is_some()
            || (zones_ctx.multi_zone.get()
                && fx
                    .zone_effects
                    .with(|zones| zones.iter().any(ZoneEffectState::is_active)))
    });
    // Only snapshot-locked scenes get a banner: the daemon rejects their
    // mutations outright, so the unblock path has to be visible. A merely
    // active named scene is normal operation, not a warning.
    let snapshot_locked_warning = Memo::new(move |_| {
        (fx.active_scene_kind.get() == Some(SceneKind::Named)
            && fx.active_scene_mutation_mode.get() == Some(SceneMutationMode::Snapshot))
        .then(|| {
            fx.active_scene_name
                .get()
                .unwrap_or_else(|| "Active scene".to_owned())
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

    // Apply-target selector (§5.3). The active scene's LED zones drive an
    // explicit target picker that appears only once a scene has more than
    // one zone — a single-zone scene keeps the unchanged "apply effect"
    // behavior with no extra control. The shared scene resource keeps the
    // zone options fresh across external scene/zone changes.
    let apply_target_scene: Signal<Option<api::ActiveSceneResponse>> =
        zones_ctx.active_scene.into();
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

        control_session.apply_raw_updates_to(
            fx.set_active_control_values,
            &controls_snapshot,
            &updates,
        );
        control_session.queue_raw_updates(&updates);

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
                tagline="Browse the library, tune the glow"
                accent=PageAccent::Purple
            >
                <HeaderTrailing slot>
                    <ApplyTargetSelect scene=apply_target_scene />
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
                                           py-1.5 animate-enter-fade animate-glow-reveal scrollbar-dropdown"
                                >
                                    // ── Category section ──
                                    <div class="px-3 pt-1 pb-1.5">
                                        <div class=format!("{} mb-1.5", label_class(LabelSize::Small, LabelTone::Subtle))>"Category"</div>
                                        <div class="flex gap-1 flex-wrap">
                                            {filter_chips(CATEGORY_CHIPS, category_filter, set_category_filter)}
                                        </div>
                                    </div>

                                    <div class="h-px bg-edge-subtle/30 mx-2 my-1" />

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
                                            <div class="h-px bg-edge-subtle/30 mx-2 my-1" />
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
                                        <div class="h-px bg-edge-subtle/30 mx-2 my-1" />
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

            {move || snapshot_locked_warning.get().map(|scene_name| view! {
                <div class="px-6 pt-4">
                    <StatusBanner
                        tone=StatusBannerTone::Warning
                        title="Snapshot Scene Locked"
                        subject=scene_name
                        detail=" is snapshot-locked. Return to Default before applying an effect or changing its controls."
                    >
                        <button
                            class="shrink-0 rounded-lg border border-status-warning/28 px-3 py-1.5 text-[11px] font-medium text-status-warning/92 transition-all duration-200 hover:bg-status-warning/8 disabled:cursor-wait disabled:opacity-60"
                            disabled=move || returning_to_default.get()
                            on:click=move |_| on_return_to_default.run(())
                        >
                            {move || if returning_to_default.get() {
                                "Returning..."
                            } else {
                                "Return to Default"
                            }}
                        </button>
                    </StatusBanner>
                </div>
            })}
            {move || degraded_effect.get().map(|(effect_name, detail)| view! {
                <div class="px-6 pt-3">
                    <StatusBanner
                        tone=StatusBannerTone::Error
                        title="Degraded Effect"
                        subject=effect_name
                        detail=format!(" is degraded. {detail}")
                    />
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
                                            // Enumerated so each card gets its grid index for
                                            // the entrance stagger (EffectCard caps the delay
                                            // tier internally).
                                            each=move || filtered_effects.get().into_iter().enumerate()
                                            key=|(_, effect)| effect.id.clone()
                                            children=move |(index, effect)| {
                                                let effect_id = effect.id.clone();
                                                let fav_effect_id = effect.id.clone();
                                                let badge_effect_id = effect.id.clone();
                                                let is_active = Signal::derive(move || {
                                                    active_effect_ids.with(|ids| ids.contains(effect_id.as_str()))
                                                });
                                                let is_favorite = Signal::derive(move || {
                                                    fx.favorite_ids.get().contains(&fav_effect_id)
                                                });
                                                // Zone badge data only in multi-zone scenes;
                                                // single-zone cards render exactly as before.
                                                let active_zone_names = Signal::derive(move || {
                                                    if !zones_ctx.multi_zone.get() {
                                                        return Vec::new();
                                                    }
                                                    fx.zone_effects.with(|zones| {
                                                        zones
                                                            .iter()
                                                            .filter(|state| {
                                                                state.effect_id.as_deref()
                                                                    == Some(badge_effect_id.as_str())
                                                            })
                                                            .map(|state| state.zone.name.clone())
                                                            .collect()
                                                    })
                                                });
                                                view! {
                                                    <EffectCard
                                                        effect=effect
                                                        is_active=is_active
                                                        is_favorite=is_favorite
                                                        active_zone_names=active_zone_names
                                                        on_apply=on_apply
                                                        on_toggle_favorite=on_toggle_favorite
                                                        index=index
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
                                class="shrink-0 flex flex-col min-h-0 animate-enter-right"
                                style=move || format!("width: {}px", detail_width.get())
                            >
                                // Input-access remediation — renders only when the active
                                // effect is interactive and host input is off or blocked.
                                <div class="shrink-0">
                                    <InputAccessBanner />
                                </div>

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
                                                            aria-label="Float controls into separate panel"
                                                            on:click=move |_| {
                                                                set_controls_detached.set(true);
                                                                persist_to_storage("hc-fx-controls-detached", "true");
                                                            }
                                                        >
                                                            <Icon icon=LuUnlink width="11px" height="11px" />
                                                        </button>
                                                    </div>
                                                    <ZoneScopedControls
                                                        controls=controls
                                                        control_values=control_values
                                                        accent_rgb=accent_rgb
                                                        on_control_change=on_control_change
                                                        schema_cache=zone_schema_cache
                                                    />
                                                    <CaptureSharedControls
                                                        visible=screen_reactive_active
                                                        accent_rgb=accent_rgb
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
                                            class="shrink-0 flex flex-col min-h-0 animate-enter-right"
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
                                                                aria-label="Dock controls back"
                                                                on:click=move |_| {
                                                                    set_controls_detached.set(false);
                                                                    persist_to_storage("hc-fx-controls-detached", "false");
                                                                }
                                                            >
                                                                <Icon icon=LuLink width="11px" height="11px" />
                                                            </button>
                                                        </div>
                                                        <ZoneScopedControls
                                                            controls=controls
                                                            control_values=control_values
                                                            accent_rgb=accent_rgb
                                                            on_control_change=on_control_change
                                                            schema_cache=zone_schema_cache
                                                        />
                                                        <CaptureSharedControls
                                                            visible=screen_reactive_active
                                                            accent_rgb=accent_rgb
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
