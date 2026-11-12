//! Studio on a phone: a zone summary with per-zone effect switching.
//!
//! Below `md` (a phone, or a desktop window narrower than 768px) Studio
//! opens on what the scene is doing: one card per LED zone with its
//! devices, what it plays, a pause toggle, and a button that opens an
//! effect picker for that zone alone. Screens are listed read-only. The
//! spatial editor is one tap away (see [`super::phone_view`]); the summary
//! is the landing view because switching effects is the common job on a
//! phone and it needs no canvas.
//!
//! A layer limited to some of a zone's outputs is listed under the zone's
//! effect with a chip naming them; the chip opens a checklist of the
//! zone's outputs in a bottom sheet. Picking them on the canvas belongs to
//! the editor.
//!
//! The counting and filtering rules are pure ([`member_counts`],
//! [`member_summary`], [`zone_effect_choices`], [`limited_layers`]) and
//! tested without a DOM.

use std::collections::{HashMap, HashSet};

use hypercolor_types::api::scene::ZoneResource;
use hypercolor_types::effect::EffectCategory;
use leptos::prelude::*;
use leptos_icons::Icon;

use super::layer_target::{LAYER_SCOPE_CAPABILITY, target_outputs_for};
use super::phone_view::StudioPhoneView;
use crate::api;
use crate::app::EffectsContext;
use crate::components::bottom_sheet::BottomSheet;
use crate::components::layer_panel::scope::{Target, TargetOutput, TargetWords, target_words};
use crate::components::layer_panel::target_checklist::TargetChecklist;
use crate::components::page_header::{HeaderToolbar, HeaderTrailing, PageAccent, PageHeader};
use crate::components::scene_picker::ScenePicker;
use crate::components::section_label::{LabelSize, LabelTone, label_class};
use crate::components::zone_now_playing::set_zone_enabled;
use crate::components::zone_tabs::{PAUSED_DIM, ZoneDot};
use crate::effect_search::IndexedEffect;
use crate::icons::*;
use crate::style_utils::category_accent_rgb;
use crate::toasts;
use crate::zones::surface::{Surface, SurfaceKind, apply_zone_id, layer_label};
use crate::zones::{ZoneEffectState, ZonesContext};
use hypercolor_leptos_ext::events::Input;
use hypercolor_types::layer::{LayerSource, SceneLayerId};

/// How many outputs a zone holds, across how many devices.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MemberCounts {
    /// Zone members: whole devices or device segments.
    pub outputs: usize,
    /// Distinct devices behind those outputs.
    pub devices: usize,
}

/// Count a zone's outputs and the distinct devices behind them.
#[must_use]
pub fn member_counts(zone: &ZoneResource) -> MemberCounts {
    let devices: HashSet<&str> = zone
        .members
        .iter()
        .map(|member| member.device_id.as_str())
        .collect();
    MemberCounts {
        outputs: zone.members.len(),
        devices: devices.len(),
    }
}

/// One line about a zone's hardware, in words.
#[must_use]
pub fn member_summary(counts: MemberCounts) -> String {
    if counts.outputs == 0 {
        return "No devices yet".to_owned();
    }
    let outputs = if counts.outputs == 1 {
        "output"
    } else {
        "outputs"
    };
    let devices = if counts.devices == 1 {
        "device"
    } else {
        "devices"
    };
    format!(
        "{} {outputs} on {} {devices}",
        counts.outputs, counts.devices
    )
}

/// One effect the zone picker offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectChoice {
    pub id: String,
    pub name: String,
    pub category: String,
    pub favorite: bool,
}

/// The effects a zone can switch to, favorites first, each group in
/// library order. Only runnable LED effects are offered: display faces
/// belong to Screens. `query` filters by name, author, category, and tags.
#[must_use]
pub fn zone_effect_choices(
    effects: &[IndexedEffect],
    favorites: &HashSet<String>,
    query: &str,
) -> Vec<EffectChoice> {
    let term = query.trim().to_lowercase();
    let (mut favorite, rest): (Vec<_>, Vec<_>) = effects
        .iter()
        .filter(|entry| {
            entry.effect.runnable
                && entry.effect.category != EffectCategory::Display
                && entry.matches_search(&term)
        })
        .map(|entry| EffectChoice {
            id: entry.effect.id.clone(),
            name: entry.effect.name.clone(),
            category: entry.effect.category.as_str().to_owned(),
            favorite: favorites.contains(&entry.effect.id),
        })
        .partition(|choice| choice.favorite);
    favorite.extend(rest);
    favorite
}

