//! `/assets` — user media library and active scene layer stack.

use std::str::FromStr;

use hypercolor_leptos_ext::events::{Change, Input};
use hypercolor_types::asset::AssetId;
use hypercolor_types::layer::{
    LayerAdjust, LayerBlendMode, LayerSource, LayerTransform, MediaPlayback, SceneLayer,
};
use hypercolor_types::scene::RenderGroupRole;
use hypercolor_types::viewport::FitMode;
use leptos::html;
use leptos::prelude::*;
use leptos_icons::Icon;

use crate::api;
use crate::components::page_header::{HeaderToolbar, HeaderTrailing, PageAccent, PageHeader};
use crate::components::page_search_bar::PageSearchBar;
use crate::components::silk_select::SilkSelect;
use crate::icons::*;
use crate::style_utils::filter_chips;
use crate::toasts;

const ASSET_FILTERS: &[(&str, &str)] = &[
    ("all", "225, 53, 255"),
    ("image", "128, 255, 234"),
    ("gif", "255, 106, 193"),
    ("video", "130, 170, 255"),
    ("lottie", "80, 250, 123"),
];

#[component]
pub fn AssetsPage() -> impl IntoView {
    let (search, set_search) = signal(String::new());
    let (kind_filter, set_kind_filter) = signal("all".to_owned());
    let (asset_refresh_tick, set_asset_refresh_tick) = signal(0_u64);
    let (scene_refresh_tick, set_scene_refresh_tick) = signal(0_u64);
    let (layers_refresh_tick, set_layers_refresh_tick) = signal(0_u64);
    let (selected_asset_id, set_selected_asset_id) = signal(None::<String>);
    let (selected_group_id, set_selected_group_id) = signal(None::<String>);
    let (uploading, set_uploading) = signal(false);
    let input_ref = NodeRef::<html::Input>::new();

    let assets_resource = LocalResource::new(move || {
        let _ = asset_refresh_tick.get();
        async move { api::list_assets().await }
    });

    let active_scene_resource = LocalResource::new(move || {
        let _ = scene_refresh_tick.get();
        async move { api::fetch_active_scene().await }
    });

    let active_scene =
        Signal::derive(move || active_scene_resource.get().and_then(Result::ok).flatten());

    Effect::new(move |_| {
        let Some(scene) = active_scene.get() else {
            if selected_group_id.get_untracked().is_some() {
                set_selected_group_id.set(None);
            }
            return;
        };

        let current = selected_group_id.get_untracked();
        let current_is_valid = current
            .as_ref()
            .is_some_and(|id| scene.groups.iter().any(|group| group.id.to_string() == *id));
        if current_is_valid {
            return;
        }

        let next = scene
            .groups
            .iter()
            .find(|group| group.role != RenderGroupRole::Display)
            .or_else(|| scene.groups.first())
            .map(|group| group.id.to_string());
        set_selected_group_id.set(next);
    });

    let layers_resource = LocalResource::new(move || {
        let _ = layers_refresh_tick.get();
        let scene = active_scene.get();
        let group_id = selected_group_id.get();

        async move {
            match (scene, group_id) {
                (Some(scene), Some(group_id)) => api::list_layers(&scene.id, &group_id).await,
                _ => Ok(api::LayerStackResponse {
                    items: Vec::new(),
                    layers_version: 0,
                }),
            }
        }
    });

    let filtered_assets = Memo::new(move |_| {
        let Some(Ok(response)) = assets_resource.get() else {
            return Vec::new();
        };

        let query = search.get().to_lowercase();
        let filter = kind_filter.get();
        let mut assets = response.items;
        assets.retain(|asset| {
            let kind_matches = filter == "all" || asset_kind(asset) == filter;
            let search_matches = query.is_empty()
                || asset.name.to_lowercase().contains(&query)
                || asset.mime_type.to_lowercase().contains(&query)
                || asset
                    .tags
                    .iter()
                    .any(|tag| tag.to_lowercase().contains(&query));
            kind_matches && search_matches
        });
        assets.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        assets
    });

    Effect::new(move |_| {
        let assets = filtered_assets.get();
        let current = selected_asset_id.get_untracked();
        let current_is_visible = current
            .as_ref()
            .is_some_and(|id| assets.iter().any(|asset| asset.id == *id));
        if current_is_visible {
            return;
        }
        set_selected_asset_id.set(assets.first().map(|asset| asset.id.clone()));
    });

    let selected_asset = Signal::derive(move || {
        let id = selected_asset_id.get()?;
        assets_resource
            .get()
            .and_then(Result::ok)
            .and_then(|response| response.items.into_iter().find(|asset| asset.id == id))
    });

    let asset_count = Memo::new(move |_| {
        assets_resource
            .get()
            .and_then(Result::ok)
            .map(|response| response.total)
            .unwrap_or_default()
    });

    let open_picker = Callback::new(move |_| {
        if let Some(input) = input_ref.get() {
            input.click();
        }
    });

    let on_upload_change = move |ev: web_sys::Event| {
        let event = Change::from_event(ev);
        let Some(file) = event.files().and_then(|files| files.get(0)) else {
            return;
        };

        set_uploading.set(true);
        leptos::task::spawn_local(async move {
            match api::upload_asset(file).await {
                Ok(response) => {
                    let name = response.record.name.clone();
                    set_selected_asset_id.set(Some(response.record.id));
                    set_asset_refresh_tick.update(|tick| *tick = tick.wrapping_add(1));
                    if let Some(input) = input_ref.get() {
                        input.set_value("");
                    }
                    if response.duplicate {
                        toasts::toast_success(&format!("Selected existing asset: {name}"));
                    } else {
                        toasts::toast_success(&format!("Uploaded asset: {name}"));
                    }
                }
                Err(error) => toasts::toast_error(&format!("Asset upload failed: {error}")),
            }
            set_uploading.set(false);
        });
    };

    let on_layers_mutated = Callback::new(move |()| {
        set_layers_refresh_tick.update(|tick| *tick = tick.wrapping_add(1));
        set_scene_refresh_tick.update(|tick| *tick = tick.wrapping_add(1));
    });

    view! {
        <div class="flex flex-col h-full">
            <PageHeader
                icon=LuFolder
                title="Assets"
                tagline="Media library and scene layers"
                accent=PageAccent::Purple
            >
                <HeaderTrailing slot>
                    <span class="shrink-0 text-[11px] font-mono text-fg-tertiary/55 tabular-nums">
                        {move || {
                            let total = asset_count.get();
                            let filtered = filtered_assets.get().len();
                            if filtered == total {
                                format!("{total} assets")
                            } else {
                                format!("{filtered}/{total} assets")
                            }
                        }}
                    </span>
                    <button
                        type="button"
                        class="flex items-center gap-1.5 px-2.5 py-1.5 rounded-lg text-[11px] font-medium transition-all btn-press shrink-0"
                        style="background: rgba(225, 53, 255, 0.08); border: 1px solid rgba(225, 53, 255, 0.16); color: rgb(225, 53, 255)"
                        on:click=move |_| open_picker.run(())
                    >
                        <Icon icon=LuPlus width="12px" height="12px" />
                        {move || if uploading.get() { "Uploading" } else { "Upload" }}
                    </button>
                </HeaderTrailing>
                <HeaderToolbar slot>
                    <PageSearchBar
                        placeholder="Search assets..."
                        value=search
                        set_value=set_search
                    />
                    <div class="flex items-center gap-1.5 shrink-0">
                        {filter_chips(ASSET_FILTERS, kind_filter, set_kind_filter)}
                    </div>
                </HeaderToolbar>
            </PageHeader>

            <input
                type="file"
                class="hidden"
                accept="image/*,video/*,application/json,.gif,.webp,.png,.jpg,.jpeg,.json"
                node_ref=input_ref
                on:change=on_upload_change
            />

            <div class="flex-1 overflow-hidden">
                <div class="flex h-full">
                    <main class="flex-1 min-w-0 overflow-y-auto px-6 pb-6 pt-4">
                        <Suspense fallback=move || view! { <AssetsLoadingSkeleton /> }>
                            {move || match assets_resource.get() {
                                None => view! { <AssetsLoadingSkeleton /> }.into_any(),
                                Some(Err(error)) => view! {
                                    <EmptyState icon=LuTriangleAlert title="Asset library unavailable" detail=error />
                                }.into_any(),
                                Some(Ok(_)) => {
                                    let assets = filtered_assets.get();
                                    if assets.is_empty() {
                                        view! {
                                            <EmptyState icon=LuFolder title="No matching assets" detail="Upload media or adjust the current filter." />
                                        }.into_any()
                                    } else {
                                        view! {
                                            <div class="grid grid-cols-[repeat(auto-fill,minmax(210px,1fr))] gap-3">
                                                {assets.into_iter().map(|asset| {
                                                    let asset_id = asset.id.clone();
                                                    let is_selected = Signal::derive(move || {
                                                        selected_asset_id.get().as_deref() == Some(asset_id.as_str())
                                                    });
                                                    view! {
                                                        <AssetCard
                                                            asset=asset
                                                            is_selected=is_selected
                                                            on_select=Callback::new(move |id| set_selected_asset_id.set(Some(id)))
                                                        />
                                                    }
                                                }).collect_view()}
                                            </div>
                                        }.into_any()
                                    }
                                }
                            }}
                        </Suspense>
                    </main>

                    <aside class="w-[420px] shrink-0 overflow-y-auto border-l border-edge-subtle/70 bg-surface-sunken/35 px-4 pb-6 pt-4 scrollbar-none">
                        <AssetDetail
                            asset=selected_asset
                            set_asset_refresh_tick=set_asset_refresh_tick
                            set_selected_asset_id=set_selected_asset_id
                        />
                        <LayerPanel
                            active_scene=active_scene
                            selected_group_id=selected_group_id
                            set_selected_group_id=set_selected_group_id
                            layers_resource=layers_resource
                            assets=Signal::derive(move || {
                                assets_resource
                                    .get()
                                    .and_then(Result::ok)
                                    .map(|response| response.items)
                                    .unwrap_or_default()
                            })
                            selected_asset=selected_asset
                            on_layers_mutated=on_layers_mutated
                        />
                    </aside>
                </div>
            </div>
        </div>
    }
}

