use leptos::prelude::*;
use leptos_icons::Icon;

use crate::api;
use crate::icons::*;
use hypercolor_leptos_ext::events::Input;

use super::DisplaysModalBackdrop;

#[component]
pub(super) fn DisplayFacePickerModal(
    display_name: String,
    faces: ReadSignal<Option<Result<Vec<api::EffectSummary>, String>>>,
    current_face_id: Signal<Option<String>>,
    assigning: ReadSignal<bool>,
    scope: ReadSignal<api::DisplayFaceScope>,
    set_scope: WriteSignal<api::DisplayFaceScope>,
    #[prop(into)] on_select: Callback<String>,
    #[prop(into)] on_clear: Callback<()>,
    #[prop(into)] on_close: Callback<()>,
) -> impl IntoView {
    let (search, set_search) = signal(String::new());
    let thumbnails = use_context::<crate::thumbnails::ThumbnailStore>();
    let close_button = on_close;
    let clear_button = on_clear;

    view! {
        <DisplaysModalBackdrop wide=true on_close=on_close>
            <div
                class="flex max-h-[85vh] w-full max-w-4xl flex-col overflow-hidden rounded-xl border border-edge-subtle bg-surface-raised shadow-2xl edge-glow-accent accent-coral"
                on:click=|event| event.stop_propagation()
            >
                <div class="flex items-start justify-between gap-3 border-b border-edge-subtle px-4 py-3">
                    <div class="flex min-w-0 items-center gap-2">
                        <div class="flex h-7 w-7 items-center justify-center rounded-md bg-coral/12 text-coral/85">
                            <Icon icon=LuLayers width="14" height="14" />
                        </div>
                        <div class="min-w-0">
                            <h2 class="text-sm font-semibold text-fg-primary">
                                "Choose display face"
                            </h2>
                            <p class="mt-0.5 text-[11px] leading-relaxed text-fg-tertiary">
                                {format!("Assign a full-screen HTML face to {display_name}.")}
                            </p>
                        </div>
                    </div>
                    <div class="flex items-center gap-1">
                        <button
                            type="button"
                            class="inline-flex items-center gap-1.5 rounded-md border border-edge-subtle bg-surface-overlay px-2.5 py-1.5 text-[10px] uppercase tracking-wider text-fg-tertiary transition hover:border-status-error/35 hover:text-status-error disabled:cursor-not-allowed disabled:opacity-40"
                            disabled=move || assigning.get() || current_face_id.with(Option::is_none)
                            on:click=move |_| clear_button.run(())
                            title="Clear the current face assignment"
                        >
                            <Icon icon=LuX width="11" height="11" />
                            "Clear"
                        </button>
                        <button
                            type="button"
                            class="rounded-sm p-1 text-fg-tertiary transition hover:text-accent-primary"
                            title="Close"
                            on:click=move |_| close_button.run(())
                        >
                            <Icon icon=LuX width="14" height="14" />
                        </button>
                    </div>
                </div>

                <div class="flex items-center gap-2 border-b border-edge-subtle px-4 py-2.5">
                    <span class="text-[10px] font-semibold uppercase tracking-wider text-fg-tertiary">
                        "Apply"
                    </span>
                    <div class="flex overflow-hidden rounded-md border border-edge-subtle">
                        <button
                            type="button"
                            class=move || scope_segment_class(scope.get() == api::DisplayFaceScope::Default)
                            on:click=move |_| set_scope.set(api::DisplayFaceScope::Default)
                            title="Persists across scene switches — this becomes the display's own face"
                        >
                            "Always"
                        </button>
                        <button
                            type="button"
                            class=move || scope_segment_class(scope.get() == api::DisplayFaceScope::Scene)
                            on:click=move |_| set_scope.set(api::DisplayFaceScope::Scene)
                            title="Lives in the active scene only — overrides the default while this scene is active"
                        >
                            "This scene"
                        </button>
                    </div>
                    <span class="text-[10px] text-fg-tertiary">
                        {move || match scope.get() {
                            api::DisplayFaceScope::Default => "sticks across scene switches",
                            api::DisplayFaceScope::Scene => "only while this scene is active",
                        }}
                    </span>
                </div>

                <div class="border-b border-edge-subtle px-4 py-3">
                    <div class="relative">
                        <span class="pointer-events-none absolute left-3 top-1/2 -translate-y-1/2 text-fg-tertiary">
                            <Icon icon=LuSearch width="13" height="13" />
                        </span>
                        <input
                            type="search"
                            class="w-full rounded-md border border-edge-subtle bg-surface-overlay px-9 py-2 text-sm text-fg-primary outline-none transition focus:border-accent-primary"
                            placeholder="Search faces by name, author, or description"
                            prop:value=move || search.get()
                            on:input=move |event| {
                                set_search.set(Input::from_event(event).value_string().unwrap_or_default())
                            }
                        />
                    </div>
                </div>

                <div class="min-h-0 flex-1 overflow-y-auto p-4">
                    {move || {
                        let query = search.get().trim().to_lowercase();
                        match faces.get() {
                            None => view! {
                                <div class="grid grid-cols-2 gap-3 sm:grid-cols-3">
                                    <FaceGallerySkeleton />
                                    <FaceGallerySkeleton />
                                    <FaceGallerySkeleton />
                                </div>
                            }
                            .into_any(),
                            Some(Err(error)) => view! {
                                <div class="rounded-md border border-status-error/35 bg-status-error/10 px-3 py-3 text-sm text-status-error">
                                    {error}
                                </div>
                            }
                            .into_any(),
                            Some(Ok(items)) => {
                                let total = items.len();
                                let filtered = items
                                    .into_iter()
                                    .filter(|effect| {
                                        if query.is_empty() {
                                            return true;
                                        }
                                        effect.name.to_lowercase().contains(&query)
                                            || effect.author.to_lowercase().contains(&query)
                                            || effect.description.to_lowercase().contains(&query)
                                    })
                                    .collect::<Vec<_>>();
                                if filtered.is_empty() {
                                    return view! {
                                        <div class="flex flex-col items-center gap-2 py-12 text-center">
                                            <Icon icon=LuSearch width="24" height="24" />
                                            <p class="text-sm text-fg-secondary">
                                                {if total == 0 {
                                                    "No display faces installed yet. Build one with the Face SDK and rebuild the effects bundle.".to_owned()
                                                } else {
                                                    format!("No faces match \"{query}\".")
                                                }}
                                            </p>
                                        </div>
                                    }
                                    .into_any();
                                }

                                let cards = filtered
                                    .into_iter()
                                    .map(|effect| render_face_gallery_card(
                                        effect,
                                        current_face_id,
                                        assigning,
                                        on_select,
                                        thumbnails,
                                    ))
                                    .collect_view();
                                view! {
                                    <div class="grid grid-cols-2 gap-3 sm:grid-cols-3">
                                        {cards}
                                    </div>
                                }
                                .into_any()
                            }
                        }
                    }}
                </div>
            </div>
        </DisplaysModalBackdrop>
    }
}