/// A layer limited to some of its zone's outputs, as a zone card lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LimitedLayer {
    pub id: SceneLayerId,
    /// The layer's own name, else its effect's, else its kind in words.
    pub name: String,
    /// What it applies to, in the words its chip shows.
    pub words: TargetWords,
}

/// The layers of `zone` that are limited to some of its outputs, top of
/// the stack first. `effect_name` resolves an effect id to its name.
#[must_use]
pub fn limited_layers(
    zone: &ZoneResource,
    outputs: &[TargetOutput],
    effect_name: impl Fn(&str) -> Option<String>,
) -> Vec<LimitedLayer> {
    zone.layers
        .iter()
        .rev()
        .filter(|layer| layer.scope.is_some())
        .map(|layer| {
            let effect = match &layer.source {
                LayerSource::Effect { effect_id, .. } => effect_name(&effect_id.to_string()),
                _ => None,
            };
            LimitedLayer {
                id: layer.id,
                name: layer_label(layer, effect.as_deref()),
                words: target_words(layer.scope.as_ref(), outputs),
            }
        })
        .collect()
}

/// A layer whose target is being edited on the phone.
#[derive(Debug, Clone, PartialEq, Eq)]
struct EditedLayer {
    zone_id: String,
    layer_id: SceneLayerId,
    name: String,
}

