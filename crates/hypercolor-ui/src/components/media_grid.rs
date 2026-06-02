//! Shared media catalog grid (Spec 65 §7).
//!
//! One responsive grid of media cards, used by both the `/media` catalog
//! page and the Add-layer picker's Media tab — so the two browsers cannot
//! drift. Callers own selection and empty-state handling; the grid only
//! lays out the cards and reports clicks.
//!
//! Cards follow the Luminary card pattern (DESIGN-SYSTEM §12.1): a full-bleed
//! hero (thumbnail for stills, a category-tinted gradient for video/lottie
//! which the daemon does not thumbnail), a scrim for legibility, a category
//! gradient on the top edge, and a kind dot + label. Selection is purple
//! chrome (§4) — the per-kind color is identity, never the active state.

use leptos::prelude::*;
use leptos_icons::Icon;

use crate::api;
use crate::components::media_kind::{
    format_bytes, kind_accent, kind_from_mime, kind_has_thumbnail, kind_icon, kind_label,
};
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
        <div class="grid grid-cols-[repeat(auto-fill,minmax(212px,1fr))] gap-3.5">
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
    let kind = asset_kind(&asset);
    let accent = kind_accent(kind);
    let icon = kind_icon(kind);
    let label = kind_label(kind);
    let has_thumb = kind_has_thumbnail(kind);
    let thumbnail_url = format!("/api/v1/assets/{}/thumbnail", asset.id);
    let meta_line = format!("{} · {}", format_bytes(asset.byte_len), card_meta_detail(&asset, kind));
    let name = asset.name.clone();

    view! {
        <button
            type="button"
            class=move || {
                let base = "group relative block aspect-[4/3] w-full overflow-hidden rounded-xl \
                            border text-left card-hover content-auto-card";
                let state = if is_selected.get() {
                    "border-electric-purple/50"
                } else {
                    "border-edge-subtle hover:border-edge-default"
                };
                format!("{base} {state}")
            }
            style:--glow-rgb=accent
            on:click=move |_| on_select.run(asset_id.clone())
        >
            {if has_thumb {
                view! {
                    <img
                        class="absolute inset-0 h-full w-full scale-[1.02] object-cover transition-transform duration-500 group-hover:scale-[1.06]"
                        src=thumbnail_url
                        alt=""
                        decoding="async"
                    />
                }
                .into_any()
            } else {
                view! {
                    <div
                        class="absolute inset-0"
                        style=format!(
                            "background: radial-gradient(125% 95% at 50% 22%, rgba({accent}, 0.24), rgba({accent}, 0.05) 55%, rgba(10, 8, 18, 1) 82%)"
                        )
                    ></div>
                    <div class="absolute inset-0 flex items-center justify-center pb-6">
                        <Icon
                            icon=icon
                            width="44px"
                            height="44px"
                            style=format!("color: rgba({accent}, 0.5)")
                        />
                    </div>
                }
                .into_any()
            }}

            <div
                class="pointer-events-none absolute inset-0"
                style="background: linear-gradient(180deg, rgba(0, 0, 0, 0.12) 0%, rgba(0, 0, 0, 0.04) 32%, rgba(0, 0, 0, 0.68) 70%, rgba(0, 0, 0, 0.9) 100%)"
            ></div>

            <div
                class="absolute inset-x-0 top-0 z-[1] h-[2px]"
                style=format!(
                    "background: linear-gradient(90deg, rgba({accent}, 0.5), rgba({accent}, 0.08))"
                )
            ></div>

            {move || {
                is_selected.get().then(|| view! {
                    <div
                        class="pointer-events-none absolute inset-0 rounded-xl"
                        style="box-shadow: inset 0 0 0 1px rgba(225, 53, 255, 0.4), inset 0 1px 0 rgba(225, 53, 255, 0.25)"
                    ></div>
                })
            }}

            <div class="absolute inset-x-0 bottom-0 z-10 flex flex-col gap-1 px-3 pb-3 pt-6">
                <div class="flex items-center gap-1.5">
                    <span
                        class="h-1.5 w-1.5 shrink-0 rounded-full"
                        style=format!("background: rgb({accent}); box-shadow: 0 0 6px rgba({accent}, 0.7)")
                    ></span>
                    <span
                        class="font-mono text-[10px] font-medium uppercase tracking-wider"
                        style=format!("color: rgba({accent}, 0.92)")
                    >
                        {label}
                    </span>
                </div>
                <div class="truncate text-[13px] font-semibold text-white drop-shadow-[0_2px_8px_rgba(0,0,0,0.85)]">
                    {name}
                </div>
                <div class="truncate font-mono text-[10px] text-white/65 drop-shadow-[0_1px_4px_rgba(0,0,0,0.85)]">
                    {meta_line}
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

/// Coarse media kind from a record's MIME type, for the card badge and the
/// catalog filter. Thin wrapper over [`kind_from_mime`] so the leptos-free
/// classification stays unit-testable.
#[must_use]
pub fn asset_kind(asset: &api::MediaAssetRecord) -> &'static str {
    kind_from_mime(&asset.mime_type, &asset.name)
}

/// Secondary card metadata: pixel dimensions when known, else a format hint.
fn card_meta_detail(asset: &api::MediaAssetRecord, kind: &str) -> String {
    match (asset.intrinsic_width, asset.intrinsic_height) {
        (Some(width), Some(height)) => format!("{width}×{height}"),
        _ => match kind {
            "video" => asset
                .mime_type
                .rsplit('/')
                .next()
                .unwrap_or("video")
                .to_uppercase(),
            "lottie" => "Vector".to_owned(),
            _ => kind_label(kind).to_owned(),
        },
    }
}
