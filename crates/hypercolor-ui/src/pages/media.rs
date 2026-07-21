//! `/media` — the media catalog page (Spec 65 §7).
//!
//! The catalog half of the old `/assets` page, kept and polished: a
//! responsive thumbnail grid with upload, search, MIME filter, and a
//! per-item detail panel. Composition lives in Studio; this page only
//! manages the library. The shared [`MediaGrid`] is also embedded as the
//! Add-layer picker's Media tab.
//!
//! The whole page is a drop target: dragging files in raises a purple
//! drop veil, and an empty library renders a drop-zone hero instead of a
//! bare placeholder. Uploads accept multiple files in one gesture.

use hypercolor_leptos_ext::events::{Change, Input};
use leptos::html;
use leptos::prelude::*;
use leptos_icons::Icon;
use wasm_bindgen::JsValue;

use crate::api;
use crate::components::empty_state::EmptyState;
use crate::components::media_grid::{MediaGrid, asset_kind};
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
    ("all", "255, 153, 255"),
    ("image", "128, 255, 234"),
    ("gif", "255, 106, 193"),
    ("video", "130, 170, 255"),
    ("lottie", "80, 250, 123"),
];

/// Whether a drag carries OS files (as opposed to text or in-page drags).
fn drag_has_files(ev: &web_sys::DragEvent) -> bool {
    ev.data_transfer()
        .is_some_and(|dt| dt.types().includes(&JsValue::from_str("Files"), 0))
}

/// All files attached to a drop, oldest-first.
fn dropped_files(ev: &web_sys::DragEvent) -> Vec<web_sys::File> {
    let Some(list) = ev.data_transfer().and_then(|dt| dt.files()) else {
        return Vec::new();
    };
    (0..list.length()).filter_map(|i| list.get(i)).collect()
}

