//! `/media` — the media catalog page (Spec 65 §7).
//!
//! The catalog half of the old `/assets` page, kept and polished: a
//! responsive thumbnail grid with upload, search, MIME filter, and a
//! per-item detail panel. Composition lives in Studio; this page only
//! manages the library. The shared [`MediaGrid`] is also embedded as the
//! Add-layer picker's Media tab.

use hypercolor_leptos_ext::events::{Change, Input};
use leptos::html;
use leptos::prelude::*;
use leptos_icons::Icon;

use crate::api;
use crate::components::media_grid::{MediaGrid, MediaGridEmpty, asset_kind};
use crate::components::media_kind::{
    format_bytes, format_duration, format_timecode, kind_accent, kind_icon, kind_label,
};
use crate::components::media_preview::{MediaPreview, VideoMeta};
use crate::components::page_header::{HeaderToolbar, HeaderTrailing, PageAccent, PageHeader};
use crate::components::page_search_bar::PageSearchBar;
use crate::icons::*;
use crate::style_utils::filter_chips;
use crate::toasts;

const MEDIA_FILTERS: &[(&str, &str)] = &[
    ("all", "255, 106, 193"),
    ("image", "128, 255, 234"),
    ("gif", "255, 106, 193"),
    ("video", "130, 170, 255"),
    ("lottie", "80, 250, 123"),
];

