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
use crate::icons::*;
use hypercolor_types::effect::{ControlDefinition, ControlType, ControlValue};

const EFFECTS_PREVIEW_FPS_CAP: u32 = 24;

/// Category -> accent RGB string for inline styles.
fn category_accent_rgb(category: &str) -> &'static str {
    match category {
        "ambient" => "128, 255, 234",
        "audio" => "255, 106, 193",
        "gaming" => "225, 53, 255",
        "reactive" => "241, 250, 140",
        "generative" => "80, 250, 123",
        "interactive" => "130, 170, 255",
        "productivity" => "255, 153, 255",
        "utility" => "139, 133, 160",
        _ => "225, 53, 255",
    }
}

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
    let (author_dropdown_open, set_author_dropdown_open) = signal(false);
    let (favorites_only, set_favorites_only) =
        signal(stored("hc-fx-favorites").as_deref() == Some("true"));
    let (audio_reactive_only, set_audio_reactive_only) =
        signal(stored("hc-fx-audio").as_deref() == Some("true"));
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

    let canvas_frame = Signal::derive(move || ws.canvas_frame.get());
    let preview_fps = Signal::derive(move || ws.preview_fps.get());
    let controls = Signal::derive(move || fx.active_controls.get());
    let control_values = Signal::derive(move || fx.active_control_values.get());
    let accent_rgb =
        Signal::derive(move || category_accent_rgb(&fx.active_effect_category.get()).to_string());

    let has_active = Memo::new(move |_| fx.active_effect_id.get().is_some());

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

    let effect_count = Memo::new(move |_| filtered_effects.get().len());

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

    view! {
        <div class="flex flex-col h-full -m-6 animate-fade-in">
            // Fixed header — title + search + categories + capability filters
            <div class="shrink-0 px-6 pt-6 pb-4 space-y-4 bg-surface-base z-10">
                // Title row
                <div class="flex items-baseline justify-between">
                    <h1 class="text-lg font-medium text-fg-primary">"Effects"</h1>
                    <span class="text-[11px] font-mono text-fg-tertiary tabular-nums">
                        {move || effect_count.get()} " effects"
                    </span>
                </div>

                // Search bar
                <div class="relative">
                    <span class="absolute left-3.5 top-1/2 -translate-y-1/2 pointer-events-none text-fg-tertiary">
                        <Icon icon=LuSearch width="14px" height="14px" />
                    </span>
                    <input
                        type="text"
                        placeholder="Search effects..."
                        class="w-full bg-surface-overlay/60 border border-edge-subtle rounded-lg pl-9 pr-10 py-2 text-sm text-fg-primary
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

                // Category + capability filter bar
                <div class="flex items-center gap-3 flex-wrap">
                    // Category chips
                    <div class="flex gap-1.5 flex-wrap">
                        {CATEGORIES.iter().map(|cat| {
                            let cat = cat.to_string();
                            let cat_clone = cat.clone();
                            let rgb = if cat == "all" { "225, 53, 255" } else { category_accent_rgb(&cat) }.to_string();
                            let is_active = {
                                let cat = cat.clone();
                                Memo::new(move |_| category_filter.get() == cat)
                            };
                            let active_style = format!(
                                "background: rgba({rgb}, 0.15); color: rgb({rgb}); border-color: rgba({rgb}, 0.3); box-shadow: 0 0 12px rgba({rgb}, 0.2), inset 0 0 8px rgba({rgb}, 0.05)"
                            );
                            let inactive_style = format!(
                                "color: rgba({rgb}, 0.6); border-color: rgba({rgb}, 0.1); background: rgba({rgb}, 0.03)"
                            );
                            view! {
                                <button
                                    class="px-2.5 py-1 rounded-full text-xs font-medium capitalize border chip-interactive"
                                    style=move || if is_active.get() { active_style.clone() } else { inactive_style.clone() }
                                    on:click=move |_| set_category_filter.set(cat_clone.clone())
                                >
                                    {cat.clone()}
                                </button>
                            }
                        }).collect_view()}
                    </div>

                    // Divider
                    <div class="w-px h-5 bg-border-subtle" />

                    // Favorites toggle chip
                    <button
                        class="inline-flex items-center gap-1.5 px-2.5 py-1 rounded-full text-xs font-medium border chip-interactive transition-all duration-200"
                        style=move || if favorites_only.get() {
                            "background: rgba(255, 106, 193, 0.15); color: rgb(255, 106, 193); border-color: rgba(255, 106, 193, 0.3); \
                             box-shadow: 0 0 12px rgba(255, 106, 193, 0.2), inset 0 0 8px rgba(255, 106, 193, 0.05)"
                        } else {
                            "color: rgba(255, 106, 193, 0.5); border-color: rgba(255, 106, 193, 0.1); background: rgba(255, 106, 193, 0.03)"
                        }
                        on:click=move |_| set_favorites_only.update(|v| *v = !*v)
                    >
                        {move || {
                            let fav_style = if favorites_only.get() { "fill: currentColor" } else { "" };
                            view! { <Icon icon=LuHeart width="11px" height="11px" style=fav_style /> }
                        }}
                        "Favorites"
                        {move || {
                            let count = favorites_count.get();
                            (count > 0).then(|| view! {
                                <span class="text-[9px] font-mono opacity-70">{count}</span>
                            })
                        }}
                    </button>

                    // Audio reactive filter chip
                    <button
                        class="inline-flex items-center gap-1.5 px-2.5 py-1 rounded-full text-xs font-medium border chip-interactive transition-all duration-200"
                        style=move || if audio_reactive_only.get() {
                            "background: rgba(255, 106, 193, 0.15); color: rgb(255, 106, 193); border-color: rgba(255, 106, 193, 0.3); \
                             box-shadow: 0 0 12px rgba(255, 106, 193, 0.2), inset 0 0 8px rgba(255, 106, 193, 0.05)"
                        } else {
                            "color: rgba(139, 133, 160, 0.5); border-color: rgba(139, 133, 160, 0.1); background: rgba(139, 133, 160, 0.03)"
                        }
                        on:click=move |_| set_audio_reactive_only.update(|v| *v = !*v)
                    >
                        <Icon icon=LuAudioLines width="11px" height="11px" />
                        "Audio"
                    </button>

                    // Author multiselect dropdown
                    {move || {
                        let author_list = authors.get();
                        (author_list.len() > 1).then(move || {
                            let sel = selected_authors.get();
                            let count = sel.len();
                            let label = if count == 0 {
                                "All authors".to_string()
                            } else if count == 1 {
                                sel.iter().next().unwrap_or(&String::new()).clone()
                            } else {
                                format!("{count} authors")
                            };

                            view! {
                                <div class="relative">
                                    // Trigger button
                                    <button
                                        class="flex items-center gap-2 px-3 py-1.5 rounded-lg text-xs font-medium border transition-all duration-200"
                                        style=move || if selected_authors.get().is_empty() {
                                            "color: rgba(255, 106, 193, 0.6); border-color: rgba(255, 106, 193, 0.1); background: rgba(255, 106, 193, 0.03)"
                                        } else {
                                            "background: rgba(255, 106, 193, 0.12); color: rgb(255, 106, 193); border-color: rgba(255, 106, 193, 0.25); box-shadow: 0 0 10px rgba(255, 106, 193, 0.15)"
                                        }
                                        on:click=move |_| set_author_dropdown_open.update(|v| *v = !*v)
                                    >
                                        <Icon icon=LuUser width="12px" height="12px" />
                                        <span>{label.clone()}</span>
                                        <span
                                            class="w-3 h-3 flex items-center justify-center transition-transform duration-200"
                                            class:rotate-180=move || author_dropdown_open.get()
                                        >
                                            <Icon icon=LuChevronDown width="12px" height="12px" />
                                        </span>
                                    </button>

                                    // Dropdown panel
                                    {move || author_dropdown_open.get().then(|| {
                                        let list = authors.get();
                                        view! {
                                            // Invisible backdrop to close on outside click
                                            <div
                                                class="fixed inset-0 z-20"
                                                on:click=move |_| set_author_dropdown_open.set(false)
                                            />
                                            <div
                                                class="absolute top-full left-0 mt-1 z-30 min-w-[200px] max-h-[280px] overflow-y-auto
                                                       rounded-xl border border-edge-subtle bg-surface-overlay dropdown-glow
                                                       py-1 animate-fade-in animate-glow-reveal scrollbar-none"
                                            >
                                                // Clear all option
                                                <button
                                                    class="w-full text-left px-3 py-1.5 text-xs text-fg-tertiary hover:bg-surface-hover/40 transition-colors"
                                                    on:click=move |_| {
                                                        set_selected_authors.set(std::collections::BTreeSet::new());
                                                        set_author_dropdown_open.set(false);
                                                    }
                                                >
                                                    "All authors"
                                                </button>
                                                <div class="h-px bg-border-subtle mx-2 my-0.5" />
                                                // Author checkboxes
                                                {list.into_iter().map(|author| {
                                                    let author_toggle = author.clone();
                                                    let author_check = author.clone();
                                                    let author_check2 = author.clone();
                                                    let author_label = author.clone();
                                                    view! {
                                                        <button
                                                            class="w-full text-left px-3 py-1.5 flex items-center gap-2.5 text-xs hover:bg-surface-hover/40 transition-colors group"
                                                            on:click=move |_| {
                                                                let a = author_toggle.clone();
                                                                set_selected_authors.update(move |set| {
                                                                    if !set.remove(&a) {
                                                                        set.insert(a);
                                                                    }
                                                                });
                                                            }
                                                        >
                                                            // Checkbox indicator
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
                                </div>
                            }
                        })
                    }}
                </div>
            </div>

            // Two-column layout — each side scrolls independently
            <div class="flex-1 flex gap-5 min-h-0 px-6 pb-6">
                // Effect grid — independently scrollable left column
                <div class="flex-1 min-w-0 overflow-y-auto pr-1">
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
                                    "grid grid-cols-[repeat(auto-fill,minmax(260px,1fr))] gap-3"
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

                // Detail panel — independently scrollable right column
                //
                // IMPORTANT: Only read active_effect_id here so that accent color
                // changes don't rebuild the DOM (which destroys CanvasPreview and
                // causes a burst of re-paints). All dynamic styles use reactive bindings.
                {move || {
                    fx.active_effect_id.get().map(|_| {
                        view! {
                            <aside
                                class="w-[380px] shrink-0 overflow-y-auto space-y-2 pb-4 animate-slide-in-right"
                                style="overscroll-behavior: contain"
                            >
                                // Active effect name with category-colored dot + preset toolbar inline
                                <div class="flex items-center gap-2.5 px-1">
                                    <div
                                        class="w-2 h-2 rounded-full dot-alive shrink-0"
                                        style:background=move || format!("rgb({})", accent_rgb.get())
                                        style:box-shadow=move || format!("0 0 8px rgba({}, 0.6)", accent_rgb.get())
                                    />
                                    <span class="text-sm font-medium text-fg-primary truncate">
                                        {move || fx.active_effect_name.get().unwrap_or_default()}
                                    </span>
                                </div>

                                // Preset toolbar — compact, no extra border wrapper
                                <PresetToolbar
                                    effect_id=Signal::derive(move || fx.active_effect_id.get())
                                    control_values=control_values
                                    accent_rgb=accent_rgb
                                    on_preset_applied=Callback::new(move |()| fx.refresh_active_effect())
                                    active_preset_id_signal=Signal::derive(move || fx.active_preset_id.get())
                                />

                                // Live preview
                                <div class="rounded-lg bg-black overflow-hidden edge-glow">
                                    <CanvasPreview
                                        frame=canvas_frame
                                        fps=preview_fps
                                        show_fps=true
                                        fps_target=ws.preview_target_fps
                                    />
                                </div>

                                // Controls panel — tighter padding
                                <div
                                    class="rounded-lg bg-surface-raised border border-edge-subtle p-3 edge-glow"
                                    style:border-top=move || format!("2px solid rgba({}, 0.15)", accent_rgb.get())
                                >
                                    <div class="flex items-center gap-2 mb-3">
                                        <Icon icon=LuSettings width="14px" height="14px" style="color: rgba(139, 133, 160, 1)" />
                                        <h3 class="text-[10px] font-mono uppercase tracking-[0.12em] text-fg-tertiary">
                                            "Controls"
                                        </h3>
                                    </div>
                                    <ControlPanel
                                        controls=controls
                                        control_values=control_values
                                        accent_rgb=accent_rgb
                                        on_change=on_control_change
                                    />
                                </div>

                            </aside>
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