#[component]
fn AssetCard(
    asset: api::MediaAssetRecord,
    #[prop(into)] is_selected: Signal<bool>,
    on_select: Callback<String>,
) -> impl IntoView {
    let asset_id = asset.id.clone();
    let thumbnail_url = format!("/api/v1/assets/{}/thumbnail", asset.id);
    let kind = asset_kind(&asset).to_owned();
    let dimensions = asset_dimensions(&asset);
    let size = format_bytes(asset.byte_len);

    view! {
        <button
            type="button"
            class="group overflow-hidden rounded-xl border bg-surface-overlay/45 text-left transition-all duration-200 btn-press"
            class=("border-accent-muted", move || is_selected.get())
            class=("shadow-[0_0_24px_rgba(225,53,255,0.13)]", move || is_selected.get())
            class=("border-edge-subtle/70", move || !is_selected.get())
            on:click=move |_| on_select.run(asset_id.clone())
        >
            <div class="aspect-[4/3] bg-surface-sunken/70 relative overflow-hidden">
                <img
                    src=thumbnail_url
                    alt=""
                    class="h-full w-full object-cover opacity-90 transition duration-300 group-hover:scale-[1.025]"
                />
                <div class="absolute left-2 top-2 rounded-full border border-black/20 bg-black/45 px-2 py-0.5 text-[10px] font-mono uppercase tracking-wide text-white/82 backdrop-blur">
                    {kind}
                </div>
            </div>
            <div class="space-y-2 px-3 py-3">
                <div class="min-w-0">
                    <div class="truncate text-sm font-semibold text-fg-primary">{asset.name}</div>
                    <div class="mt-0.5 truncate text-[11px] text-fg-tertiary">{asset.mime_type}</div>
                </div>
                <div class="flex items-center justify-between gap-2 text-[10px] font-mono text-fg-tertiary/70">
                    <span>{size}</span>
                    <span>{dimensions}</span>
                </div>
            </div>
        </button>
    }
}