#[component]
pub fn MediaPage() -> impl IntoView {
    let (search, set_search) = signal(String::new());
    let (kind_filter, set_kind_filter) = signal("all".to_owned());
    let (refresh_tick, set_refresh_tick) = signal(0_u64);
    let (selected_id, set_selected_id) = signal(None::<String>);
    let (uploading, set_uploading) = signal(false);
    // Nested dragenter/dragleave pairs from child elements balance out; the
    // veil shows while the depth is positive.
    let (drag_depth, set_drag_depth) = signal(0_i32);
    let is_dragging = Memo::new(move |_| drag_depth.get() > 0);
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

    // The detail rail earns its width only once the library has content;
    // while loading it stays up so the layout does not jump on settle.
    let show_rail = Memo::new(move |_| match media_resource.get() {
        None => true,
        Some(Ok(response)) => response.total > 0,
        Some(Err(_)) => false,
    });

    let open_file_picker = Callback::new(move |_: ()| {
        if let Some(input) = input_ref.get() {
            input.click();
        }
    });

    // Shared by the file picker and the page-wide drop target. Uploads run
    // sequentially; one refresh + selection move at the end keeps the grid
    // from churning mid-batch.
    let upload_files = move |files: Vec<web_sys::File>| {
        if files.is_empty() || uploading.get_untracked() {
            return;
        }
        set_uploading.set(true);
        leptos::task::spawn_local(async move {
            let total = files.len();
            let mut uploaded = 0_usize;
            let mut last: Option<(String, String, bool)> = None;
            for file in files {
                let file_name = file.name();
                match api::upload_asset(file).await {
                    Ok(response) => {
                        uploaded += 1;
                        last = Some((response.record.id, response.record.name, response.duplicate));
                    }
                    Err(error) => {
                        toasts::toast_error(&format!("Upload failed for {file_name}: {error}"));
                    }
                }
            }
            if let Some((id, name, duplicate)) = last {
                set_selected_id.set(Some(id));
                set_refresh_tick.update(|tick| *tick = tick.wrapping_add(1));
                if total == 1 && duplicate {
                    toasts::toast_success(&format!("Selected existing media: {name}"));
                } else if total == 1 {
                    toasts::toast_success(&format!("Uploaded media: {name}"));
                } else {
                    toasts::toast_success(&format!("Uploaded {uploaded} of {total} files"));
                }
            }
            if let Some(input) = input_ref.get() {
                input.set_value("");
            }
            set_uploading.set(false);
        });
    };

    let on_upload_change = move |ev: web_sys::Event| {
        let Some(list) = Change::from_event(ev).files() else {
            return;
        };
        let files: Vec<_> = (0..list.length()).filter_map(|i| list.get(i)).collect();
        upload_files(files);
    };

    let clear_filters = move |_| {
        set_search.set(String::new());
        set_kind_filter.set("all".to_owned());
    };

    let on_changed = Callback::new(move |()| {
        set_refresh_tick.update(|tick| *tick = tick.wrapping_add(1));
    });

    view! {
        <div
            class="relative flex h-full flex-col"
            on:dragenter=move |ev: web_sys::DragEvent| {
                if drag_has_files(&ev) {
                    ev.prevent_default();
                    set_drag_depth.update(|depth| *depth += 1);
                }
            }
            on:dragover=move |ev: web_sys::DragEvent| {
                if drag_has_files(&ev) {
                    ev.prevent_default();
                }
            }
            on:dragleave=move |_| {
                set_drag_depth.update(|depth| *depth = (*depth - 1).max(0));
            }
            on:drop=move |ev: web_sys::DragEvent| {
                ev.prevent_default();
                set_drag_depth.set(0);
                upload_files(dropped_files(&ev));
            }
        >
            <PageHeader
                icon=LuImages
                title="Media"
                tagline="Stills, loops, and clips for your scenes"
                accent=PageAccent::Pink
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
                    // Standard purple-accent secondary treatment (§4: purple
                    // is the only chrome accent) — matches the Effects
                    // header's Install button.
                    <button
                        type="button"
                        class="flex shrink-0 items-center gap-1.5 rounded-lg px-2.5 py-1.5 text-[11px] font-medium transition-all btn-press disabled:opacity-60
                               text-fg-primary bg-surface-overlay/70 border border-edge-subtle hover:border-accent-muted hover:bg-surface-overlay glow-ring"
                        prop:disabled=move || uploading.get()
                        on:click=move |_| open_file_picker.run(())
                    >
                        <span class=move || {
                            if uploading.get() { "inline-flex animate-pulse" } else { "inline-flex" }
                        }>
                            <Icon icon=LuPlus width="12px" height="12px" />
                        </span>
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
                multiple
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
                                    <EmptyState
                                        icon=LuTriangleAlert
                                        title="Media library unavailable"
                                        hint=error
                                    />
                                }.into_any(),
                                Some(Ok(_)) => {
                                    if total_count.get() == 0 {
                                        view! {
                                            <MediaEmptyHero
                                                dragging=is_dragging
                                                uploading=uploading
                                                on_browse=open_file_picker
                                            />
                                        }.into_any()
                                    } else if filtered_media.get().is_empty() {
                                        view! {
                                            <EmptyState
                                                icon=LuSearchX
                                                title="No matching media"
                                                hint="Nothing in your library matches this search or filter."
                                            >
                                                <button
                                                    type="button"
                                                    class="rounded-lg border border-edge-subtle bg-surface-overlay/70 px-3 py-1.5 text-[11px] font-medium text-fg-primary transition-all hover:border-accent-muted hover:bg-surface-overlay btn-press glow-ring"
                                                    on:click=clear_filters
                                                >
                                                    "Clear filters"
                                                </button>
                                            </EmptyState>
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

                    <Show when=move || show_rail.get()>
                        <aside class="w-[380px] shrink-0 overflow-y-auto border-l border-edge-subtle/70 bg-surface-sunken/35 px-4 pb-6 pt-4 scrollbar-none">
                            <MediaDetail asset=selected_asset on_changed=on_changed />
                        </aside>
                    </Show>
                </div>
            </div>

            <Show when=move || is_dragging.get()>
                <div class="pointer-events-none absolute inset-0 z-40 accent-purple">
                    <div
                        class="absolute inset-0"
                        style="background: rgba(var(--glow-rgb), 0.05)"
                    ></div>
                    <div
                        class="absolute inset-4 flex items-center justify-center rounded-xl border-2 border-dashed border-accent-muted/70"
                        style="background: rgba(var(--glow-rgb), 0.06); box-shadow: inset 0 0 70px rgba(var(--glow-rgb), 0.10)"
                    >
                        <div class="flex flex-col items-center gap-3 text-center animate-enter-scale">
                            <div
                                class="flex h-14 w-14 items-center justify-center rounded-xl border border-accent-muted/50 bg-surface-overlay/85 text-accent"
                                style="box-shadow: 0 0 30px rgba(var(--glow-rgb), 0.25)"
                            >
                                <Icon icon=LuUpload width="24px" height="24px" />
                            </div>
                            <div class="text-sm font-semibold text-fg-primary">"Drop to upload"</div>
                            <div class="text-xs text-fg-tertiary">
                                "Images, GIFs, video, and Lottie JSON"
                            </div>
                        </div>
                    </div>
                </div>
            </Show>
        </div>
    }
}

/// Full-width invitation shown when the library holds no media at all: a
/// dashed drop-zone hero with kind-tinted tiles, an upload CTA, and format
/// hints. The kind tiles wear category accents (§4.1 — identity, never
/// chrome); the CTA and drag highlight stay on purple.
#[component]
fn MediaEmptyHero(
    #[prop(into)] dragging: Signal<bool>,
    #[prop(into)] uploading: Signal<bool>,
    on_browse: Callback<()>,
) -> impl IntoView {
    let kind_tile = |kind: &'static str, tilt: &'static str| {
        let rgb = kind_accent(kind);
        view! {
            <div
                class=format!(
                    "flex h-11 w-11 items-center justify-center rounded-xl border {tilt}"
                )
                style=format!(
                    "background: rgba({rgb}, 0.10); border-color: rgba({rgb}, 0.30); \
                     box-shadow: 0 0 20px rgba({rgb}, 0.14); color: rgba({rgb}, 0.9)"
                )
            >
                <Icon icon=kind_icon(kind) width="18px" height="18px" />
            </div>
        }
    };

    view! {
        <div class=move || {
            let state = if dragging.get() {
                "border-accent-muted bg-accent/5"
            } else {
                "border-edge-default/70 bg-surface-overlay/25"
            };
            format!(
                "relative mx-auto mt-6 max-w-2xl overflow-hidden rounded-xl border border-dashed \
                 transition-all duration-300 accent-purple animate-enter-up {state}"
            )
        }>
            <div
                class="pointer-events-none absolute inset-0"
                style="background: radial-gradient(90% 65% at 50% 0%, rgba(var(--glow-rgb), 0.07), transparent 70%)"
            ></div>
            <div class="relative flex flex-col items-center px-8 py-14 text-center">
                <div class="flex items-end gap-3">
                    {kind_tile("video", "-rotate-6 mb-0.5")}
                    <div
                        class="flex h-14 w-14 items-center justify-center rounded-xl border bg-surface-overlay/70 accent-pink"
                        style="box-shadow: 0 0 26px rgba(var(--glow-rgb), 0.16); color: rgba(var(--glow-rgb), 0.9); border-color: rgba(var(--glow-rgb), 0.35)"
                    >
                        <Icon icon=LuImages width="24px" height="24px" />
                    </div>
                    {kind_tile("lottie", "rotate-6 mb-0.5")}
                </div>
                <h2 class="mt-6 text-base font-semibold text-fg-primary">
                    "Your media library is empty"
                </h2>
                <p class="mt-1.5 max-w-sm text-xs leading-relaxed text-fg-tertiary">
                    "Drop files anywhere on this page, or browse from your computer. "
                    "Everything here becomes a layer you can composite in Studio."
                </p>
                <button
                    type="button"
                    class="mt-6 inline-flex items-center gap-2 rounded-lg border border-accent-muted/40 bg-accent/15 px-4 py-2 text-xs font-semibold text-accent transition-colors hover:bg-accent/25 btn-press glow-ring disabled:opacity-60"
                    prop:disabled=move || uploading.get()
                    on:click=move |_| on_browse.run(())
                >
                    <span class=move || {
                        if uploading.get() { "inline-flex animate-pulse" } else { "inline-flex" }
                    }>
                        <Icon icon=LuUpload width="14px" height="14px" />
                    </span>
                    {move || if uploading.get() { "Uploading..." } else { "Upload media" }}
                </button>
                <div class="mt-6 font-mono text-[10px] uppercase tracking-wider text-fg-tertiary/60">
                    "PNG · JPEG · GIF · WebP · MP4 · Lottie JSON"
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
                    <div class="flex flex-col items-center gap-3 px-4 py-12 text-center">
                        <div class="flex h-11 w-11 items-center justify-center rounded-xl border border-edge-subtle/70 bg-surface-sunken/60 text-fg-tertiary/60">
                            <Icon icon=LuImages width="18px" height="18px" />
                        </div>
                        <div>
                            <div class="text-xs font-semibold text-fg-secondary">"Nothing selected"</div>
                            <div class="mx-auto mt-1 max-w-[24ch] text-[11px] leading-relaxed text-fg-tertiary/75">
                                "Choose an item from the grid to preview it and edit its details."
                            </div>
                        </div>
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