/// Visual card used inside the face gallery. Shows a thumbnail captured
/// from the `ThumbnailStore` when the face has rendered before, plus a
/// gradient fallback, name/author, and an "Assigned" pill for the
/// currently-active face.
fn render_face_gallery_card(
    effect: api::EffectSummary,
    current_face_id: Signal<Option<String>>,
    assigning: ReadSignal<bool>,
    on_select: Callback<String>,
    thumbnails: Option<crate::thumbnails::ThumbnailStore>,
) -> impl IntoView {
    let effect_id = effect.id.clone();
    let effect_id_for_click = effect_id.clone();
    let effect_id_for_current = effect_id.clone();
    let effect_version = effect.version.clone();
    let is_current = Signal::derive(move || {
        current_face_id.get().as_deref() == Some(effect_id_for_current.as_str())
    });

    // Thumbnail lookup is reactive on the ThumbnailStore inner signal, so
    // newly captured thumbnails appear without closing the picker.
    let thumbnail = Signal::derive({
        let effect_id = effect_id.clone();
        let effect_version = effect_version.clone();
        move || thumbnails.and_then(|store| store.get(&effect_id, &effect_version))
    });

    // Deterministic gradient fallback derived from the effect name so
    // each face still has a distinct visual even without a thumbnail.
    let gradient_fallback = face_gradient_fallback(&effect.name);

    let name = effect.name;
    let author = effect.author;

    view! {
        <button
            type="button"
            class=move || {
                if is_current.get() {
                    "group flex flex-col overflow-hidden rounded-lg border border-coral bg-coral/5 text-left transition disabled:cursor-not-allowed disabled:opacity-60"
                } else {
                    "group flex flex-col overflow-hidden rounded-lg border border-edge-subtle bg-surface-overlay/40 text-left transition hover:border-accent-primary/35 disabled:cursor-not-allowed disabled:opacity-60"
                }
            }
            disabled=move || assigning.get()
            on:click=move |_| on_select.run(effect_id_for_click.clone())
        >
            <div
                class="relative aspect-[4/3] overflow-hidden"
                style=move || {
                    thumbnail.get().map_or_else(
                        || gradient_fallback.clone(),
                        |thumb| format!(
                            "background: #000 url('{}') center/cover no-repeat;",
                            thumb.data_url
                        ),
                    )
                }
            >
                <Show when=move || is_current.get() fallback=|| ()>
                    <span class="absolute right-1.5 top-1.5 rounded-full border border-coral/60 bg-coral/25 px-2 py-0.5 text-[9px] font-semibold uppercase tracking-wider text-white backdrop-blur-sm">
                        "Assigned"
                    </span>
                </Show>
            </div>
            <div class="flex min-h-0 flex-col gap-1 border-t border-edge-subtle/50 px-3 py-2.5">
                <div class="truncate text-sm font-medium text-fg-primary">{name}</div>
                <div class="truncate text-[10px] uppercase tracking-wider text-fg-tertiary">
                    {format!("by {author}")}
                </div>
            </div>
        </button>
    }
}