#[component]
fn AssetDetail(
    #[prop(into)] asset: Signal<Option<api::MediaAssetRecord>>,
    set_asset_refresh_tick: WriteSignal<u64>,
    set_selected_asset_id: WriteSignal<Option<String>>,
) -> impl IntoView {
    let (draft_name, set_draft_name) = signal(String::new());
    let (draft_tags, set_draft_tags) = signal(String::new());

    Effect::new(move |_| {
        if let Some(asset) = asset.get() {
            set_draft_name.set(asset.name);
            set_draft_tags.set(asset.tags.join(", "));
        } else {
            set_draft_name.set(String::new());
            set_draft_tags.set(String::new());
        }
    });

    let save_asset = Callback::new(move |()| {
        let Some(asset) = asset.get_untracked() else {
            return;
        };
        let request = api::AssetUpdateRequest {
            name: Some(draft_name.get_untracked()),
            tags: Some(parse_tags(&draft_tags.get_untracked())),
        };

        leptos::task::spawn_local(async move {
            match api::update_asset(&asset.id, &request).await {
                Ok(updated) => {
                    set_selected_asset_id.set(Some(updated.id));
                    set_asset_refresh_tick.update(|tick| *tick = tick.wrapping_add(1));
                    toasts::toast_success("Asset metadata saved");
                }
                Err(error) => toasts::toast_error(&format!("Asset update failed: {error}")),
            }
        });
    });

    let delete_asset = Callback::new(move |()| {
        let Some(asset) = asset.get_untracked() else {
            return;
        };

        leptos::task::spawn_local(async move {
            match api::delete_asset(&asset.id).await {
                Ok(()) => {
                    set_selected_asset_id.set(None);
                    set_asset_refresh_tick.update(|tick| *tick = tick.wrapping_add(1));
                    toasts::toast_success("Asset removed");
                }
                Err(error) => toasts::toast_error(&format!("Asset delete failed: {error}")),
            }
        });
    });

    view! {
        <section class="rounded-xl border border-edge-subtle/70 bg-surface-overlay/50">
            <div class="flex items-center justify-between gap-3 border-b border-edge-subtle/60 px-4 py-3">
                <div>
                    <div class="text-sm font-semibold text-fg-primary">"Selected Asset"</div>
                    <div class="text-[11px] text-fg-tertiary">"Metadata and file details"</div>
                </div>
                <Icon icon=LuPalette width="16px" height="16px" style="color: rgba(225, 53, 255, 0.7)" />
            </div>
            {move || match asset.get() {
                Some(asset) => view! {
                    <div class="space-y-4 px-4 py-4">
                        <div class="overflow-hidden rounded-lg border border-edge-subtle/60 bg-surface-sunken/60">
                            <img
                                src=format!("/api/v1/assets/{}/thumbnail", asset.id)
                                alt=""
                                class="aspect-video w-full object-cover"
                            />
                        </div>
                        <div class="grid grid-cols-2 gap-2 text-[11px]">
                            <AssetFact label="Type" value=asset_kind(&asset).to_owned() />
                            <AssetFact label="Size" value=format_bytes(asset.byte_len) />
                            <AssetFact label="Pixels" value=asset_dimensions(&asset) />
                            <AssetFact label="Frames" value=asset.frame_count.map(|v| v.to_string()).unwrap_or_else(|| "single".to_owned()) />
                        </div>
                        <label class="block space-y-1.5">
                            <span class="text-[10px] font-mono uppercase tracking-wide text-fg-tertiary/70">"Name"</span>
                            <input
                                type="text"
                                class="w-full rounded-lg border border-edge-subtle bg-surface-sunken/55 px-3 py-2 text-sm text-fg-primary focus:border-accent-muted focus:outline-none"
                                prop:value=move || draft_name.get()
                                on:input=move |event| {
                                    if let Some(value) = Input::from_event(event).value_string() {
                                        set_draft_name.set(value);
                                    }
                                }
                            />
                        </label>
                        <label class="block space-y-1.5">
                            <span class="text-[10px] font-mono uppercase tracking-wide text-fg-tertiary/70">"Tags"</span>
                            <input
                                type="text"
                                class="w-full rounded-lg border border-edge-subtle bg-surface-sunken/55 px-3 py-2 text-sm text-fg-primary focus:border-accent-muted focus:outline-none"
                                prop:value=move || draft_tags.get()
                                on:input=move |event| {
                                    if let Some(value) = Input::from_event(event).value_string() {
                                        set_draft_tags.set(value);
                                    }
                                }
                            />
                        </label>
                        <div class="flex items-center gap-2">
                            <button
                                type="button"
                                class="inline-flex flex-1 items-center justify-center gap-1.5 rounded-lg border border-accent-muted/30 bg-accent/10 px-3 py-2 text-xs font-semibold text-accent transition-colors hover:bg-accent/15"
                                on:click=move |_| save_asset.run(())
                            >
                                <Icon icon=LuSave width="13px" height="13px" />
                                "Save"
                            </button>
                            <button
                                type="button"
                                class="inline-flex items-center justify-center rounded-lg border border-red-400/20 bg-red-400/5 p-2 text-red-300 transition-colors hover:bg-red-400/10"
                                on:click=move |_| delete_asset.run(())
                            >
                                <Icon icon=LuTrash2 width="14px" height="14px" />
                            </button>
                        </div>
                    </div>
                }.into_any(),
                None => view! {
                    <div class="flex flex-col items-center justify-center gap-2 px-4 py-10 text-center">
                        <Icon icon=LuFolder width="28px" height="28px" style="color: rgba(139, 133, 160, 0.35)" />
                        <div class="text-xs text-fg-tertiary/70">"No asset selected"</div>
                    </div>
                }.into_any(),
            }}
        </section>
    }
}

