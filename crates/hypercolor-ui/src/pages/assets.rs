//! `/assets` — user media library and active scene layer stack.

use hypercolor_leptos_ext::events::{Change, Input};
use hypercolor_types::scene::ZoneRole;
use leptos::html;
use leptos::prelude::*;
use leptos_icons::Icon;

use crate::api;
use crate::components::layer_panel::LayerPanel;
use crate::components::page_header::{HeaderToolbar, HeaderTrailing, PageAccent, PageHeader};
use crate::components::page_search_bar::PageSearchBar;
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
            .find(|group| group.role != ZoneRole::Display)
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
        assets.sort_by_key(|asset| asset.name.to_lowercase());
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