/// Studio's phone layout.
#[component]
pub fn StudioPhoneSummary() -> impl IntoView {
    let zones_ctx = expect_context::<ZonesContext>();
    let fx = expect_context::<EffectsContext>();
    let (picker_zone, set_picker_zone) = signal(None::<Surface>);
    let (edited_layer, set_edited_layer) = signal(None::<EditedLayer>);

    let counts = Memo::new(move |_| {
        zones_ctx.active_scene.with(|scene| {
            scene
                .as_ref()
                .map(|scene| {
                    scene
                        .zones
                        .iter()
                        .map(|zone| (zone.id.to_string(), member_counts(zone)))
                        .collect::<HashMap<_, _>>()
                })
                .unwrap_or_default()
        })
    });
    let screens = Memo::new(move |_| {
        zones_ctx.zones.with(|zones| {
            zones
                .iter()
                .filter(|zone| zone.kind == SurfaceKind::Screen)
                .cloned()
                .collect::<Vec<_>>()
        })
    });
    let toolbar_summary = Memo::new(move |_| {
        let zones = zones_ctx.led_zones.with(Vec::len);
        let outputs: usize = zones_ctx.led_zones.with(|led| {
            counts.with(|counts| {
                led.iter()
                    .filter_map(|zone| counts.get(&zone.id))
                    .map(|counts| counts.outputs)
                    .sum()
            })
        });
        format!(
            "{zones} {} \u{00b7} {outputs} {}",
            if zones == 1 { "zone" } else { "zones" },
            if outputs == 1 { "output" } else { "outputs" }
        )
    });

    view! {
        <div class="flex h-full min-h-0 flex-col">
            <PageHeader icon=LuLayoutTemplate title="Studio" accent=PageAccent::Coral>
                <HeaderTrailing slot>
                    <ScenePicker />
                </HeaderTrailing>
                <HeaderToolbar slot>
                    <span class="truncate font-mono text-[11px] tabular-nums text-fg-tertiary">
                        {move || toolbar_summary.get()}
                    </span>
                </HeaderToolbar>
            </PageHeader>

            <div
                class="ambient-scroll page-scroll min-h-0 flex-1 overflow-y-auto px-4 pb-4 pt-4"
                style:--ambient-hue=crate::ambient::hue_style()
            >
                <div class="space-y-5">
                    <div class="flex items-start gap-3 rounded-xl border border-edge-subtle bg-surface-overlay/60 p-3.5 accent-coral">
                        <span
                            class="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg text-coral"
                            style="background: rgba(var(--glow-rgb), 0.10)"
                        >
                            <Icon icon=LuLayoutTemplate width="18px" height="18px" />
                        </span>
                        <div class="min-w-0 flex-1">
                            <div class="text-sm font-medium text-fg-primary">
                                "Every zone at a glance"
                            </div>
                            <p class="mt-1 text-xs leading-relaxed text-fg-tertiary">
                                "Switch what each zone plays here. Placing devices, drawing zones, and \
                                 stacking layers live in the editor, which is roomier on a bigger screen."
                            </p>
                            <a
                                href=crate::route_ui::route_href(StudioPhoneView::Editor.href())
                                class="mt-3 inline-flex min-h-11 items-center gap-1.5 rounded-lg border border-edge-subtle/70 bg-surface-overlay/40 px-3.5 text-xs font-medium text-fg-primary btn-press"
                            >
                                <Icon icon=LuLayoutTemplate width="14px" height="14px" />
                                "Open the editor"
                            </a>
                        </div>
                    </div>

                    <section aria-label="Zones">
                        <h2 class=format!("{} mb-2", label_class(LabelSize::Small, LabelTone::Default))>
                            "Zones"
                        </h2>
                        <div class="space-y-3">
                            <For
                                each=move || fx.zone_effects.get()
                                key=|state| {
                                    (
                                        state.zone.id.clone(),
                                        state.zone.enabled,
                                        state.zone.name.clone(),
                                        state.zone.color.clone(),
                                        state.effect_id.clone(),
                                        state.display_label(),
                                    )
                                }
                                children=move |state| {
                                    let zone_id = state.zone.id.clone();
                                    let summary = Signal::derive(move || {
                                        counts.with(|counts| {
                                            member_summary(counts.get(&zone_id).copied().unwrap_or_default())
                                        })
                                    });
                                    view! {
                                        <ZoneCard
                                            state=state
                                            summary=summary
                                            on_pick=Callback::new(move |zone| set_picker_zone.set(Some(zone)))
                                            on_target=Callback::new(move |layer| set_edited_layer.set(Some(layer)))
                                        />
                                    }
                                }
                            />
                        </div>
                    </section>

                    {move || (!screens.with(Vec::is_empty)).then(|| view! {
                        <section aria-label="Screens">
                            <h2 class=format!("{} mb-2", label_class(LabelSize::Small, LabelTone::Default))>
                                "Screens"
                            </h2>
                            <div class="divide-y divide-edge-subtle/40 rounded-xl border border-edge-subtle bg-surface-overlay/60">
                                {move || screens.get().into_iter().map(|screen| view! {
                                    <div class="flex min-h-13 items-center gap-3 px-3.5 py-2.5">
                                        <span class="flex shrink-0 text-fg-tertiary">
                                            <Icon icon=LuMonitor width="16px" height="16px" />
                                        </span>
                                        <div class="min-w-0 flex-1">
                                            <div class="truncate text-sm font-medium text-fg-primary">{screen.name.clone()}</div>
                                            <div class="truncate text-xs text-fg-tertiary">
                                                {screen.top_layer.clone().unwrap_or_else(|| "Nothing showing".to_owned())}
                                            </div>
                                        </div>
                                    </div>
                                }).collect_view()}
                            </div>
                        </section>
                    })}
                </div>
            </div>

            {move || picker_zone.get().map(|zone| view! {
                <ZoneEffectPicker zone=zone on_close=Callback::new(move |()| set_picker_zone.set(None)) />
            })}
            {move || edited_layer.get().map(|layer| view! {
                <LayerTargetSheet layer=layer on_close=Callback::new(move |()| set_edited_layer.set(None)) />
            })}
        </div>
    }
}

