//! Effects browse page — grid of effect cards with filtering and detail panel.

use leptos::prelude::*;
use leptos_icons::Icon;
use wasm_bindgen::JsCast;

use crate::api;
use crate::app::{EffectsContext, WsContext};
use crate::components::canvas_preview::CanvasPreview;
use crate::components::control_panel::ControlPanel;
use crate::components::effect_card::EffectCard;
use crate::components::preset_panel::PresetToolbar;
use crate::icons::*;
use hypercolor_types::effect::{ControlDefinition, ControlType, ControlValue};

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
    "audio",
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

    let (search, set_search) = signal(String::new());
    let (category_filter, set_category_filter) = signal("all".to_string());
    let (selected_authors, set_selected_authors) =
        signal(std::collections::BTreeSet::<String>::new());
    let (author_dropdown_open, set_author_dropdown_open) = signal(false);
    let (favorites_only, set_favorites_only) = signal(false);
    let (audio_reactive_only, set_audio_reactive_only) = signal(false);

    // Derive unique sorted author list from loaded effects
    let authors = Memo::new(move |_| {
        let Some(Ok(effects)) = fx.effects_resource.get() else {
            return Vec::new();
        };
        let mut seen = std::collections::BTreeSet::new();
        for e in &effects {
            if !e.author.is_empty() {
                seen.insert(e.author.clone());
            }
        }
        seen.into_iter().collect::<Vec<_>>()
    });

    // Count how many effects are favorited (for the badge)
    let favorites_count = Memo::new(move |_| fx.favorite_ids.get().len());

    let canvas_frame = Signal::derive(move || ws.canvas_frame.get());
    let ws_fps = Signal::derive(move || ws.fps.get());
    let controls = Signal::derive(move || fx.active_controls.get());
    let control_values = Signal::derive(move || fx.active_control_values.get());
    let accent_rgb =
        Signal::derive(move || category_accent_rgb(&fx.active_effect_category.get()).to_string());

    let has_active = Memo::new(move |_| fx.active_effect_id.get().is_some());

    // Filter effects
    let filtered_effects = Memo::new(move |_| {
        let Some(Ok(effects)) = fx.effects_resource.get() else {
            return Vec::new();
        };

        let search_term = search.get().to_lowercase();
        let cat = category_filter.get();
        let sel_authors = selected_authors.get();
        let fav_only = favorites_only.get();
        let audio_only = audio_reactive_only.get();
        let fav_ids = fx.favorite_ids.get();

        effects
            .into_iter()
            .filter(|e| {
                if cat != "all" && e.category != cat {
                    return false;
                }
                if !sel_authors.is_empty() && !sel_authors.contains(&e.author) {
                    return false;
                }
                if fav_only && !fav_ids.contains(&e.id) {
                    return false;
                }
                if audio_only && !e.audio_reactive {
                    return false;
                }
                if !search_term.is_empty() {
                    let matches_name = e.name.to_lowercase().contains(&search_term);
                    let matches_desc = e.description.to_lowercase().contains(&search_term);
                    let matches_author = e.author.to_lowercase().contains(&search_term);
                    let matches_tags = e
                        .tags
                        .iter()
                        .any(|t| t.to_lowercase().contains(&search_term));
                    return matches_name || matches_desc || matches_author || matches_tags;
                }
                true
            })
            .collect::<Vec<_>>()
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
        if fx.active_effect_id.get().is_some() {
            let controls_snapshot = fx.active_controls.get();
            if let Some(control_value) = json_to_control_value(&name, &controls_snapshot, &value) {
                let control_name = name.clone();
                fx.set_active_control_values.update(move |values| {
                    values.insert(control_name, control_value);
                });
            }

            let controls_json = serde_json::json!({ name: value });
            leptos::task::spawn_local(async move {
                let _ = api::update_controls(&controls_json).await;
            });
        }
    });

    view! {
        <div class="flex flex-col h-full -m-6 animate-fade-in">
            // Fixed header — title + search + categories + capability filters
            <div class="shrink-0 px-6 pt-6 pb-4 space-y-4 bg-layer-0 z-10">
                // Title row
                <div class="flex items-baseline justify-between">
                    <h1 class="text-lg font-medium text-fg">"Effects"</h1>
                    <span class="text-[11px] font-mono text-fg-dim tabular-nums">
                        {move || effect_count.get()} " effects"
                    </span>
                </div>

                // Search bar
                <div class="relative">
                    <span class="absolute left-3.5 top-1/2 -translate-y-1/2 pointer-events-none text-fg-dim">
                        <Icon icon=LuSearch width="14px" height="14px" />
                    </span>
                    <input
                        type="text"
                        placeholder="Search effects..."
                        class="w-full bg-layer-2/60 border border-white/[0.04] rounded-lg pl-9 pr-10 py-2 text-sm text-fg
                               placeholder-fg-dim focus:outline-none focus:border-electric-purple/20
                               focus:shadow-[0_0_0_1px_rgba(225,53,255,0.1),0_0_20px_rgba(225,53,255,0.06)]
                               transition-all duration-300"
                        prop:value=move || search.get()
                        on:input=move |ev| {
                            let target = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                            if let Some(el) = target {
                                set_search.set(el.value());
                            }
                        }
                    />
                    <kbd class="absolute right-3 top-1/2 -translate-y-1/2 text-[9px] font-mono text-fg-dim bg-white/[0.03] px-1.5 py-0.5 rounded border border-white/[0.03]">"/"</kbd>
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
                    <div class="w-px h-5 bg-white/[0.06]" />

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
                        <Icon
                            icon=LuHeart
                            width="11px"
                            height="11px"
                            style=move || if favorites_only.get() { "fill: currentColor" } else { "" }
                        />
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
                                                       rounded-xl border border-white/[0.06] bg-layer-2 shadow-[0_8px_32px_rgba(0,0,0,0.4)]
                                                       py-1 animate-fade-in scrollbar-none"
                                            >
                                                // Clear all option
                                                <button
                                                    class="w-full text-left px-3 py-1.5 text-xs text-fg-dim hover:bg-white/[0.04] transition-colors"
                                                    on:click=move |_| {
                                                        set_selected_authors.set(std::collections::BTreeSet::new());
                                                        set_author_dropdown_open.set(false);
                                                    }
                                                >
                                                    "All authors"
                                                </button>
                                                <div class="h-px bg-white/[0.04] mx-2 my-0.5" />
                                                // Author checkboxes
                                                {list.into_iter().map(|author| {
                                                    let author_toggle = author.clone();
                                                    let author_check = author.clone();
                                                    let author_check2 = author.clone();
                                                    let author_label = author.clone();
                                                    view! {
                                                        <button
                                                            class="w-full text-left px-3 py-1.5 flex items-center gap-2.5 text-xs hover:bg-white/[0.04] transition-colors group"
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
                                                            <span class="text-fg-muted group-hover:text-fg transition-colors truncate">
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

            // Scrollable content: grid + pinned detail panel
            <div class="flex-1 overflow-y-auto px-6 pb-6">
                <div class="flex gap-5 items-start">
                    // Effect grid — z-[2] ensures cards stay above the sticky aside for click targeting
                    <div class="flex-1 min-w-0 relative z-[2]">
                        <Suspense fallback=move || view! { <LoadingSkeleton /> }>
                            {move || {
                                let effects = filtered_effects.get();
                                if effects.is_empty() {
                                    view! {
                                        <div class="text-center py-20">
                                            <div class="text-fg-dim text-sm">"No effects found"</div>
                                            <div class="text-fg-dim/50 text-xs mt-1">"Try a different search or category"</div>
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
                                            {effects.into_iter().enumerate().map(|(i, effect)| {
                                                let effect_id = effect.id.clone();
                                                let fav_effect_id = effect.id.clone();
                                                let is_active = Signal::derive(move || {
                                                    fx.active_effect_id.get().as_deref() == Some(&effect_id)
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
                                                        index=i
                                                    />
                                                }
                                            }).collect_view()}
                                        </div>
                                    }.into_any()
                                }
                            }}
                        </Suspense>
                    </div>

                    // Detail panel — sticky, scrolls with cards until bottom pins
                    {move || {
                        fx.active_effect_id.get().map(|_| {
                            let rgb = accent_rgb.get();
                            let dot_style = format!("background: rgb({}); box-shadow: 0 0 8px rgba({}, 0.6)", rgb, rgb);
                            let controls_accent = format!("border-top: 2px solid rgba({}, 0.15)", rgb);
                            view! {
                                <aside
                                    class="w-[420px] shrink-0 sticky top-0 self-start space-y-3 animate-slide-in-right scrollbar-none z-[1]"
                                    style="max-height: calc(100vh - 10rem); overflow-y: auto"
                                >
                                    // Active effect name with category-colored dot
                                    {move || fx.active_effect_name.get().map(|name| {
                                        let dot_s = dot_style.clone();
                                        view! {
                                            <div class="flex items-center gap-2.5 px-1">
                                                <div class="w-2.5 h-2.5 rounded-full dot-alive shrink-0" style=dot_s />
                                                <span class="text-base font-medium text-fg">{name}</span>
                                            </div>
                                        }
                                    })}

                                    // Preset toolbar — select, save, create, edit, delete
                                    <PresetToolbar
                                        effect_id=Signal::derive(move || fx.active_effect_id.get())
                                        control_values=control_values
                                        accent_rgb=accent_rgb
                                        on_preset_applied=Callback::new(move |()| {
                                            let set_name = fx.set_active_effect_name;
                                            let set_controls = fx.set_active_controls;
                                            let set_values = fx.set_active_control_values;
                                            let set_preset = fx.set_active_preset_id;
                                            leptos::task::spawn_local(async move {
                                                if let Ok(Some(active)) = api::fetch_active_effect().await {
                                                    set_name.set(Some(active.name));
                                                    set_controls.set(active.controls);
                                                    set_values.set(active.control_values);
                                                    set_preset.set(active.active_preset_id);
                                                }
                                            });
                                        })
                                        active_preset_id_signal=Signal::derive(move || fx.active_preset_id.get())
                                    />

                                    // Live preview — no border, black bleeds to edge
                                    <div class="rounded-xl bg-black overflow-hidden shadow-[0_4px_24px_rgba(0,0,0,0.3)]">

                                        <CanvasPreview
                                            frame=canvas_frame
                                            fps=ws_fps
                                            show_fps=true
                                        />
                                    </div>

                                    // Controls panel with category accent line
                                    <div
                                        class="rounded-xl bg-layer-1 border border-white/[0.06] p-5
                                               shadow-[0_2px_12px_rgba(0,0,0,0.2)]"
                                        style=controls_accent.clone()
                                    >
                                        <div class="flex items-center gap-2 mb-4">
                                            <Icon icon=LuSettings width="16px" height="16px" style="color: rgba(139, 133, 160, 1)" />
                                            <h3 class="text-xs font-mono uppercase tracking-[0.12em] text-fg-dim">
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
        </div>
    }
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
        let is_dropdown = controls
            .iter()
            .find(|def| def.control_id().eq_ignore_ascii_case(control_name))
            .map(|def| matches!(def.control_type, ControlType::Dropdown))
            .unwrap_or(false);
        if is_dropdown {
            return Some(ControlValue::Enum(v.to_owned()));
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

/// Loading skeleton for the effects grid.
#[component]
fn LoadingSkeleton() -> impl IntoView {
    view! {
        <div class="grid grid-cols-[repeat(auto-fill,minmax(300px,1fr))] gap-4">
            {(0..12).map(|_| {
                view! {
                    <div class="rounded-2xl border border-white/[0.03] bg-layer-2/40 px-4 py-4 animate-pulse space-y-3">
                        <div class="flex justify-between">
                            <div class="h-4 w-28 bg-white/[0.04] rounded" />
                            <div class="h-4 w-14 bg-white/[0.04] rounded-full" />
                        </div>
                        <div class="space-y-1.5">
                            <div class="h-3 w-full bg-white/[0.02] rounded" />
                            <div class="h-3 w-3/4 bg-white/[0.02] rounded" />
                        </div>
                        <div class="flex gap-1.5">
                            <div class="h-4 w-14 bg-white/[0.02] rounded" />
                            <div class="h-4 w-12 bg-white/[0.02] rounded" />
                        </div>
                        <div class="flex justify-between pt-1 border-t border-white/[0.02]">
                            <div class="h-2.5 w-16 bg-white/[0.02] rounded" />
                            <div class="h-2.5 w-12 bg-white/[0.02] rounded" />
                        </div>
                    </div>
                }
            }).collect_view()}
        </div>
    }
}
