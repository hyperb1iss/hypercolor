//! Effects browse page — grid of effect cards with filtering and detail panel.

use leptos::prelude::*;
use leptos_icons::Icon;
use leptos_use::use_debounce_fn;
use wasm_bindgen::JsCast;

use crate::api;
use crate::app::{EffectsContext, WsContext};
use crate::components::canvas_preview::CanvasPreview;
use crate::components::control_panel::ControlPanel;
use crate::components::effect_card::EffectCard;
use crate::components::preset_panel::PresetToolbar;
use crate::components::resize_handle::ResizeHandle;
use crate::icons::*;
use hypercolor_types::effect::{ControlDefinition, ControlType, ControlValue};

use crate::style_utils::category_accent_rgb;

const EFFECTS_PREVIEW_FPS_CAP: u32 = 24;
const MIN_DETAIL_WIDTH: f64 = 260.0;
const MAX_DETAIL_WIDTH: f64 = 1200.0;
const MIN_CONTROLS_WIDTH: f64 = 220.0;
const MAX_CONTROLS_WIDTH: f64 = 800.0;

/// Category filter options.
const CATEGORIES: &[&str] = &[
    "all",
    "ambient",
    "gaming",
    "reactive",
    "generative",
    "interactive",
    "productivity",
    "utility",
];

