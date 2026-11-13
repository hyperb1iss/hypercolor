//! Add-layer picker — a modal that turns an effect/face or media asset into
//! a layer draft for the panel to commit.

use hypercolor_leptos_ext::events::Input;
use hypercolor_types::asset::AssetId;
use hypercolor_types::layer::LayerSource;
use leptos::prelude::*;
use leptos_icons::Icon;

use super::source::{
    EffectPickerMode, LayerSourceKind, effect_category_label, effect_picker_matches_query,
    media_layer_source_for, remembered_effect_layer_source,
};
use crate::api;
use crate::components::media_grid::MediaGrid;
use crate::components::modal::Modal;
use crate::icons::*;
use crate::preferences::PreferencesStore;
use crate::toasts;

/// A picked layer ready for the panel to send as a `create_layer` request.
#[derive(Debug, Clone)]
pub struct NewLayerDraft {
    pub name: Option<String>,
    pub source: LayerSource,
}

impl NewLayerDraft {
    fn named(name: impl Into<String>, source: LayerSource) -> Self {
        Self {
            name: Some(name.into()),
            source,
        }
    }
}

/// Modal picker for adding a layer. Emits one [`NewLayerDraft`] on `on_pick`
/// and closes via `on_cancel` (backdrop click or the close button).
#[component]
pub fn AddLayerPicker(
    #[prop(into)] assets: Signal<Vec<api::MediaAssetRecord>>,
    /// Faces when screens are all the new layer plays on, effects
    /// otherwise.
    #[prop(into)]
    mode: Signal<EffectPickerMode>,
    /// The outputs the new layer will be limited to, in words. `None` for
    /// a layer on the whole zone.
    #[prop(into)]
    target: Signal<Option<String>>,
    /// What adding to every screen of the scene would reach, in words,
    /// when the scene has screens beyond `target`. Offers the choice.
    #[prop(into)]
    all_screens: Signal<Option<String>>,
    /// The picked layer, and whether it goes to every screen.
    on_pick: Callback<(NewLayerDraft, bool)>,
    on_cancel: Callback<()>,
) -> impl IntoView {
    let tab = RwSignal::new(LayerSourceKind::Effect);
    let everywhere = RwSignal::new(false);
    let effect_mode = mode;
    // The source tabs know nothing of where the layer goes: they emit a
    // draft, and this wrapper says whether it is for every screen.
    let emit = Callback::new(move |draft: NewLayerDraft| {
        let all = everywhere.get_untracked() && all_screens.with_untracked(Option::is_some);
        on_pick.run((draft, all));
    });
    let effects = api::daemon_resource(api::fetch_effects);

    view! {
        <Modal
            on_close=on_cancel
            label="Add layer"
            container_class="fixed inset-0 z-50 flex items-center justify-center px-4 animate-enter-fade"
            backdrop_class="absolute inset-0 bg-black/70 backdrop-blur-sm"
        >
            <div
                class="relative flex max-h-[82vh] w-full max-w-2xl flex-col overflow-hidden rounded-xl border border-edge-subtle bg-surface-overlay"
                style="box-shadow: 0 0 48px rgba(225, 53, 255, 0.13)"
                on:click=|event| event.stop_propagation()
            >
                <div class="flex items-center justify-between gap-3 border-b border-edge-subtle/60 px-5 py-3.5">
                    <div class="flex items-center gap-2.5">
                        <Icon icon=LuLayers width="16px" height="16px" style="color: rgba(225, 53, 255, 0.75)" />
                        <div>
                            <div class="text-sm font-semibold text-fg-primary">"Add Layer"</div>
                            <div class="text-[11px] text-fg-tertiary">
                                {move || match target.get() {
                                    Some(_) if everywhere.get() && all_screens.with(Option::is_some) => {
                                        "Pick what plays on every screen".to_owned()
                                    }
                                    Some(target) => format!("Pick what plays on {target}"),
                                    None => "Pick a content source for the new layer".to_owned(),
                                }}
                            </div>
                        </div>
                    </div>
                    <button
                        type="button"
                        class="rounded-md border border-edge-subtle p-1.5 text-fg-tertiary transition-colors hover:text-fg-primary btn-press"
                        aria-label="Close"
                        on:click=move |_| on_cancel.run(())
                    >
                        <Icon icon=LuX width="15px" height="15px" />
                    </button>
                </div>

                <div class="flex items-center gap-1 border-b border-edge-subtle/50 px-3 py-2">
                    {LayerSourceKind::ALL.into_iter().map(|kind| {
                        let is_active = move || tab.get() == kind;
                        view! {
                            <button
                                type="button"
                                class="rounded-lg px-3 py-1.5 text-[11px] font-medium transition-all chip-interactive"
                                class=("text-fg-tertiary", move || !is_active())
                                class=("hover:text-fg-secondary", move || !is_active())
                                class=("bg-accent/12", is_active)
                                class=("text-accent", is_active)
                                on:click=move |_| tab.set(kind)
                            >
                                {move || {
                                    if kind == LayerSourceKind::Effect {
                                        effect_mode.get().tab_label()
                                    } else {
                                        kind.label()
                                    }
                                }}
                            </button>
                        }
                    }).collect_view()}
                </div>

                // Where it goes, when there is a choice: the screens it was
                // opened for, or every screen of the scene. The line under
                // the choice says what "every" reaches before it is made.
                {move || {
                    let reach = all_screens.get()?;
                    let here = target.get().unwrap_or_else(|| "These screens".to_owned());
                    let choice = move |all: bool, label: String| {
                        let selected = move || everywhere.get() == all;
                        view! {
                            <button
                                type="button"
                                role="radio"
                                aria-checked=move || selected().to_string()
                                class="max-w-[16rem] truncate rounded-md border px-2 py-1 text-[11px] font-medium transition-colors"
                                class=("border-accent-muted", selected)
                                class=("bg-accent/10", selected)
                                class=("text-fg-primary", selected)
                                class=("border-edge-subtle/60", move || !selected())
                                class=("text-fg-tertiary", move || !selected())
                                class=("hover:text-fg-secondary", move || !selected())
                                on:click=move |_| everywhere.set(all)
                            >
                                {label}
                            </button>
                        }
                    };
                    Some(view! {
                        <div class="border-b border-edge-subtle/50 px-5 py-2.5">
                            <div class="flex flex-wrap items-center gap-2" role="radiogroup" aria-label="Add to">
                                <span class="text-[11px] font-medium text-fg-tertiary">"Add to"</span>
                                {choice(false, here)}
                                {choice(true, "All screens".to_owned())}
                            </div>
                            <Show when=move || everywhere.get()>
                                <p class="mt-1.5 text-[11px] leading-relaxed text-fg-tertiary" data-all-screens-reach="">
                                    {reach.clone()}
                                </p>
                            </Show>
                        </div>
                    })
                }}

                <div class="ambient-scroll flex-1 overflow-y-auto px-5 py-4" style:--ambient-hue=crate::ambient::hue_style()>
                    <div class:hidden=move || tab.get() != LayerSourceKind::Effect>
                        <EffectTab effects=effects mode=effect_mode on_pick=emit />
                    </div>
                    <div class:hidden=move || tab.get() != LayerSourceKind::Media>
                        <MediaTab assets=assets on_pick=emit />
                    </div>
                </div>
            </div>
        </Modal>
    }
}