/// One LED zone: name, hardware, what it plays, pause, and the switch.
#[component]
fn ZoneCard(
    state: ZoneEffectState,
    #[prop(into)] summary: Signal<String>,
    on_pick: Callback<Surface>,
    on_target: Callback<EditedLayer>,
) -> impl IntoView {
    let zones_ctx = expect_context::<ZonesContext>();
    let fx = expect_context::<EffectsContext>();
    let caps = expect_context::<crate::app::CapabilitiesContext>();
    let devices = expect_context::<crate::app::DevicesContext>().devices_resource;
    let zone = state.zone.clone();
    // The zone's layers that play on only some of its outputs. A daemon
    // that cannot limit a layer has none to list.
    let limited = Memo::new({
        let zone_id = zone.id.clone();
        move |_| {
            if !caps.has(LAYER_SCOPE_CAPABILITY) {
                return Vec::new();
            }
            let devices = devices.get().and_then(Result::ok).unwrap_or_default();
            zones_ctx.active_scene.with(|scene| {
                let Some(zone) = scene.as_ref().and_then(|scene| {
                    scene
                        .zones
                        .iter()
                        .find(|zone| zone.id.to_string() == zone_id)
                }) else {
                    return Vec::new();
                };
                let outputs = target_outputs_for(zone, &devices);
                limited_layers(zone, &outputs, |effect_id| {
                    fx.effects_index.with(|effects| {
                        effects
                            .iter()
                            .find(|entry| entry.effect.id == effect_id)
                            .map(|entry| entry.effect.name.clone())
                    })
                })
            })
        }
    });
    let target_zone_id = StoredValue::new(zone.id.clone());
    let zone_id = zone.id.clone();
    let enabled = zone.enabled;
    let effect_label = state.display_label();
    let has_effect = effect_label.is_some();
    let category = state.effect_category.clone().unwrap_or_default();
    let accent = category_accent_rgb(&category);
    let pause_label = if enabled {
        format!("Pause {}", zone.name)
    } else {
        format!("Resume {}", zone.name)
    };
    let change_label = format!("Change the effect on {}", zone.name);
    let pick_zone = zone.clone();

    view! {
        <article
            class="rounded-xl border border-edge-subtle bg-surface-overlay/60 p-3.5"
            class=(PAUSED_DIM, !enabled)
            data-zone-card=zone.id.clone()
        >
            <div class="flex items-center gap-3">
                <span class="flex shrink-0 text-sm">
                    <ZoneDot color=Signal::stored(zone.color.clone()) />
                </span>
                <div class="min-w-0 flex-1">
                    <h3 class="truncate text-sm font-semibold text-fg-primary">{zone.name.clone()}</h3>
                    <div class="mt-0.5 truncate text-xs text-fg-tertiary">
                        {move || summary.get()}
                        {(!enabled).then_some(" \u{00b7} Paused")}
                    </div>
                </div>
                <button
                    type="button"
                    class="-mr-1.5 flex h-11 w-11 shrink-0 items-center justify-center rounded-lg text-fg-secondary btn-press"
                    aria-label=pause_label
                    on:click=move |_| set_zone_enabled(zones_ctx, zone_id.clone(), !enabled)
                >
                    {if enabled {
                        view! { <Icon icon=LuPause width="16px" height="16px" /> }.into_any()
                    } else {
                        view! { <Icon icon=LuPlay width="16px" height="16px" /> }.into_any()
                    }}
                </button>
            </div>
            <button
                type="button"
                class="mt-3 flex min-h-12 w-full items-center gap-3 rounded-lg border border-edge-subtle \
                       bg-surface-sunken/50 px-3 text-left btn-press"
                aria-label=change_label
                on:click=move |_| on_pick.run(pick_zone.clone())
            >
                <span
                    class="h-7 w-1 shrink-0 rounded-full"
                    style=format!("background: rgb({accent}); box-shadow: 0 0 10px rgba({accent}, 0.45)")
                    class:opacity-30=!has_effect
                />
                <span class="min-w-0 flex-1">
                    <span class="block truncate text-sm font-medium text-fg-primary">
                        {effect_label.unwrap_or_else(|| "No effect".to_owned())}
                    </span>
                    {(!category.is_empty()).then(|| view! {
                        <span class="block truncate text-[11px] capitalize text-fg-tertiary">{category.clone()}</span>
                    })}
                </span>
                <span class="flex shrink-0 items-center gap-1 text-xs font-medium text-accent">
                    "Change"
                    <Icon icon=LuChevronRight width="14px" height="14px" />
                </span>
            </button>
            <Show when=move || !limited.with(Vec::is_empty)>
                <div class="mt-3" data-zone-limited-layers="">
                    <div class=label_class(LabelSize::Micro, LabelTone::Default)>"Also playing"</div>
                    <ul class="mt-1 divide-y divide-edge-subtle/40">
                        <For
                            each=move || limited.get()
                            key=|layer| (layer.id, layer.name.clone(), layer.words.label().to_owned())
                            children=move |layer| {
                                let dormant = layer.words == TargetWords::Dormant;
                                let chip_label = format!("{} applies to {}", layer.name, layer.words.label());
                                let edited = EditedLayer {
                                    zone_id: target_zone_id.get_value(),
                                    layer_id: layer.id,
                                    name: layer.name.clone(),
                                };
                                view! {
                                    <li class="flex min-h-11 items-center gap-3 py-1">
                                        <span class="min-w-0 flex-1 truncate text-sm text-fg-primary">
                                            {layer.name.clone()}
                                        </span>
                                        <button
                                            type="button"
                                            class="inline-flex min-h-9 min-w-0 max-w-[60%] shrink-0 items-center gap-1.5 rounded-full border px-3 text-xs font-medium btn-press"
                                            class=("border-edge-subtle", !dormant)
                                            class=("bg-surface-overlay", !dormant)
                                            class=("text-fg-secondary", !dormant)
                                            class=("border-status-warning/30", dormant)
                                            class=("bg-status-warning/10", dormant)
                                            class=("text-status-warning", dormant)
                                            aria-haspopup="dialog"
                                            aria-label=chip_label
                                            on:click=move |_| on_target.run(edited.clone())
                                        >
                                            {if dormant {
                                                view! { <Icon icon=LuTriangleAlert width="12px" height="12px" /> }.into_any()
                                            } else {
                                                view! {
                                                    <span class="flex shrink-0 text-accent">
                                                        <Icon icon=LuScan width="12px" height="12px" />
                                                    </span>
                                                }.into_any()
                                            }}
                                            <span class="min-w-0 truncate">{layer.words.label().to_owned()}</span>
                                        </button>
                                    </li>
                                }
                            }
                        />
                    </ul>
                </div>
            </Show>
        </article>
    }
}