#[component]
fn LayerPanel(
    #[prop(into)] active_scene: Signal<Option<api::ActiveSceneResponse>>,
    selected_group_id: ReadSignal<Option<String>>,
    set_selected_group_id: WriteSignal<Option<String>>,
    layers_resource: LocalResource<Result<api::LayerStackResponse, String>>,
    #[prop(into)] assets: Signal<Vec<api::MediaAssetRecord>>,
    #[prop(into)] selected_asset: Signal<Option<api::MediaAssetRecord>>,
    on_layers_mutated: Callback<()>,
) -> impl IntoView {
    let group_options = Signal::derive(move || {
        active_scene
            .get()
            .map(|scene| {
                scene
                    .groups
                    .into_iter()
                    .map(|group| {
                        let role = group_role_label(group.role);
                        (group.id.to_string(), format!("{} · {role}", group.name))
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    });

    let add_selected_asset = Callback::new(move |()| {
        let Some(asset) = selected_asset.get_untracked() else {
            toasts::toast_error("Select an asset before adding a layer");
            return;
        };
        let Some(scene) = active_scene.get_untracked() else {
            toasts::toast_error("No active scene is available");
            return;
        };
        let Some(group_id) = selected_group_id.get_untracked() else {
            toasts::toast_error("No render group is selected");
            return;
        };
        let Ok(asset_id) = AssetId::from_str(&asset.id) else {
            toasts::toast_error("Selected asset id is invalid");
            return;
        };

        let request = api::CreateLayerRequest {
            name: Some(asset.name.clone()),
            source: LayerSource::Media {
                asset_id,
                playback: MediaPlayback::default(),
            },
            blend: LayerBlendMode::Alpha,
            opacity: 1.0,
            transform: LayerTransform::default(),
            adjust: LayerAdjust::default(),
            bindings: Vec::new(),
            enabled: true,
        };

        leptos::task::spawn_local(async move {
            match api::create_layer(&scene.id, &group_id, &request).await {
                Ok(_) => {
                    on_layers_mutated.run(());
                    toasts::toast_success("Media layer added");
                }
                Err(error) => toasts::toast_error(&format!("Layer add failed: {error}")),
            }
        });
    });

    view! {
        <section class="mt-4 rounded-xl border border-edge-subtle/70 bg-surface-overlay/50">
            <div class="flex items-center justify-between gap-3 border-b border-edge-subtle/60 px-4 py-3">
                <div>
                    <div class="text-sm font-semibold text-fg-primary">"Layer Stack"</div>
                    <div class="text-[11px] text-fg-tertiary">"Active scene render group"</div>
                </div>
                <Icon icon=LuLayers width="16px" height="16px" style="color: rgba(128, 255, 234, 0.72)" />
            </div>
            <div class="space-y-4 px-4 py-4">
                <SilkSelect
                    value=Signal::derive(move || selected_group_id.get().unwrap_or_default())
                    options=group_options
                    on_change=Callback::new(move |id: String| {
                        set_selected_group_id.set((!id.is_empty()).then_some(id));
                    })
                    placeholder="Select group"
                    class="border border-edge-subtle bg-surface-sunken/55 px-3 py-2 text-xs text-fg-primary"
                    label_class="font-medium"
                />

                <button
                    type="button"
                    class="inline-flex w-full items-center justify-center gap-1.5 rounded-lg border border-cyan-300/20 bg-cyan-300/8 px-3 py-2 text-xs font-semibold text-cyan-200 transition-colors hover:bg-cyan-300/12 disabled:cursor-not-allowed disabled:opacity-45"
                    disabled=move || selected_asset.get().is_none() || selected_group_id.get().is_none()
                    on:click=move |_| add_selected_asset.run(())
                >
                    <Icon icon=LuPlus width="13px" height="13px" />
                    {move || {
                        selected_asset
                            .get()
                            .map(|asset| format!("Add {}", asset.name))
                            .unwrap_or_else(|| "Add Selected Asset".to_owned())
                    }}
                </button>

                <Suspense fallback=move || view! { <LayerLoadingSkeleton /> }>
                    {move || match layers_resource.get() {
                        None => view! { <LayerLoadingSkeleton /> }.into_any(),
                        Some(Err(error)) => view! {
                            <div class="rounded-lg border border-red-400/20 bg-red-400/5 px-3 py-3 text-xs text-red-200">
                                {error}
                            </div>
                        }.into_any(),
                        Some(Ok(stack)) if stack.items.is_empty() => view! {
                            <div class="rounded-lg border border-edge-subtle bg-surface-sunken/45 px-3 py-8 text-center text-xs text-fg-tertiary">
                                "No layers in this group"
                            </div>
                        }.into_any(),
                        Some(Ok(stack)) => {
                            let scene_id = active_scene.get().map(|scene| scene.id).unwrap_or_default();
                            let group_id = selected_group_id.get().unwrap_or_default();
                            let asset_records = assets.get();
                            let total = stack.items.len();
                            let mut rows = stack
                                .items
                                .iter()
                                .cloned()
                                .enumerate()
                                .collect::<Vec<_>>();
                            rows.reverse();
                            view! {
                                <div class="space-y-2">
                                    <div class="flex items-center justify-between text-[10px] font-mono uppercase tracking-wide text-fg-tertiary/65">
                                        <span>"Top"</span>
                                        <span>{format!("v{}", stack.layers_version)}</span>
                                    </div>
                                    {rows.into_iter().map(|(stack_index, layer)| {
                                        view! {
                                            <LayerRow
                                                scene_id=scene_id.clone()
                                                group_id=group_id.clone()
                                                layer=layer
                                                stack_index=stack_index
                                                total_layers=total
                                                stack=stack.items.clone()
                                                assets=asset_records.clone()
                                                on_layers_mutated=on_layers_mutated
                                            />
                                        }
                                    }).collect_view()}
                                    <div class="text-[10px] font-mono uppercase tracking-wide text-fg-tertiary/65">
                                        "Bottom"
                                    </div>
                                </div>
                            }.into_any()
                        }
                    }}
                </Suspense>
            </div>
        </section>
    }
}

#[component]
fn LayerRow(
    scene_id: String,
    group_id: String,
    layer: SceneLayer,
    stack_index: usize,
    total_layers: usize,
    stack: Vec<SceneLayer>,
    assets: Vec<api::MediaAssetRecord>,
    on_layers_mutated: Callback<()>,
) -> impl IntoView {
    let source = layer_source_label(&layer.source, &assets);
    let title = layer.name.clone().unwrap_or_else(|| source.clone());
    let layer_id = layer.id.to_string();
    let can_move_up = stack_index + 1 < total_layers;
    let can_move_down = stack_index > 0;
    let enabled = layer.enabled;
    let opacity = layer.opacity;
    let blend = layer.blend;
    let fit = layer.transform.fit;
    let brightness = layer.adjust.brightness;
    let saturation = layer.adjust.saturation;
    let tint_strength = layer.adjust.tint_strength;
    let scale_x = layer.transform.scale[0];
    let scale_y = layer.transform.scale[1];

    let update_enabled_layer = layer.clone();
    let update_blend_layer = layer.clone();
    let update_opacity_layer = layer.clone();
    let update_fit_layer = layer.clone();
    let update_brightness_layer = layer.clone();
    let update_saturation_layer = layer.clone();
    let update_tint_layer = layer.clone();
    let update_scale_x_layer = layer.clone();
    let update_scale_y_layer = layer.clone();
    let delete_layer_id = layer_id.clone();
    let move_up_stack = stack.clone();
    let move_down_stack = stack;
    let scene_enabled = scene_id.clone();
    let group_enabled = group_id.clone();
    let scene_blend = scene_id.clone();
    let group_blend = group_id.clone();
    let scene_opacity = scene_id.clone();
    let group_opacity = group_id.clone();
    let scene_fit = scene_id.clone();
    let group_fit = group_id.clone();
    let scene_brightness = scene_id.clone();
    let group_brightness = group_id.clone();
    let scene_saturation = scene_id.clone();
    let group_saturation = group_id.clone();
    let scene_tint = scene_id.clone();
    let group_tint = group_id.clone();
    let scene_scale_x = scene_id.clone();
    let group_scale_x = group_id.clone();
    let scene_scale_y = scene_id.clone();
    let group_scale_y = group_id.clone();
    let scene_delete = scene_id.clone();
    let group_delete = group_id.clone();
    let scene_move_up = scene_id.clone();
    let group_move_up = group_id.clone();
    let scene_move_down = scene_id.clone();
    let group_move_down = group_id.clone();
    let on_enabled = on_layers_mutated;
    let on_blend = on_layers_mutated;
    let on_opacity = on_layers_mutated;
    let on_fit = on_layers_mutated;
    let on_brightness = on_layers_mutated;
    let on_saturation = on_layers_mutated;
    let on_tint = on_layers_mutated;
    let on_scale_x = on_layers_mutated;
    let on_scale_y = on_layers_mutated;
    let on_delete = on_layers_mutated;
    let on_move_up = on_layers_mutated;
    let on_move_down = on_layers_mutated;

    view! {
        <article class="rounded-xl border border-edge-subtle/70 bg-surface-sunken/45 px-3 py-3">
            <div class="flex items-start justify-between gap-2">
                <div class="min-w-0">
                    <div class="truncate text-sm font-semibold text-fg-primary">{title}</div>
                    <div class="mt-0.5 truncate text-[11px] text-fg-tertiary">{source}</div>
                </div>
                <div class="flex shrink-0 items-center gap-1">
                    <button
                        type="button"
                        class="rounded-md border border-edge-subtle p-1.5 text-fg-tertiary transition-colors hover:text-fg-primary disabled:opacity-30"
                        disabled=!can_move_up
                        on:click=move |_| reorder_layer(scene_move_up.clone(), group_move_up.clone(), move_up_stack.clone(), stack_index, 1, on_move_up)
                    >
                        <Icon icon=LuChevronUp width="13px" height="13px" />
                    </button>
                    <button
                        type="button"
                        class="rounded-md border border-edge-subtle p-1.5 text-fg-tertiary transition-colors hover:text-fg-primary disabled:opacity-30"
                        disabled=!can_move_down
                        on:click=move |_| reorder_layer(scene_move_down.clone(), group_move_down.clone(), move_down_stack.clone(), stack_index, -1, on_move_down)
                    >
                        <Icon icon=LuChevronDown width="13px" height="13px" />
                    </button>
                    <button
                        type="button"
                        class="rounded-md border border-red-400/20 p-1.5 text-red-300 transition-colors hover:bg-red-400/10"
                        on:click=move |_| delete_layer(scene_delete.clone(), group_delete.clone(), delete_layer_id.clone(), on_delete)
                    >
                        <Icon icon=LuTrash2 width="13px" height="13px" />
                    </button>
                </div>
            </div>

            <div class="mt-3 grid grid-cols-[auto_1fr] items-center gap-x-3 gap-y-2">
                <label class="flex items-center gap-2 text-[11px] text-fg-secondary">
                    <input
                        type="checkbox"
                        class="accent-accent"
                        prop:checked=enabled
                        on:change=move |event| {
                            if let Some(checked) = Change::from_event(event).checked() {
                                let mut next = update_enabled_layer.clone();
                                next.enabled = checked;
                                update_layer(scene_enabled.clone(), group_enabled.clone(), next, on_enabled);
                            }
                        }
                    />
                    "Enabled"
                </label>
                <SilkSelect
                    value=Signal::derive(move || blend_value(blend).to_owned())
                    options=Signal::derive(blend_options)
                    on_change=Callback::new(move |value: String| {
                        let mut next = update_blend_layer.clone();
                        next.blend = parse_blend(&value);
                        update_layer(scene_blend.clone(), group_blend.clone(), next, on_blend);
                    })
                    placeholder="Blend"
                    class="border border-edge-subtle bg-surface-overlay/45 px-2.5 py-1.5 text-[11px] text-fg-primary"
                    label_class="font-mono"
                />
                <span class="text-[10px] font-mono uppercase tracking-wide text-fg-tertiary/70">"Opacity"</span>
                <input
                    type="range"
                    min="0"
                    max="1"
                    step="0.01"
                    class="w-full accent-accent"
                    prop:value=format!("{opacity:.2}")
                    on:change=move |event| {
                        if let Some(value) = Change::from_event(event).value::<f32>() {
                            let mut next = update_opacity_layer.clone();
                            next.opacity = value.clamp(0.0, 1.0);
                            update_layer(scene_opacity.clone(), group_opacity.clone(), next, on_opacity);
                        }
                    }
                />
            </div>

            <details class="mt-3 rounded-lg border border-edge-subtle/60 bg-surface-overlay/25">
                <summary class="cursor-pointer px-3 py-2 text-[11px] font-semibold text-fg-secondary">
                    "Transform & Color"
                </summary>
                <div class="space-y-3 border-t border-edge-subtle/50 px-3 py-3">
                    <SilkSelect
                        value=Signal::derive(move || fit_value(fit).to_owned())
                        options=Signal::derive(fit_options)
                        on_change=Callback::new(move |value: String| {
                            let mut next = update_fit_layer.clone();
                            next.transform.fit = parse_fit(&value);
                            update_layer(scene_fit.clone(), group_fit.clone(), next, on_fit);
                        })
                        placeholder="Fit"
                        class="border border-edge-subtle bg-surface-sunken/55 px-2.5 py-1.5 text-[11px] text-fg-primary"
                        label_class="font-mono"
                    />
                    <LayerSlider
                        label="Brightness"
                        value=brightness
                        min=0.0
                        max=4.0
                        step=0.05
                        on_change=Callback::new(move |value: f32| {
                            let mut next = update_brightness_layer.clone();
                            next.adjust.brightness = value.clamp(0.0, 4.0);
                            update_layer(scene_brightness.clone(), group_brightness.clone(), next, on_brightness);
                        })
                    />
                    <LayerSlider
                        label="Saturation"
                        value=saturation
                        min=0.0
                        max=4.0
                        step=0.05
                        on_change=Callback::new(move |value: f32| {
                            let mut next = update_saturation_layer.clone();
                            next.adjust.saturation = value.clamp(0.0, 4.0);
                            update_layer(scene_saturation.clone(), group_saturation.clone(), next, on_saturation);
                        })
                    />
                    <LayerSlider
                        label="Tint"
                        value=tint_strength
                        min=0.0
                        max=1.0
                        step=0.01
                        on_change=Callback::new(move |value: f32| {
                            let mut next = update_tint_layer.clone();
                            next.adjust.tint_strength = value.clamp(0.0, 1.0);
                            update_layer(scene_tint.clone(), group_tint.clone(), next, on_tint);
                        })
                    />
                    <LayerSlider
                        label="Scale X"
                        value=scale_x
                        min=0.1
                        max=4.0
                        step=0.05
                        on_change=Callback::new(move |value: f32| {
                            let mut next = update_scale_x_layer.clone();
                            next.transform.scale[0] = value.clamp(0.1, 4.0);
                            update_layer(scene_scale_x.clone(), group_scale_x.clone(), next, on_scale_x);
                        })
                    />
                    <LayerSlider
                        label="Scale Y"
                        value=scale_y
                        min=0.1
                        max=4.0
                        step=0.05
                        on_change=Callback::new(move |value: f32| {
                            let mut next = update_scale_y_layer.clone();
                            next.transform.scale[1] = value.clamp(0.1, 4.0);
                            update_layer(scene_scale_y.clone(), group_scale_y.clone(), next, on_scale_y);
                        })
                    />
                </div>
            </details>
        </article>
    }
}

#[component]
fn LayerSlider(
    label: &'static str,
    value: f32,
    min: f32,
    max: f32,
    step: f32,
    on_change: Callback<f32>,
) -> impl IntoView {
    view! {
        <label class="grid grid-cols-[78px_1fr_38px] items-center gap-2 text-[10px] font-mono text-fg-tertiary/75">
            <span class="uppercase tracking-wide">{label}</span>
            <input
                type="range"
                min=min.to_string()
                max=max.to_string()
                step=step.to_string()
                class="w-full accent-accent"
                prop:value=format!("{value:.2}")
                on:change=move |event| {
                    if let Some(value) = Change::from_event(event).value::<f32>() {
                        on_change.run(value);
                    }
                }
            />
            <span class="text-right">{format!("{value:.2}")}</span>
        </label>
    }
}

#[component]
fn AssetFact(label: &'static str, value: String) -> impl IntoView {
    view! {
        <div class="rounded-lg border border-edge-subtle/55 bg-surface-sunken/45 px-2.5 py-2">
            <div class="font-mono text-[9px] uppercase tracking-wide text-fg-tertiary/55">{label}</div>
            <div class="mt-1 truncate text-xs font-medium text-fg-secondary">{value}</div>
        </div>
    }
}

#[component]
fn EmptyState(
    icon: icondata_core::Icon,
    title: &'static str,
    detail: impl Into<String>,
) -> impl IntoView {
    let detail = detail.into();
    view! {
        <div class="flex flex-col items-center justify-center py-20 text-center">
            <Icon icon=icon width="36px" height="36px" style="color: rgba(139, 133, 160, 0.35)" />
            <div class="mt-3 text-sm font-semibold text-fg-secondary">{title}</div>
            <div class="mt-1 max-w-xs text-xs text-fg-tertiary/70">{detail}</div>
        </div>
    }
}

#[component]
fn AssetsLoadingSkeleton() -> impl IntoView {
    view! {
        <div class="grid grid-cols-[repeat(auto-fill,minmax(210px,1fr))] gap-3">
            {(0..8).map(|_| view! {
                <div class="h-[220px] rounded-xl border border-edge-subtle/50 bg-surface-overlay/30 animate-pulse" />
            }).collect_view()}
        </div>
    }
}

#[component]
fn LayerLoadingSkeleton() -> impl IntoView {
    view! {
        <div class="space-y-2">
            {(0..3).map(|_| view! {
                <div class="h-[96px] rounded-xl border border-edge-subtle/50 bg-surface-sunken/40 animate-pulse" />
            }).collect_view()}
        </div>
    }
}

fn update_layer(
    scene_id: String,
    group_id: String,
    layer: SceneLayer,
    on_layers_mutated: Callback<()>,
) {
    let layer_id = layer.id.to_string();
    let request = api::UpdateLayerRequest::from(&layer);
    leptos::task::spawn_local(async move {
        match api::update_layer(&scene_id, &group_id, &layer_id, &request).await {
            Ok(_) => on_layers_mutated.run(()),
            Err(error) => toasts::toast_error(&format!("Layer update failed: {error}")),
        }
    });
}

fn delete_layer(
    scene_id: String,
    group_id: String,
    layer_id: String,
    on_layers_mutated: Callback<()>,
) {
    leptos::task::spawn_local(async move {
        match api::delete_layer(&scene_id, &group_id, &layer_id).await {
            Ok(_) => {
                on_layers_mutated.run(());
                toasts::toast_success("Layer removed");
            }
            Err(error) => toasts::toast_error(&format!("Layer delete failed: {error}")),
        }
    });
}

fn reorder_layer(
    scene_id: String,
    group_id: String,
    stack: Vec<SceneLayer>,
    index: usize,
    delta: isize,
    on_layers_mutated: Callback<()>,
) {
    let Some(target) = index.checked_add_signed(delta) else {
        return;
    };
    if target >= stack.len() {
        return;
    }

    let mut layer_ids = stack.iter().map(|layer| layer.id).collect::<Vec<_>>();
    layer_ids.swap(index, target);
    leptos::task::spawn_local(async move {
        match api::reorder_layers(&scene_id, &group_id, layer_ids).await {
            Ok(_) => on_layers_mutated.run(()),
            Err(error) => toasts::toast_error(&format!("Layer reorder failed: {error}")),
        }
    });
}

fn asset_kind(asset: &api::MediaAssetRecord) -> &'static str {
    let mime = asset.mime_type.to_lowercase();
    if mime == "image/gif" {
        "gif"
    } else if mime.starts_with("image/") {
        "image"
    } else if mime.starts_with("video/") {
        "video"
    } else if mime == "application/json" || asset.name.to_lowercase().ends_with(".json") {
        "lottie"
    } else {
        "other"
    }
}

fn asset_dimensions(asset: &api::MediaAssetRecord) -> String {
    match (asset.intrinsic_width, asset.intrinsic_height) {
        (Some(width), Some(height)) => format!("{width}x{height}"),
        _ => "unknown".to_owned(),
    }
}

fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let bytes = bytes as f64;
    if bytes >= GB {
        format!("{:.1} GB", bytes / GB)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes / MB)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes / KB)
    } else {
        format!("{} B", bytes as u64)
    }
}