#[component]
fn PickerSearch(placeholder: &'static str, value: RwSignal<String>) -> impl IntoView {
    view! {
        <label class="mb-3 flex items-center gap-2 rounded-lg border border-edge-subtle bg-surface-sunken/55 px-3 py-2">
            <Icon icon=LuSearch width="13px" height="13px" style="color: rgba(139, 133, 160, 0.6)" />
            <input
                type="text"
                class="w-full bg-transparent text-xs text-fg-primary placeholder-fg-tertiary/60 focus:outline-none"
                placeholder=placeholder
                prop:value=move || value.get()
                on:input=move |event| {
                    if let Some(text) = Input::from_event(event).value_string() {
                        value.set(text);
                    }
                }
            />
        </label>
    }
}

#[component]
fn EffectTab(
    effects: LocalResource<api::ApiResult<Vec<api::EffectSummary>>>,
    #[prop(into)] mode: Signal<EffectPickerMode>,
    on_pick: Callback<NewLayerDraft>,
) -> impl IntoView {
    // Absent when the panel mounts outside the app shell; picks then start
    // at the effect's defaults.
    let preferences = use_context::<PreferencesStore>();
    let search = RwSignal::new(String::new());
    let filtered = Memo::new(move |_| {
        let Some(Ok(items)) = effects.get() else {
            return Vec::new();
        };
        let query = search.get().to_lowercase();
        let mode = mode.get();
        let mut items = items
            .into_iter()
            .filter(|effect| effect.runnable)
            .filter(|effect| mode.includes_category(effect.category))
            .filter(|effect| effect_picker_matches_query(&effect.name, effect.category, &query))
            .collect::<Vec<_>>();
        items.sort_by_key(|item| (mode.sort_bucket(item.category), item.name.to_lowercase()));
        items
    });

    view! {
        {move || view! {
            <PickerSearch placeholder=mode.get().search_placeholder() value=search />
        }}
        <Suspense fallback=move || view! { <PickerLoading /> }>
            {move || match effects.get() {
                None => view! { <PickerLoading /> }.into_any(),
                Some(Err(error)) => view! { <PickerError detail=error.to_string() /> }.into_any(),
                Some(Ok(_)) => {
                    let items = filtered.get();
                    if items.is_empty() {
                        view! { <PickerEmpty detail=mode.get().empty_detail() /> }.into_any()
                    } else {
                        view! {
                            <div class="space-y-1.5">
                                {items.into_iter().map(|effect| {
                                    let id = effect.id.clone();
                                    let name = effect.name.clone();
                                    let category = effect_category_label(effect.category);
                                    let pick = move |_| {
                                        let prefs = preferences.and_then(|store| store.get(&id));
                                        match remembered_effect_layer_source(&id, prefs.as_ref()) {
                                            Ok(source) => on_pick.run(NewLayerDraft::named(name.clone(), source)),
                                            Err(error) => toasts::toast_error(&error),
                                        }
                                    };
                                    view! {
                                        <button
                                            type="button"
                                            class="flex w-full items-center gap-3 rounded-lg border border-edge-subtle/70 bg-surface-overlay/40 px-3 py-2.5 text-left transition-all card-hover"
                                            on:click=pick
                                        >
                                            <span class="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-accent/10">
                                                <Icon icon=LuZap width="14px" height="14px" style="color: rgba(225, 53, 255, 0.8)" />
                                            </span>
                                            <span class="min-w-0 flex-1">
                                                <span class="block truncate text-sm font-medium text-fg-primary">{effect.name}</span>
                                                <span class="block truncate text-[11px] text-fg-tertiary">{category}</span>
                                            </span>
                                            <Icon icon=LuPlus width="14px" height="14px" style="color: rgba(139, 133, 160, 0.55)" />
                                        </button>
                                    }
                                }).collect_view()}
                            </div>
                        }.into_any()
                    }
                }
            }}
        </Suspense>
    }
}