/// The checklist that chooses what one layer applies to, in a bottom
/// sheet. It edits a draft and writes once, on "Done".
#[component]
fn LayerTargetSheet(layer: EditedLayer, on_close: Callback<()>) -> impl IntoView {
    let zones_ctx = expect_context::<ZonesContext>();
    let devices = expect_context::<crate::app::DevicesContext>().devices_resource;
    let layer = StoredValue::new(layer);
    // The zone and layer as the scene has them now.
    let found = move || {
        zones_ctx.active_scene.with_untracked(|scene| {
            let scene = scene.as_ref()?;
            let (zone_id, layer_id) =
                layer.with_value(|layer| (layer.zone_id.clone(), layer.layer_id));
            let zone = scene
                .zones
                .iter()
                .find(|zone| zone.id.to_string() == zone_id)?;
            let stored = zone.layers.iter().find(|stored| stored.id == layer_id)?;
            Some((api::Fence::of(scene), zone.clone(), stored.clone()))
        })
    };
    let outputs = Memo::new(move |_| {
        let devices = devices.get().and_then(Result::ok).unwrap_or_default();
        let zone_id = layer.with_value(|layer| layer.zone_id.clone());
        zones_ctx.active_scene.with(|scene| {
            scene
                .as_ref()
                .and_then(|scene| {
                    scene
                        .zones
                        .iter()
                        .find(|zone| zone.id.to_string() == zone_id)
                })
                .map(|zone| target_outputs_for(zone, &devices))
                .unwrap_or_default()
        })
    });
    let draft = RwSignal::new(found().map_or_else(Target::default, |(_, _, stored)| {
        Target::of(stored.scope.as_ref())
    }));
    let (saving, set_saving) = signal(false);
    let title = layer.with_value(|layer| format!("{} applies to", layer.name));

    let done = move |_| {
        let target = draft.get_untracked();
        let Some((fence, zone, stored)) = found() else {
            on_close.run(());
            return;
        };
        if !target.is_complete() {
            return;
        }
        let scope = target.scope();
        if scope == stored.scope {
            on_close.run(());
            return;
        }
        set_saving.set(true);
        let request = api::scope_request(&stored, scope);
        leptos::task::spawn_local(async move {
            let zone_id = zone.id.to_string();
            let layer_id = stored.id.to_string();
            match api::update_layer(&zone_id, &layer_id, &request, Some(&fence)).await {
                Ok(api::LayerStackOutcome::Applied(_)) => {}
                Ok(api::LayerStackOutcome::Stale { .. }) => {
                    toasts::toast_error("The scene changed elsewhere. Reloaded, try again.");
                }
                Err(error) => toasts::toast_error(&format!(
                    "Couldn't change what the layer applies to: {error}"
                )),
            }
            zones_ctx.refresh.run(());
            let _ = set_saving.try_set(false);
            let _ = on_close.try_run(());
        });
    };

    view! {
        <BottomSheet on_close=on_close title=title>
            <TargetChecklist
                draft=draft
                outputs=outputs
                on_change=Callback::new(move |next| draft.set(next))
            />
            <button
                type="button"
                class="mt-3 flex min-h-12 w-full items-center justify-center rounded-lg border border-accent-muted bg-accent/10 text-sm font-semibold text-accent btn-press disabled:cursor-not-allowed disabled:opacity-45"
                disabled=move || saving.get() || !draft.with(Target::is_complete)
                on:click=done
            >
                "Done"
            </button>
        </BottomSheet>
    }
}