/// Effects browse page with compact grid, search, category filtering, and live preview.
#[component]
pub fn EffectsPage() -> impl IntoView {
    let ws = expect_context::<WsContext>();
    let fx = expect_context::<EffectsContext>();

    Effect::new(move |_| {
        ws.set_preview_cap.set(EFFECTS_PREVIEW_FPS_CAP);
    });
    on_cleanup(move || ws.set_preview_cap.set(crate::ws::DEFAULT_PREVIEW_FPS_CAP));

    // Restore persisted filter state from localStorage
    let storage = web_sys::window().and_then(|w| w.local_storage().ok().flatten());
    let stored = |key: &str| -> Option<String> { storage.as_ref()?.get_item(key).ok().flatten() };

    let (search, set_search) = signal(String::new());
    let (category_filter, set_category_filter) =
        signal(stored("hc-fx-category").unwrap_or_else(|| "all".to_string()));
    let (selected_authors, set_selected_authors) = signal({
        stored("hc-fx-authors")
            .map(|s| {
                s.split(',')
                    .filter(|a| !a.is_empty())
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_else(std::collections::BTreeSet::<String>::new)
    });
    let (favorites_only, set_favorites_only) =
        signal(stored("hc-fx-favorites").as_deref() == Some("true"));
    let (audio_reactive_only, set_audio_reactive_only) =
        signal(stored("hc-fx-audio").as_deref() == Some("true"));

    // Panel layout state (persisted to localStorage)
    let (detail_width, set_detail_width) = signal(
        stored("hc-fx-detail-width")
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(380.0)
            .clamp(MIN_DETAIL_WIDTH, MAX_DETAIL_WIDTH),
    );
    let (controls_detached, set_controls_detached) =
        signal(stored("hc-fx-controls-detached").as_deref() != Some("false"));
    let (controls_width, set_controls_width) = signal(
        stored("hc-fx-controls-width")
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(320.0)
            .clamp(MIN_CONTROLS_WIDTH, MAX_CONTROLS_WIDTH),
    );
    let detail_drag_start = StoredValue::new(0.0_f64);
    let controls_drag_start = StoredValue::new(0.0_f64);

    let pending_control_updates =
        StoredValue::new(std::collections::HashMap::<String, serde_json::Value>::new());
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
            leptos::task::spawn_local(async move {
                let _ = api::update_controls(&controls_json).await;
            });
        },
        75.0,
    );

    // Persist filter changes to localStorage
    Effect::new(move |_| {
        let Some(storage) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) else {
            return;
        };
        let _ = storage.set_item("hc-fx-category", &category_filter.get());
        let _ = storage.set_item("hc-fx-favorites", &favorites_only.get().to_string());
        let _ = storage.set_item("hc-fx-audio", &audio_reactive_only.get().to_string());
        let authors_str: String = selected_authors
            .get()
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join(",");
        let _ = storage.set_item("hc-fx-authors", &authors_str);
    });

    // Derive unique sorted author list from loaded effects
    let authors = Memo::new(move |_| {
        let mut seen = std::collections::BTreeSet::new();
        fx.effects_index.with(|effects| {
            for entry in effects {
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

    let has_active = Memo::new(move |_| fx.active_effect_id.get().is_some());

    // Derive the full active effect metadata from the index
    let active_effect_meta = Memo::new(move |_| {
        let id = fx.active_effect_id.get()?;
        fx.effects_index.with(|effects| {
            effects
                .iter()
                .find(|e| e.effect.id == id)
                .map(|e| e.effect.clone())
        })
    });

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

    // Apply effect handler — delegates to shared context
    let on_apply = Callback::new(move |id: String| {
        fx.apply_effect(id);
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

    // Detail panel resize callbacks
    let on_detail_drag_start = Callback::new(move |()| {
        detail_drag_start.set_value(detail_width.get_untracked());
        toggle_body_resizing(true);
    });
    let on_detail_drag = Callback::new(move |delta_x: f64| {
        let new_w =
            (detail_drag_start.get_value() - delta_x).clamp(MIN_DETAIL_WIDTH, MAX_DETAIL_WIDTH);
        set_detail_width.set(new_w);
    });
    let on_detail_drag_end = Callback::new(move |()| {
        toggle_body_resizing(false);
        persist_to_storage(
            "hc-fx-detail-width",
            &detail_width.get_untracked().to_string(),
        );
    });

    // Controls panel resize callbacks (when detached)
    let on_controls_drag_start = Callback::new(move |()| {
        controls_drag_start.set_value(controls_width.get_untracked());
        toggle_body_resizing(true);
    });
    let on_controls_drag = Callback::new(move |delta_x: f64| {
        let new_w = (controls_drag_start.get_value() - delta_x)
            .clamp(MIN_CONTROLS_WIDTH, MAX_CONTROLS_WIDTH);
        set_controls_width.set(new_w);
    });
    let on_controls_drag_end = Callback::new(move |()| {
        toggle_body_resizing(false);
        persist_to_storage(
            "hc-fx-controls-width",
            &controls_width.get_untracked().to_string(),
        );
    });

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
        <div class="flex flex-col h-full -m-6 animate-fade-in">
            // Fixed header — title + search + filters on one line
            <div class="shrink-0 px-6 pt-5 pb-3 bg-surface-base z-10">
                <div class="flex items-center gap-3">
                    <h1 class="text-lg font-medium text-fg-primary shrink-0">"Effects"</h1>

                    // Search bar — fills available space
                    <div class="relative flex-1 min-w-0">
                        <span class="absolute left-3 top-1/2 -translate-y-1/2 pointer-events-none text-fg-tertiary">
                            <Icon icon=LuSearch width="14px" height="14px" />
                        </span>
                        <input
                            type="text"
                            placeholder="Search effects..."
                            class="w-full bg-surface-overlay/60 border border-edge-subtle rounded-lg pl-9 pr-10 py-1.5 text-sm text-fg-primary
                                   placeholder-fg-tertiary focus:outline-none focus:border-accent-muted
                                   search-glow glow-ring transition-all duration-300"
                            prop:value=move || search.get()
                            on:input=move |ev| {
                                let target = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                                if let Some(el) = target {
                                    set_search.set(el.value());
                                }
                            }
                        />
                        <kbd class="absolute right-3 top-1/2 -translate-y-1/2 text-[9px] font-mono text-fg-tertiary bg-surface-overlay/30 px-1.5 py-0.5 rounded border border-edge-subtle">"/"</kbd>
                    </div>

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
                                    class="absolute top-full right-0 mt-1 z-30 w-[260px] max-h-[400px] overflow-y-auto
                                           rounded-xl border border-edge-subtle bg-surface-overlay dropdown-glow
                                           py-1.5 animate-fade-in animate-glow-reveal scrollbar-none"
                                >
                                    // ── Category section ──
                                    <div class="px-3 pt-1 pb-1.5">
                                        <div class="text-[10px] font-medium uppercase tracking-wider text-fg-tertiary/50 mb-1.5">"Category"</div>
                                        <div class="flex gap-1 flex-wrap">
                                            {CATEGORIES.iter().map(|cat| {
                                                let cat = cat.to_string();
                                                let cat_clone = cat.clone();
                                                let rgb = if cat == "all" { "225, 53, 255" } else { category_accent_rgb(&cat) }.to_string();
                                                let is_active = {
                                                    let cat = cat.clone();
                                                    Memo::new(move |_| category_filter.get() == cat)
                                                };
                                                let active_style = format!(
                                                    "background: rgba({rgb}, 0.15); color: rgb({rgb}); border-color: rgba({rgb}, 0.3); box-shadow: 0 0 8px rgba({rgb}, 0.15)"
                                                );
                                                let inactive_style = format!(
                                                    "color: rgba({rgb}, 0.5); border-color: rgba({rgb}, 0.08); background: transparent"
                                                );
                                                view! {
                                                    <button
                                                        class="px-2 py-0.5 rounded-full text-[10px] font-medium capitalize border transition-all"
                                                        style=move || if is_active.get() { active_style.clone() } else { inactive_style.clone() }
                                                        on:click=move |_| set_category_filter.set(cat_clone.clone())
                                                    >
                                                        {cat.clone()}
                                                    </button>
                                                }
                                            }).collect_view()}
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
                                                <div class="text-[10px] font-medium uppercase tracking-wider text-fg-tertiary/50 mb-1">"Author"</div>
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
                </div>
            </div>

            // Resizable multi-column layout — grid | handle | preview [| handle | controls]
            <div class="flex-1 flex min-h-0 px-6 pb-6">
                // Effect grid — independently scrollable left column
                <div class="flex-1 min-w-0 overflow-y-auto" style="min-width: 120px">
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
                                    "grid grid-cols-[repeat(auto-fill,minmax(200px,1fr))] gap-3"
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

                // Detail panel(s) — right side, visible when an effect is selected
                //
                // IMPORTANT: Only read active_effect_id here so that accent color
                // changes don't rebuild the DOM (which destroys CanvasPreview and
                // causes a burst of re-paints). All dynamic styles use reactive bindings.
                {move || {
                    fx.active_effect_id.get().map(|_| {
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
                                    // Info card + live preview
                                    <div class="shrink-0 space-y-2 pb-2">
                                        <div
                                            class="rounded-lg bg-surface-overlay/40 border border-edge-subtle px-3 py-2.5 space-y-2"
                                            style:border-top=move || format!("2px solid rgba({}, 0.2)", accent_rgb.get())
                                        >
                                            <div class="flex items-center gap-2 min-w-0">
                                                <div
                                                    class="w-2 h-2 rounded-full dot-alive shrink-0"
                                                    style:background=move || format!("rgb({})", accent_rgb.get())
                                                    style:box-shadow=move || format!("0 0 8px rgba({}, 0.6)", accent_rgb.get())
                                                />
                                                <span class="text-[13px] font-medium text-fg-primary truncate">
                                                    {move || fx.active_effect_name.get().unwrap_or_default()}
                                                </span>
                                                {move || {
                                                    active_effect_meta.get().map(|meta| {
                                                        view! {
                                                            <span class="ml-auto text-[10px] text-fg-tertiary/50 shrink-0 truncate max-w-[120px]">
                                                                {meta.author.clone()}
                                                            </span>
                                                        }
                                                    })
                                                }}
                                            </div>
                                            {move || {
                                                active_effect_meta.get().and_then(|meta| {
                                                    (!meta.description.is_empty()).then(|| view! {
                                                        <p class="text-[10px] text-fg-tertiary/40 truncate pl-4 -mt-1">
                                                            {meta.description.clone()}
                                                        </p>
                                                    })
                                                })
                                            }}
                                            <div class="h-px bg-edge-subtle/50" />
                                            <PresetToolbar
                                                effect_id=Signal::derive(move || fx.active_effect_id.get())
                                                control_values=control_values
                                                accent_rgb=accent_rgb
                                                on_preset_applied=Callback::new(move |()| fx.refresh_active_effect())
                                                active_preset_id_signal=Signal::derive(move || fx.active_preset_id.get())
                                            />
                                        </div>

                                        <div class="rounded-lg bg-black overflow-hidden edge-glow">
                                            <CanvasPreview
                                                frame=ws.canvas_frame
                                                fps=ws.preview_fps
                                                show_fps=true
                                                fps_target=ws.preview_target_fps
                                            />
                                        </div>
                                    </div>

                                    // Controls (docked mode — inside preview panel)
                                    {move || (!controls_detached.get()).then(|| {
                                        view! {
                                            <div
                                                class="flex-1 min-h-0 overflow-y-auto"
                                                style="overscroll-behavior: contain"
                                            >
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

fn hex_to_rgba_json(hex: &str) -> Option<serde_json::Value> {
    let hex = hex.strip_prefix('#').unwrap_or(hex);
    if hex.len() != 6 {
        return None;
    }

    let red = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let green = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let blue = u8::from_str_radix(&hex[4..6], 16).ok()?;

    Some(serde_json::json!([
        f32::from(red) / 255.0,
        f32::from(green) / 255.0,
        f32::from(blue) / 255.0,
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
    let hex = hex.strip_prefix('#').unwrap_or(hex);
    if hex.len() != 6 {
        return None;
    }

    let red = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let green = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let blue = u8::from_str_radix(&hex[4..6], 16).ok()?;

    Some(ControlValue::Color([
        f32::from(red) / 255.0,
        f32::from(green) / 255.0,
        f32::from(blue) / 255.0,
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

fn persist_to_storage(key: &str, value: &str) {
    if let Some(storage) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
        let _ = storage.set_item(key, value);
    }
}

/// Loading skeleton for the effects grid.
#[component]
fn LoadingSkeleton() -> impl IntoView {
    view! {
        <div class="grid grid-cols-[repeat(auto-fill,minmax(300px,1fr))] gap-4">
            {(0..12).map(|_| {
                view! {
                    <div class="rounded-2xl border border-edge-subtle bg-surface-overlay/40 px-4 py-4 animate-pulse space-y-3">
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