#[component]
fn MediaTab(
    #[prop(into)] assets: Signal<Vec<api::MediaAssetRecord>>,
    on_pick: Callback<NewLayerDraft>,
) -> impl IntoView {
    let search = RwSignal::new(String::new());
    let filtered = Memo::new(move |_| {
        let query = search.get().to_lowercase();
        let mut items = assets.get();
        items.retain(|asset| {
            query.is_empty()
                || asset.name.to_lowercase().contains(&query)
                || asset.mime_type.to_lowercase().contains(&query)
        });
        items.sort_by_key(|item| item.name.to_lowercase());
        items
    });

    // A click on a media card is an immediate add, so resolve the asset's
    // name for the layer and emit a draft — no persistent selection.
    let pick_media = Callback::new(move |id: AssetId| {
        let name = assets.with_untracked(|list| {
            list.iter()
                .find(|asset| asset.id == id)
                .map(|asset| asset.name.clone())
        });
        on_pick.run(NewLayerDraft {
            name,
            source: media_layer_source_for(id),
        });
    });

    view! {
        <PickerSearch placeholder="Search media..." value=search />
        {move || {
            if filtered.get().is_empty() {
                view! {
                    <PickerEmpty detail="No matching media — upload from the Media library" />
                }.into_any()
            } else {
                view! {
                    <MediaGrid
                        assets=filtered
                        selected_id=Signal::derive(|| None::<AssetId>)
                        on_select=pick_media
                    />
                }.into_any()
            }
        }}
    }
}

#[component]
fn PickerLoading() -> impl IntoView {
    view! {
        <div class="space-y-1.5">
            {(0..5).map(|_| view! {
                <div class="h-[52px] rounded-lg border border-edge-subtle/50 bg-surface-overlay/30 animate-pulse" />
            }).collect_view()}
        </div>
    }
}

#[component]
fn PickerEmpty(detail: &'static str) -> impl IntoView {
    view! {
        <div class="flex flex-col items-center justify-center gap-2 py-10 text-center">
            <Icon icon=LuLayers width="26px" height="26px" style="color: rgba(139, 133, 160, 0.32)" />
            <div class="text-xs text-fg-tertiary/75">{detail}</div>
        </div>
    }
}

#[component]
fn PickerError(detail: String) -> impl IntoView {
    view! {
        <div class="rounded-lg border border-status-error/30 bg-status-error/10 px-3 py-3 text-xs text-status-error">
            {detail}
        </div>
    }
}