/// The effect picker for one zone, in a bottom sheet.
#[component]
fn ZoneEffectPicker(zone: Surface, on_close: Callback<()>) -> impl IntoView {
    let fx = expect_context::<EffectsContext>();
    let (query, set_query) = signal(String::new());
    let zone_id = zone.id.clone();
    let apply_zone = StoredValue::new(apply_zone_id(Some(&zone)));
    let current = Memo::new(move |_| {
        fx.zone_effects.with(|zones| {
            zones
                .iter()
                .find(|state| state.zone.id == zone_id)
                .and_then(|state| state.effect_id.clone())
        })
    });
    let choices = Memo::new(move |_| {
        fx.effects_index.with(|effects| {
            fx.favorite_ids.with(|favorites| {
                query.with(|query| zone_effect_choices(effects, favorites, query))
            })
        })
    });
    let title = format!("Effect for {}", zone.name);

    view! {
        <BottomSheet on_close=on_close title=title>
            <div class="relative mb-3">
                <span class="pointer-events-none absolute left-3 top-1/2 -translate-y-1/2 text-fg-tertiary">
                    <Icon icon=LuSearch width="16px" height="16px" />
                </span>
                <input
                    type="search"
                    placeholder="Search effects"
                    aria-label="Search effects"
                    class="h-11 w-full rounded-lg border border-edge-subtle bg-surface-sunken/60 pl-10 pr-3 \
                           text-base text-fg-primary placeholder-fg-tertiary focus:border-accent-muted focus:outline-none"
                    prop:value=move || query.get()
                    on:input=move |ev| {
                        if let Some(value) = Input::from_event(ev).value_string() {
                            set_query.set(value);
                        }
                    }
                />
            </div>
            {move || {
                let list = choices.get();
                if list.is_empty() {
                    return view! {
                        <div class="py-10 text-center text-sm text-fg-tertiary">"No effects match that search"</div>
                    }.into_any();
                }
                view! {
                    <ul class="divide-y divide-edge-subtle/40 rounded-lg border border-edge-subtle/60 bg-surface-sunken/40">
                        {list.into_iter().map(|choice| {
                            let accent = category_accent_rgb(&choice.category);
                            let id = choice.id.clone();
                            let check_id = choice.id.clone();
                            let is_current = Memo::new(move |_| current.with(|current| current.as_deref() == Some(check_id.as_str())));
                            view! {
                                <li>
                                    <button
                                        type="button"
                                        class="flex min-h-12 w-full items-center gap-3 px-3 text-left active:bg-surface-hover/30"
                                        aria-pressed=move || is_current.get().to_string()
                                        on:click=move |_| {
                                            fx.apply_effect_to(id.clone(), apply_zone.get_value());
                                            on_close.run(());
                                        }
                                    >
                                        <span
                                            class="h-2 w-2 shrink-0 rounded-full"
                                            style=format!("background: rgb({accent}); box-shadow: 0 0 8px rgba({accent}, 0.5)")
                                        />
                                        <span class="min-w-0 flex-1">
                                            <span class="block truncate text-sm text-fg-primary">{choice.name.clone()}</span>
                                            <span class="block truncate text-[11px] capitalize text-fg-tertiary">{choice.category.clone()}</span>
                                        </span>
                                        {choice.favorite.then(|| view! {
                                            <span class="flex shrink-0 text-coral" title="Favorite">
                                                <Icon icon=LuHeart width="13px" height="13px" style="fill: currentColor" />
                                            </span>
                                        })}
                                        <span class="flex w-4 shrink-0 justify-center text-accent">
                                            {move || is_current.get().then(|| view! { <Icon icon=LuCheck width="16px" height="16px" /> })}
                                        </span>
                                    </button>
                                </li>
                            }
                        }).collect_view()}
                    </ul>
                }.into_any()
            }}
        </BottomSheet>
    }
}