#[component]
pub fn MediaPage() -> impl IntoView {
    let (search, set_search) = signal(String::new());
    let (kind_filter, set_kind_filter) = signal("all".to_owned());
    let (refresh_tick, set_refresh_tick) = signal(0_u64);
    let (selected_id, set_selected_id) = signal(None::<String>);
    let (uploading, set_uploading) = signal(false);
    let input_ref = NodeRef::<html::Input>::new();

    let media_resource = LocalResource::new(move || {
        let _ = refresh_tick.get();
        async move { api::list_assets().await }
    });

    let filtered_media = Memo::new(move |_| {
        let Some(Ok(response)) = media_resource.get() else {
            return Vec::new();
        };
        let query = search.get().to_lowercase();
        let filter = kind_filter.get();
        let mut items = response.items;
        items.retain(|asset| {
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
        items.sort_by_key(|asset| asset.name.to_lowercase());
        items
    });

    // Keep the selection pointed at a still-visible item.
    Effect::new(move |_| {
        let items = filtered_media.get();
        let current = selected_id.get_untracked();
        let still_visible = current
            .as_ref()
            .is_some_and(|id| items.iter().any(|asset| asset.id == *id));
        if !still_visible {
            set_selected_id.set(items.first().map(|asset| asset.id.clone()));
        }
    });

    let selected_asset = Signal::derive(move || {
        let id = selected_id.get()?;
        media_resource
            .get()
            .and_then(Result::ok)
            .and_then(|response| response.items.into_iter().find(|asset| asset.id == id))
    });

    let total_count = Memo::new(move |_| {
        media_resource
            .get()
            .and_then(Result::ok)
            .map(|response| response.total)
            .unwrap_or_default()
    });

    let open_file_picker = Callback::new(move |_| {
        if let Some(input) = input_ref.get() {
            input.click();
        }
    });

    let on_upload_change = move |ev: web_sys::Event| {
        let Some(file) = Change::from_event(ev)
            .files()
            .and_then(|files| files.get(0))
        else {
            return;
        };
        set_uploading.set(true);
        leptos::task::spawn_local(async move {
            match api::upload_asset(file).await {
                Ok(response) => {
                    let name = response.record.name.clone();
                    set_selected_id.set(Some(response.record.id));
                    set_refresh_tick.update(|tick| *tick = tick.wrapping_add(1));
                    if let Some(input) = input_ref.get() {
                        input.set_value("");
                    }
                    if response.duplicate {
                        toasts::toast_success(&format!("Selected existing media: {name}"));
                    } else {
                        toasts::toast_success(&format!("Uploaded media: {name}"));
                    }
                }
                Err(error) => toasts::toast_error(&format!("Media upload failed: {error}")),
            }
            set_uploading.set(false);
        });
    };

    let on_changed = Callback::new(move |()| {
        set_refresh_tick.update(|tick| *tick = tick.wrapping_add(1));
    });

    view! {
        <div class="flex h-full flex-col">
            <PageHeader
                icon=LuFolder
                title="Media"
                tagline="Upload and organize composition media"
                accent=PageAccent::Coral
            >
                <HeaderTrailing slot>
                    <span class="shrink-0 text-[11px] font-mono text-fg-tertiary/55 tabular-nums">
                        {move || {
                            let total = total_count.get();
                            let shown = filtered_media.get().len();
                            if shown == total {
                                format!("{total} files")
                            } else {
                                format!("{shown}/{total} files")
                            }
                        }}
                    </span>
                    <button
                        type="button"
                        class="flex shrink-0 items-center gap-1.5 rounded-lg px-2.5 py-1.5 text-[11px] font-medium transition-all btn-press"
                        style="background: rgba(255, 106, 193, 0.08); border: 1px solid rgba(255, 106, 193, 0.16); color: rgb(255, 106, 193)"
                        on:click=move |_| open_file_picker.run(())
                    >
                        <Icon icon=LuPlus width="12px" height="12px" />
                        {move || if uploading.get() { "Uploading" } else { "Upload" }}
                    </button>
                </HeaderTrailing>
                <HeaderToolbar slot>
                    <PageSearchBar placeholder="Search media..." value=search set_value=set_search />
                    <div class="flex shrink-0 items-center gap-1.5">
                        {filter_chips(MEDIA_FILTERS, kind_filter, set_kind_filter)}
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
                    <main class="min-w-0 flex-1 overflow-y-auto px-6 pb-6 pt-4">
                        <Suspense fallback=move || view! { <MediaLoadingSkeleton /> }>
                            {move || match media_resource.get() {
                                None => view! { <MediaLoadingSkeleton /> }.into_any(),
                                Some(Err(error)) => view! {
                                    <MediaGridEmpty title="Media library unavailable" detail=error />
                                }.into_any(),
                                Some(Ok(_)) => {
                                    if filtered_media.get().is_empty() {
                                        view! {
                                            <MediaGridEmpty
                                                title="No matching media"
                                                detail="Upload a file or adjust the current filter."
                                            />
                                        }.into_any()
                                    } else {
                                        view! {
                                            <MediaGrid
                                                assets=filtered_media
                                                selected_id=selected_id
                                                on_select=Callback::new(move |id| set_selected_id.set(Some(id)))
                                            />
                                        }.into_any()
                                    }
                                }
                            }}
                        </Suspense>
                    </main>

                    <aside class="w-[380px] shrink-0 overflow-y-auto border-l border-edge-subtle/70 bg-surface-sunken/35 px-4 pb-6 pt-4 scrollbar-none">
                        <MediaDetail asset=selected_asset on_changed=on_changed />
                    </aside>
                </div>
            </div>
        </div>
    }
}

#[component]
fn MediaDetail(
    #[prop(into)] asset: Signal<Option<api::MediaAssetRecord>>,
    on_changed: Callback<()>,
) -> impl IntoView {
    let (draft_name, set_draft_name) = signal(String::new());
    let (draft_tags, set_draft_tags) = signal(String::new());
    let video_meta = RwSignal::new(None::<VideoMeta>);

    Effect::new(move |_| {
        match asset.get() {
            Some(asset) => {
                set_draft_name.set(asset.name);
                set_draft_tags.set(asset.tags.join(", "));
            }
            None => {
                set_draft_name.set(String::new());
                set_draft_tags.set(String::new());
            }
        }
        // A fresh selection forgets any video metadata probed from the player.
        video_meta.set(None);
    });

    let save = Callback::new(move |()| {
        let Some(asset) = asset.get_untracked() else {
            return;
        };
        let request = api::AssetUpdateRequest {
            name: Some(draft_name.get_untracked()),
            tags: Some(parse_tags(&draft_tags.get_untracked())),
        };
        leptos::task::spawn_local(async move {
            match api::update_asset(&asset.id, &request).await {
                Ok(_) => {
                    on_changed.run(());
                    toasts::toast_success("Media metadata saved");
                }
                Err(error) => toasts::toast_error(&format!("Media update failed: {error}")),
            }
        });
    });

    let delete = Callback::new(move |()| {
        let Some(asset) = asset.get_untracked() else {
            return;
        };
        leptos::task::spawn_local(async move {
            match api::delete_asset(&asset.id).await {
                Ok(()) => {
                    on_changed.run(());
                    toasts::toast_success("Media removed");
                }
                Err(error) => toasts::toast_error(&format!("Media delete failed: {error}")),
            }
        });
    });

    let on_video_loaded = Callback::new(move |meta: VideoMeta| video_meta.set(Some(meta)));

    view! {
        <section class="overflow-hidden rounded-xl border border-edge-subtle/70 bg-surface-overlay/50">
            {move || match asset.get() {
                Some(asset) => {
                    let kind = asset_kind(&asset);
                    let accent = kind_accent(kind);
                    let icon = kind_icon(kind);
                    let label = kind_label(kind);
                    let blob_url = format!("/api/v1/assets/{}/blob", asset.id);
                    let header_name = asset.name.clone();
                    let download_name = asset.name.clone();
                    let type_text = asset.mime_type.clone();
                    let size_text = format_bytes(asset.byte_len);
                    let frames_text = asset
                        .frame_count
                        .map(|count| count.to_string())
                        .unwrap_or_else(|| "single".to_owned());
                    let hash_text = asset
                        .hash_sha256
                        .get(..12)
                        .map(|prefix| format!("{prefix}…"))
                        .unwrap_or_else(|| "unknown".to_owned());
                    let asset_dims = (asset.intrinsic_width, asset.intrinsic_height);
                    let asset_duration_us = asset.duration_us;

                    let pixels = move || match asset_dims {
                        (Some(width), Some(height)) => format!("{width}×{height}"),
                        _ => video_meta
                            .get()
                            .map(|meta| format!("{}×{}", meta.width, meta.height))
                            .unwrap_or_else(|| "—".to_owned()),
                    };
                    let duration_value = move || {
                        if let Some(micros) = asset_duration_us {
                            format_duration(micros)
                        } else if let Some(meta) = video_meta.get() {
                            format_timecode(meta.duration_secs)
                        } else if kind == "video" {
                            "—".to_owned()
                        } else {
                            "still".to_owned()
                        }
                    };

                    view! {
                        <div
                            class="flex items-center gap-2.5 border-b border-edge-subtle/55 px-4 py-3"
                            style=format!("background: linear-gradient(180deg, rgba({accent}, 0.08), transparent)")
                        >
                            <div
                                class="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg"
                                style=format!("background: rgba({accent}, 0.14); border: 1px solid rgba({accent}, 0.3)")
                            >
                                <Icon icon=icon width="15px" height="15px" style=format!("color: rgba({accent}, 0.95)") />
                            </div>
                            <div class="min-w-0 flex-1">
                                <div class="truncate text-sm font-semibold text-fg-primary">{header_name}</div>
                                <div class="text-[11px] font-medium" style=format!("color: rgba({accent}, 0.85)")>
                                    {label}
                                </div>
                            </div>
                        </div>

                        <div class="space-y-4 px-4 py-4">
                            <MediaPreview asset=asset on_video_loaded=Some(on_video_loaded) />

                            <div class="grid grid-cols-2 gap-2 text-[11px]">
                                <MediaFact label="Type" value=Signal::derive(move || type_text.clone()) />
                                <MediaFact label="Size" value=Signal::derive(move || size_text.clone()) />
                                <MediaFact label="Pixels" value=Signal::derive(pixels) />
                                <MediaFact label="Frames" value=Signal::derive(move || frames_text.clone()) />
                                <MediaFact label="Duration" value=Signal::derive(duration_value) />
                                <MediaFact label="Hash" value=Signal::derive(move || hash_text.clone()) />
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
                                    class="inline-flex flex-1 items-center justify-center gap-1.5 rounded-lg border border-accent-muted/30 bg-accent/10 px-3 py-2 text-xs font-semibold text-accent transition-colors hover:bg-accent/15 btn-press"
                                    on:click=move |_| save.run(())
                                >
                                    <Icon icon=LuSave width="13px" height="13px" />
                                    "Save"
                                </button>
                                <a
                                    href=blob_url
                                    download=download_name
                                    title="Download original"
                                    class="inline-flex items-center justify-center rounded-lg border border-edge-subtle bg-surface-sunken/45 p-2 text-fg-secondary transition-colors hover:bg-surface-hover/30 hover:text-fg-primary btn-press"
                                >
                                    <Icon icon=LuDownload width="14px" height="14px" />
                                </a>
                                <button
                                    type="button"
                                    class="inline-flex items-center justify-center rounded-lg border border-red-400/20 bg-red-400/5 p-2 text-red-300 transition-colors hover:bg-red-400/10 btn-press"
                                    on:click=move |_| delete.run(())
                                >
                                    <Icon icon=LuTrash2 width="14px" height="14px" />
                                </button>
                            </div>
                        </div>
                    }
                    .into_any()
                }
                None => view! {
                    <div class="flex flex-col items-center justify-center gap-2 px-4 py-10 text-center">
                        <Icon icon=LuFolder width="28px" height="28px" style="color: rgba(139, 133, 160, 0.35)" />
                        <div class="text-xs text-fg-tertiary/70">"No media selected"</div>
                    </div>
                }.into_any(),
            }}
        </section>
    }
}

#[component]
fn MediaFact(label: &'static str, #[prop(into)] value: Signal<String>) -> impl IntoView {
    view! {
        <div class="rounded-lg border border-edge-subtle/55 bg-surface-sunken/45 px-2.5 py-2">
            <div class="font-mono text-[9px] uppercase tracking-wide text-fg-tertiary/55">{label}</div>
            <div class="mt-1 truncate text-xs font-medium text-fg-secondary">{move || value.get()}</div>
        </div>
    }
}

#[component]
fn MediaLoadingSkeleton() -> impl IntoView {
    view! {
        <div class="grid grid-cols-[repeat(auto-fill,minmax(212px,1fr))] gap-3.5">
            {(0..8).map(|_| view! {
                <div class="aspect-[4/3] rounded-xl border border-edge-subtle/50 bg-surface-overlay/30 animate-pulse" />
            }).collect_view()}
        </div>
    }
}

fn parse_tags(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|tag| !tag.is_empty())
        .map(str::to_owned)
        .collect()
}