/// Segmented-toggle styling for the assignment-scope switch.
fn scope_segment_class(active: bool) -> &'static str {
    if active {
        "bg-accent-primary/20 px-2.5 py-1 text-[10px] font-semibold uppercase tracking-wider text-accent-primary transition"
    } else {
        "px-2.5 py-1 text-[10px] font-semibold uppercase tracking-wider text-fg-tertiary transition hover:text-fg-secondary"
    }
}

/// Skeleton card shown while the face catalog is loading.
#[component]
fn FaceGallerySkeleton() -> impl IntoView {
    view! {
        <div class="flex animate-pulse flex-col overflow-hidden rounded-lg border border-edge-subtle bg-surface-overlay/30">
            <div class="aspect-[4/3] bg-surface-overlay/50" />
            <div class="flex flex-col gap-1 border-t border-edge-subtle/50 px-3 py-2.5">
                <div class="h-3 w-3/4 rounded bg-surface-overlay/60" />
                <div class="h-2 w-1/2 rounded bg-surface-overlay/40" />
            </div>
        </div>
    }
}

/// Deterministic CSS background for face cards without a thumbnail. The
/// hue is derived from the effect name so each face keeps a distinct
/// identity in the picker before its first render.
fn face_gradient_fallback(name: &str) -> String {
    // Small FNV-1a-ish hash so the hue is stable per name but evenly
    // distributed across the 360deg wheel.
    let mut hash: u32 = 2_166_136_261;
    for byte in name.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(16_777_619);
    }
    let hue = hash % 360;
    format!(
        "background: linear-gradient(135deg, hsl({hue}deg 65% 28%) 0%, hsl({secondary}deg 55% 18%) 100%);",
        secondary = (hue + 40) % 360,
    )
}
