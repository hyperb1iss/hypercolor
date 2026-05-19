//! The five-source Add-layer picker — a modal that turns a chosen content
//! source (Effect, Media, Screen Capture, Web Page, Color) into a layer
//! draft for the panel to commit.

use hypercolor_leptos_ext::events::Input;
use hypercolor_types::layer::LayerSource;
use leptos::prelude::*;
use leptos_icons::Icon;

use super::source::{
    AddLayerScope, LayerSourceKind, color_layer_source, effect_layer_source, hex_to_layer_rgba,
    media_layer_source, screen_layer_source, web_layer_source,
};
use crate::api;
use crate::components::media_grid::MediaGrid;
use crate::icons::*;
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

    fn anonymous(source: LayerSource) -> Self {
        Self { name: None, source }
    }
}

/// Modal picker for adding a layer. Emits one [`NewLayerDraft`] on `on_pick`
/// and closes via `on_cancel` (backdrop click or the close button).
#[component]
pub fn AddLayerPicker(
    #[prop(into)] assets: Signal<Vec<api::MediaAssetRecord>>,
    /// Scopes the new layer can target (§6.6); a list shorter than two
    /// hides the selector — there is nothing to scope to.
    #[prop(into)]
    scopes: Signal<Vec<AddLayerScope>>,
    on_pick: Callback<(NewLayerDraft, AddLayerScope)>,
    on_cancel: Callback<()>,
) -> impl IntoView {
    let tab = RwSignal::new(LayerSourceKind::Effect);
    let scope = RwSignal::new(AddLayerScope::ThisSurface);
    // The source tabs stay scope-unaware: they emit a draft, and this
    // wrapper tags it with the chosen target scope before it leaves.
    let emit = Callback::new(move |draft: NewLayerDraft| {
        on_pick.run((draft, scope.get()));
    });
    let effects = LocalResource::new(api::fetch_effects);

    view! {
        <div
            class="fixed inset-0 z-50 flex items-center justify-center bg-black/70 px-4 backdrop-blur-sm animate-enter-fade"
            on:click=move |_| on_cancel.run(())
        >
            <div
                class="flex max-h-[82vh] w-full max-w-2xl flex-col overflow-hidden rounded-2xl border border-edge-subtle bg-surface-panel"
                style="box-shadow: 0 0 48px rgba(225, 53, 255, 0.13)"
                on:click=|event| event.stop_propagation()
            >
                <div class="flex items-center justify-between gap-3 border-b border-edge-subtle/60 px-5 py-3.5">
                    <div class="flex items-center gap-2.5">
                        <Icon icon=LuLayers width="16px" height="16px" style="color: rgba(225, 53, 255, 0.75)" />
                        <div>
                            <div class="text-sm font-semibold text-fg-primary">"Add Layer"</div>
                            <div class="text-[11px] text-fg-tertiary">"Pick a content source for the new layer"</div>
                        </div>
                    </div>
                    <button
                        type="button"
                        class="rounded-md border border-edge-subtle p-1.5 text-fg-tertiary transition-colors hover:text-fg-primary btn-press"
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
                                {kind.label()}
                            </button>
                        }
                    }).collect_view()}
                </div>

                {move || {
                    let available = scopes.get();
                    (available.len() > 1)
                        .then(|| {
                            view! {
                                <div class="flex flex-wrap items-center gap-2 border-b border-edge-subtle/50 px-5 py-2.5">
                                    <span class="text-[11px] font-medium text-fg-tertiary">
                                        "Add to"
                                    </span>
                                    <div class="flex flex-wrap items-center gap-1">
                                        {available
                                            .into_iter()
                                            .map(|option| {
                                                let selected = move || scope.get() == option;
                                                view! {
                                                    <button
                                                        type="button"
                                                        class="rounded-md border px-2 py-1 text-[11px] font-medium transition-colors"
                                                        class=("border-accent-muted", selected)
                                                        class=("bg-accent/10", selected)
                                                        class=("text-fg-primary", selected)
                                                        class=("border-edge-subtle/60", move || !selected())
                                                        class=("text-fg-tertiary", move || !selected())
                                                        on:click=move |_| scope.set(option)
                                                    >
                                                        {option.label()}
                                                    </button>
                                                }
                                            })
                                            .collect_view()}
                                    </div>
                                </div>
                            }
                        })
                }}

                <div class="flex-1 overflow-y-auto px-5 py-4">
                    <div class:hidden=move || tab.get() != LayerSourceKind::Effect>
                        <EffectTab effects=effects on_pick=emit />
                    </div>
                    <div class:hidden=move || tab.get() != LayerSourceKind::Media>
                        <MediaTab assets=assets on_pick=emit />
                    </div>
                    <div class:hidden=move || tab.get() != LayerSourceKind::ScreenCapture>
                        <ScreenTab on_pick=emit />
                    </div>
                    <div class:hidden=move || tab.get() != LayerSourceKind::WebPage>
                        <WebTab on_pick=emit />
                    </div>
                    <div class:hidden=move || tab.get() != LayerSourceKind::Color>
                        <ColorTab on_pick=emit />
                    </div>
                </div>
            </div>
        </div>
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
    effects: LocalResource<Result<Vec<api::EffectSummary>, String>>,
    on_pick: Callback<NewLayerDraft>,
) -> impl IntoView {
    let search = RwSignal::new(String::new());
    let filtered = Memo::new(move |_| {
        let Some(Ok(items)) = effects.get() else {
            return Vec::new();
        };
        let query = search.get().to_lowercase();
        let mut items = items
            .into_iter()
            .filter(|effect| effect.runnable)
            .filter(|effect| {
                query.is_empty()
                    || effect.name.to_lowercase().contains(&query)
                    || effect.category.to_lowercase().contains(&query)
            })
            .collect::<Vec<_>>();
        items.sort_by_key(|item| item.name.to_lowercase());
        items
    });

    view! {
        <PickerSearch placeholder="Search effects..." value=search />
        <Suspense fallback=move || view! { <PickerLoading /> }>
            {move || match effects.get() {
                None => view! { <PickerLoading /> }.into_any(),
                Some(Err(error)) => view! { <PickerError detail=error /> }.into_any(),
                Some(Ok(_)) => {
                    let items = filtered.get();
                    if items.is_empty() {
                        view! { <PickerEmpty detail="No matching effects" /> }.into_any()
                    } else {
                        view! {
                            <div class="space-y-1.5">
                                {items.into_iter().map(|effect| {
                                    let id = effect.id.clone();
                                    let name = effect.name.clone();
                                    let pick = move |_| {
                                        match effect_layer_source(&id) {
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
                                                <span class="block truncate text-[11px] text-fg-tertiary">{effect.category}</span>
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
    let pick_media = Callback::new(move |id: String| match media_layer_source(&id) {
        Ok(source) => {
            let name = assets.with_untracked(|list| {
                list.iter()
                    .find(|asset| asset.id == id)
                    .map(|asset| asset.name.clone())
            });
            on_pick.run(NewLayerDraft { name, source });
        }
        Err(error) => toasts::toast_error(&error),
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
                        selected_id=Signal::derive(|| None::<String>)
                        on_select=pick_media
                    />
                }.into_any()
            }
        }}
    }
}

#[component]
fn ScreenTab(on_pick: Callback<NewLayerDraft>) -> impl IntoView {
    view! {
        <div class="flex flex-col items-center justify-center gap-3 py-8 text-center">
            <span class="flex h-14 w-14 items-center justify-center rounded-2xl bg-accent/10">
                <Icon icon=LuMonitor width="24px" height="24px" style="color: rgba(241, 250, 140, 0.85)" />
            </span>
            <div class="text-sm font-semibold text-fg-primary">"Screen Capture"</div>
            <div class="max-w-sm text-xs text-fg-tertiary">
                "Mirrors your live desktop capture across the full canvas. Crop the region later from the layer's transform controls."
            </div>
            <button
                type="button"
                class="mt-1 inline-flex items-center gap-1.5 rounded-lg border border-accent-muted/30 bg-accent/10 px-4 py-2 text-xs font-semibold text-accent transition-colors hover:bg-accent/15 btn-press"
                on:click=move |_| on_pick.run(NewLayerDraft::anonymous(screen_layer_source()))
            >
                <Icon icon=LuPlus width="13px" height="13px" />
                "Add Screen Capture Layer"
            </button>
        </div>
    }
}

#[component]
fn WebTab(on_pick: Callback<NewLayerDraft>) -> impl IntoView {
    let url = RwSignal::new(String::new());
    let is_blank = move || url.get().trim().is_empty();
    let submit = move || {
        let raw = url.get();
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            toasts::toast_error("Enter a URL for the web layer");
            return;
        }
        on_pick.run(NewLayerDraft::anonymous(web_layer_source(trimmed)));
    };

    view! {
        <div class="flex flex-col gap-3 py-2">
            <div class="flex items-center gap-2.5">
                <span class="flex h-10 w-10 shrink-0 items-center justify-center rounded-xl bg-accent/10">
                    <Icon icon=LuGlobe width="18px" height="18px" style="color: rgba(130, 170, 255, 0.85)" />
                </span>
                <div>
                    <div class="text-sm font-semibold text-fg-primary">"Web Page"</div>
                    <div class="text-[11px] text-fg-tertiary">"Render any URL through the web viewport"</div>
                </div>
            </div>
            <input
                type="url"
                class="w-full rounded-lg border border-edge-subtle bg-surface-sunken/55 px-3 py-2 text-sm text-fg-primary placeholder-fg-tertiary/55 focus:border-accent-muted focus:outline-none"
                placeholder="https://example.com"
                prop:value=move || url.get()
                on:input=move |event| {
                    if let Some(text) = Input::from_event(event).value_string() {
                        url.set(text);
                    }
                }
                on:keydown=move |event| {
                    if event.key() == "Enter" {
                        submit();
                    }
                }
            />
            <button
                type="button"
                class="inline-flex items-center justify-center gap-1.5 self-start rounded-lg border border-accent-muted/30 bg-accent/10 px-4 py-2 text-xs font-semibold text-accent transition-colors hover:bg-accent/15 btn-press disabled:cursor-not-allowed disabled:opacity-45"
                disabled=is_blank
                on:click=move |_| submit()
            >
                <Icon icon=LuPlus width="13px" height="13px" />
                "Add Web Page Layer"
            </button>
        </div>
    }
}

#[component]
fn ColorTab(on_pick: Callback<NewLayerDraft>) -> impl IntoView {
    let hex = RwSignal::new("#e135ff".to_owned());
    let submit = move || match hex_to_layer_rgba(&hex.get()) {
        Some(rgba) => on_pick.run(NewLayerDraft::anonymous(color_layer_source(rgba))),
        None => toasts::toast_error("Enter a valid hex color"),
    };

    view! {
        <div class="flex flex-col gap-3 py-2">
            <div class="flex items-center gap-2.5">
                <span class="flex h-10 w-10 shrink-0 items-center justify-center rounded-xl bg-accent/10">
                    <Icon icon=LuPalette width="18px" height="18px" style="color: rgba(255, 106, 193, 0.85)" />
                </span>
                <div>
                    <div class="text-sm font-semibold text-fg-primary">"Color Fill"</div>
                    <div class="text-[11px] text-fg-tertiary">"A constant color across the whole canvas"</div>
                </div>
            </div>
            <div class="flex items-center gap-3">
                <input
                    type="color"
                    class="h-12 w-16 cursor-pointer rounded-lg border border-edge-subtle bg-surface-sunken/55"
                    prop:value=move || hex.get()
                    on:input=move |event| {
                        if let Some(text) = Input::from_event(event).value_string() {
                            hex.set(text);
                        }
                    }
                />
                <span class="font-mono text-sm text-fg-secondary">{move || hex.get().to_uppercase()}</span>
            </div>
            <button
                type="button"
                class="inline-flex items-center justify-center gap-1.5 self-start rounded-lg border border-accent-muted/30 bg-accent/10 px-4 py-2 text-xs font-semibold text-accent transition-colors hover:bg-accent/15 btn-press"
                on:click=move |_| submit()
            >
                <Icon icon=LuPlus width="13px" height="13px" />
                "Add Color Layer"
            </button>
        </div>
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
