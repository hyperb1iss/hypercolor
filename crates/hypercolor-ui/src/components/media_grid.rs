//! Shared media catalog grid (Spec 65 §7).
//!
//! One responsive grid of media cards, used by both the `/media` catalog
//! page and the Add-layer picker's Media tab — so the two browsers cannot
//! drift. Callers own selection and empty-state handling; the grid only
//! lays out the cards and reports clicks.

use leptos::prelude::*;
use leptos_icons::Icon;

use crate::api;
use crate::icons::*;

/// Responsive grid of media cards. `selected_id` drives the highlight;
/// pass an always-`None` signal where selection is not meaningful (the
/// picker, where a click is an immediate add).
#[component]
pub fn MediaGrid(
    #[prop(into)] assets: Signal<Vec<api::MediaAssetRecord>>,
    #[prop(into)] selected_id: Signal<Option<String>>,
    on_select: Callback<String>,
) -> impl IntoView {
    view! {
        <div class="grid grid-cols-[repeat(auto-fill,minmax(210px,1fr))] gap-3">
            {move || {
                assets
                    .get()
                    .into_iter()
                    .map(|asset| {
                        let asset_id = asset.id.clone();
                        let is_selected = Signal::derive(move || {
                            selected_id.get().as_deref() == Some(asset_id.as_str())
                        });
                        view! {
                            <AssetCard asset=asset is_selected=is_selected on_select=on_select />
                        }
                    })
                    .collect_view()
            }}
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
            class="group overflow-hidden rounded-xl border bg-surface-overlay/45 text-left transition-all duration-200 btn-press card-hover"
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

/// Empty-state placeholder for the media grid — shown by callers when the
/// filtered asset list is empty.
#[component]
pub fn MediaGridEmpty(#[prop(into)] title: String, #[prop(into)] detail: String) -> impl IntoView {
    view! {
        <div class="flex flex-col items-center justify-center py-20 text-center">
            <Icon icon=LuFolder width="36px" height="36px" style="color: rgba(139, 133, 160, 0.35)" />
            <div class="mt-3 text-sm font-semibold text-fg-secondary">{title}</div>
            <div class="mt-1 max-w-xs text-xs text-fg-tertiary/70">{detail}</div>
        </div>
    }
}

/// Coarse media kind from a record's MIME type, for the card badge and
/// the catalog filter: `image` / `gif` / `video` / `lottie` / `other`.
#[must_use]
pub fn asset_kind(asset: &api::MediaAssetRecord) -> &'static str {
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

/// `WIDTHxHEIGHT` for a record's intrinsic pixel size, or `unknown`.
#[must_use]
pub fn asset_dimensions(asset: &api::MediaAssetRecord) -> String {
    match (asset.intrinsic_width, asset.intrinsic_height) {
        (Some(width), Some(height)) => format!("{width}x{height}"),
        _ => "unknown".to_owned(),
    }
}

/// Human-readable byte size (`1.4 MB`).
#[must_use]
pub fn format_bytes(bytes: u64) -> String {
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

/// Human-readable clip duration from a microsecond count (`4.2s`, `1m 03s`).
#[must_use]
pub fn format_duration(micros: u64) -> String {
    let seconds = micros as f64 / 1_000_000.0;
    if seconds >= 60.0 {
        let minutes = (seconds / 60.0).floor();
        let remainder = seconds - minutes * 60.0;
        format!("{minutes:.0}m {remainder:02.0}s")
    } else {
        format!("{seconds:.1}s")
    }
}
