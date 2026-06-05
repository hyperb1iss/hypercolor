use leptos::prelude::*;
use leptos_icons::Icon;

use crate::api;
use crate::components::display_preview_surface::DisplayPreviewSurface;
use crate::display_preview_state::use_display_preview_subscription;
use crate::display_utils::display_preview_shell_url;
use crate::icons::*;

#[component]
pub(super) fn DisplayEmptyWorkspace() -> impl IntoView {
    view! {
        <section class="flex min-h-0 flex-1 flex-col overflow-hidden rounded-xl border border-edge-subtle bg-surface-raised/80">
            <header class="flex items-center gap-2 border-b border-edge-subtle px-4 py-3">
                <div class="flex h-6 w-6 items-center justify-center rounded-md bg-coral/10 text-coral/80">
                    <Icon icon=LuMonitor width="13" height="13" />
                </div>
                <h2 class="text-[11px] font-semibold uppercase tracking-wide text-fg-secondary">
                    "Live preview"
                </h2>
            </header>
            <div class="flex min-h-0 flex-1 items-center justify-center p-5">
                <div class="flex flex-col items-center gap-2 text-center text-fg-tertiary">
                    <Icon icon=LuMonitor width="32" height="32" />
                    <p class="text-xs">
                        "Add a virtual display simulator or connect an LCD device to begin."
                    </p>
                </div>
            </div>
        </section>
    }
}

#[component]
pub(super) fn DisplayWorkspace(
    selected_display: Memo<Option<api::DisplaySummary>>,
    display_face: ReadSignal<Option<Result<Option<api::DisplayFaceResponse>, String>>>,
    face_refresh_tick: ReadSignal<u64>,
) -> impl IntoView {
    let ws = use_context::<crate::app::WsContext>();

    if let Some(ctx) = ws {
        use_display_preview_subscription(
            ctx,
            Signal::derive(move || {
                selected_display.with(|display| display.as_ref().map(|item| item.id.clone()))
            }),
        );
    }
    let preview_frame = Signal::derive(move || ws.and_then(|ctx| ctx.display_preview_frame.get()));

    let subtitle = Signal::derive(move || {
        selected_display.with(|display| {
            display.as_ref().map(|summary| {
                let shape = if summary.circular {
                    "round"
                } else if summary.width > summary.height * 2 {
                    "wide"
                } else {
                    "rect"
                };
                format!(
                    "{} · {}x{} · {}",
                    summary.vendor, summary.width, summary.height, shape
                )
            })
        })
    });
    let current_face_name = Signal::derive(move || match display_face.get() {
        None => "Loading face...".to_owned(),
        Some(Ok(Some(face))) => face.effect.name,
        Some(Ok(None)) => "No face assigned".to_owned(),
        Some(Err(_)) => "Face unavailable".to_owned(),
    });
    let has_face = Signal::derive(move || matches!(display_face.get(), Some(Ok(Some(_)))));

    view! {
        <section class="flex min-h-0 flex-1 flex-col overflow-hidden rounded-xl border border-edge-subtle bg-surface-raised/80 edge-glow-accent accent-coral">
            <header class="flex flex-wrap items-center justify-between gap-3 border-b border-edge-subtle px-4 py-3">
                <div class="flex min-w-0 flex-1 items-center gap-2">
                    <div class="flex h-6 w-6 items-center justify-center rounded-md bg-coral/10 text-coral/80">
                        <Icon icon=LuMonitor width="13" height="13" />
                    </div>
                    <h2 class="text-[11px] font-semibold uppercase tracking-wide text-fg-secondary">
                        "Live preview"
                    </h2>
                    <Show when=move || selected_display.with(Option::is_some) fallback=|| ()>
                        <span class="rounded-full border border-edge-subtle bg-surface-overlay/60 px-2 py-0.5 text-[10px] text-fg-tertiary">
                            {move || subtitle.get().unwrap_or_default()}
                        </span>
                        <span class=move || if has_face.get() {
                            "inline-flex min-w-0 items-center gap-1.5 rounded-full border border-coral/35 bg-coral/10 px-2 py-0.5 text-[10px] text-coral"
                        } else {
                            "inline-flex min-w-0 items-center gap-1.5 rounded-full border border-edge-subtle px-2 py-0.5 text-[10px] text-fg-tertiary"
                        }>
                            <Icon icon=LuLayers width="10" height="10" />
                            <span class="truncate">{move || current_face_name.get()}</span>
                        </span>
                    </Show>
                </div>
                {move || {
                    selected_display.get().map(|display| {
                        let href = display_preview_shell_url(&display.id);
                        view! {
                            <a
                                href=href
                                target="_blank"
                                rel="noopener"
                                class="inline-flex items-center gap-1.5 text-[11px] text-fg-tertiary transition hover:text-accent-primary"
                            >
                                <Icon icon=LuExternalLink width="11" height="11" />
                                "Open preview"
                            </a>
                        }
                    })
                }}
            </header>
            <div class="flex min-h-0 flex-1 items-center justify-center p-5">
                {move || {
                    let Some(display) = selected_display.get() else {
                        return view! {
                            <div class="flex flex-col items-center gap-2 text-center">
                                <Icon icon=LuMonitor width="32" height="32" />
                                <p class="text-xs text-fg-tertiary">
                                    "Select a display on the left to begin."
                                </p>
                            </div>
                        }.into_any();
                    };
                    let refresh_tick = face_refresh_tick.get();
                    let aspect = format!("{} / {}", display.width, display.height);
                    let container_class = if display.circular {
                        "max-h-full max-w-full overflow-hidden rounded-full border border-edge-default bg-black shadow-2xl"
                    } else {
                        "max-h-full max-w-full overflow-hidden rounded-lg border border-edge-default bg-black shadow-2xl"
                    };
                    let aria_label = format!("Live preview of {}", display.name);
                    let fallback_src =
                        api::display_preview_url(&display.id, Some(refresh_tick));

                    view! {
                        <DisplayPreviewSurface
                            frame=preview_frame
                            fallback_src=fallback_src
                            aspect_ratio=aspect
                            aria_label=aria_label
                            container_class=container_class
                        />
                    }.into_any()
                }}
            </div>
        </section>
    }
}