fn parse_tags(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|tag| !tag.is_empty())
        .map(str::to_owned)
        .collect()
}

fn group_role_label(role: RenderGroupRole) -> &'static str {
    match role {
        RenderGroupRole::Custom => "Custom",
        RenderGroupRole::Primary => "Primary",
        RenderGroupRole::Display => "Display",
    }
}

fn layer_source_label(source: &LayerSource, assets: &[api::MediaAssetRecord]) -> String {
    match source {
        LayerSource::Effect { effect_id, .. } => format!("Effect {effect_id}"),
        LayerSource::Media { asset_id, .. } => {
            let id = asset_id.to_string();
            assets
                .iter()
                .find(|asset| asset.id == id)
                .map(|asset| format!("Media {}", asset.name))
                .unwrap_or_else(|| format!("Media {asset_id}"))
        }
        LayerSource::ScreenRegion { .. } => "Screen region".to_owned(),
        LayerSource::WebViewport { url, .. } => format!("Web {url}"),
        LayerSource::ColorFill { .. } => "Color fill".to_owned(),
    }
}

fn blend_value(mode: LayerBlendMode) -> &'static str {
    match mode {
        LayerBlendMode::Replace => "replace",
        LayerBlendMode::Alpha => "alpha",
        LayerBlendMode::Add => "add",
        LayerBlendMode::Screen => "screen",
        LayerBlendMode::Multiply => "multiply",
        LayerBlendMode::Overlay => "overlay",
        LayerBlendMode::SoftLight => "soft_light",
        LayerBlendMode::ColorDodge => "color_dodge",
        LayerBlendMode::Difference => "difference",
        LayerBlendMode::Tint => "tint",
        LayerBlendMode::LumaReveal => "luma_reveal",
    }
}

