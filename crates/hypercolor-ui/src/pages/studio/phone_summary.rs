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
//! The counting and filtering rules are pure ([`member_counts`],
//! [`member_summary`], [`zone_effect_choices`]) and tested without a DOM.

use std::collections::{HashMap, HashSet};

use hypercolor_types::api::scene::ZoneResource;
use hypercolor_types::effect::EffectCategory;
use leptos::prelude::*;
use leptos_icons::Icon;

use super::phone_view::StudioPhoneView;
use crate::app::EffectsContext;
use crate::components::bottom_sheet::BottomSheet;
use crate::components::page_header::{HeaderToolbar, HeaderTrailing, PageAccent, PageHeader};
use crate::components::scene_picker::ScenePicker;
use crate::components::section_label::{LabelSize, LabelTone, label_class};
use crate::components::zone_now_playing::set_zone_enabled;
use crate::components::zone_tabs::{PAUSED_DIM, ZoneDot};
use crate::effect_search::IndexedEffect;
use crate::icons::*;
use crate::style_utils::category_accent_rgb;
use crate::zones::surface::{Surface, SurfaceKind, apply_zone_id};
use crate::zones::{ZoneEffectState, ZonesContext};
use hypercolor_leptos_ext::events::Input;

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

/// Studio's phone layout.
#[component]
pub fn StudioPhoneSummary() -> impl IntoView {
    let zones_ctx = expect_context::<ZonesContext>();
    let fx = expect_context::<EffectsContext>();
    let (picker_zone, set_picker_zone) = signal(None::<Surface>);

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
        </div>
    }
}

/// One LED zone: name, hardware, what it plays, pause, and the switch.
#[component]
fn ZoneCard(
    state: ZoneEffectState,
    #[prop(into)] summary: Signal<String>,
    on_pick: Callback<Surface>,
) -> impl IntoView {
    let zones_ctx = expect_context::<ZonesContext>();
    let zone = state.zone.clone();
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
        </article>
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
