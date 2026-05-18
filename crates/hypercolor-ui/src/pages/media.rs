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
use crate::components::media_grid::{
    MediaGrid, MediaGridEmpty, asset_dimensions, asset_kind, format_bytes, format_duration,
};
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

    Effect::new(move |_| {
        if let Some(asset) = asset.get() {
            set_draft_name.set(asset.name);
            set_draft_tags.set(asset.tags.join(", "));
        } else {
            set_draft_name.set(String::new());
            set_draft_tags.set(String::new());
        }
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

    view! {
        <section class="rounded-xl border border-edge-subtle/70 bg-surface-overlay/50">
            <div class="flex items-center justify-between gap-3 border-b border-edge-subtle/60 px-4 py-3">
                <div>
                    <div class="text-sm font-semibold text-fg-primary">"Selected Media"</div>
                    <div class="text-[11px] text-fg-tertiary">"File details and metadata"</div>
                </div>
                <Icon icon=LuFolder width="16px" height="16px" style="color: rgba(255, 106, 193, 0.7)" />
            </div>
            {move || match asset.get() {
                Some(asset) => {
                    let duration = asset.duration_us.map(format_duration);
                    let hash = asset
                        .hash_sha256
                        .get(..12)
                        .map(|prefix| format!("{prefix}…"))
                        .unwrap_or_else(|| "unknown".to_owned());
                    view! {
                        <div class="space-y-4 px-4 py-4">
                            <div class="overflow-hidden rounded-lg border border-edge-subtle/60 bg-surface-sunken/60">
                                <img
                                    src=format!("/api/v1/assets/{}/thumbnail", asset.id)
                                    alt=""
                                    class="aspect-video w-full object-cover"
                                />
                            </div>
                            <div class="grid grid-cols-2 gap-2 text-[11px]">
                                <MediaFact label="Type" value=asset_kind(&asset).to_owned() />
                                <MediaFact label="Size" value=format_bytes(asset.byte_len) />
                                <MediaFact label="Pixels" value=asset_dimensions(&asset) />
                                <MediaFact
                                    label="Frames"
                                    value=asset.frame_count.map(|count| count.to_string()).unwrap_or_else(|| "single".to_owned())
                                />
                                <MediaFact label="Duration" value=duration.unwrap_or_else(|| "still".to_owned()) />
                                <MediaFact label="Hash" value=hash />
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
                                <button
                                    type="button"
                                    class="inline-flex items-center justify-center rounded-lg border border-red-400/20 bg-red-400/5 p-2 text-red-300 transition-colors hover:bg-red-400/10 btn-press"
                                    on:click=move |_| delete.run(())
                                >
                                    <Icon icon=LuTrash2 width="14px" height="14px" />
                                </button>
                            </div>
                        </div>
                    }.into_any()
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
fn MediaFact(label: &'static str, value: String) -> impl IntoView {
    view! {
        <div class="rounded-lg border border-edge-subtle/55 bg-surface-sunken/45 px-2.5 py-2">
            <div class="font-mono text-[9px] uppercase tracking-wide text-fg-tertiary/55">{label}</div>
            <div class="mt-1 truncate text-xs font-medium text-fg-secondary">{value}</div>
        </div>
    }
}

#[component]
fn MediaLoadingSkeleton() -> impl IntoView {
    view! {
        <div class="grid grid-cols-[repeat(auto-fill,minmax(210px,1fr))] gap-3">
            {(0..8).map(|_| view! {
                <div class="h-[220px] rounded-xl border border-edge-subtle/50 bg-surface-overlay/30 animate-pulse" />
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