fn parse_blend(value: &str) -> LayerBlendMode {
    match value {
        "replace" => LayerBlendMode::Replace,
        "add" => LayerBlendMode::Add,
        "screen" => LayerBlendMode::Screen,
        "multiply" => LayerBlendMode::Multiply,
        "overlay" => LayerBlendMode::Overlay,
        "soft_light" => LayerBlendMode::SoftLight,
        "color_dodge" => LayerBlendMode::ColorDodge,
        "difference" => LayerBlendMode::Difference,
        "tint" => LayerBlendMode::Tint,
        "luma_reveal" => LayerBlendMode::LumaReveal,
        _ => LayerBlendMode::Alpha,
    }
}

fn blend_options() -> Vec<(String, String)> {
    [
        ("alpha", "Alpha"),
        ("replace", "Replace"),
        ("add", "Add"),
        ("screen", "Screen"),
        ("multiply", "Multiply"),
        ("overlay", "Overlay"),
        ("soft_light", "Soft Light"),
        ("color_dodge", "Color Dodge"),
        ("difference", "Difference"),
        ("tint", "Tint"),
        ("luma_reveal", "Luma Reveal"),
    ]
    .into_iter()
    .map(|(value, label)| (value.to_owned(), label.to_owned()))
    .collect()
}

fn fit_value(mode: FitMode) -> &'static str {
    match mode {
        FitMode::Contain => "contain",
        FitMode::Cover => "cover",
        FitMode::Stretch => "stretch",
        FitMode::Tile => "tile",
        FitMode::Mirror => "mirror",
    }
}

fn parse_fit(value: &str) -> FitMode {
    match value {
        "contain" => FitMode::Contain,
        "stretch" => FitMode::Stretch,
        "tile" => FitMode::Tile,
        "mirror" => FitMode::Mirror,
        _ => FitMode::Cover,
    }
}

fn fit_options() -> Vec<(String, String)> {
    [
        ("cover", "Cover"),
        ("contain", "Contain"),
        ("stretch", "Stretch"),
        ("tile", "Tile"),
        ("mirror", "Mirror"),
    ]
    .into_iter()
    .map(|(value, label)| (value.to_owned(), label.to_owned()))
    .collect()
}
