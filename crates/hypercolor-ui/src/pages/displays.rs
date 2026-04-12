//! `/displays` — LCD-equipped devices and their overlay stacks.
//!
//! Three-pane workspace (picker, live preview, overlay stack + inspector)
//! for composing clock/sensor/image/text widgets on top of effect frames
//! before they reach pixel-addressable displays. Wave A delivers the page
//! shell plus the display picker; later tasks fill in the preview canvas,
//! catalog modal, and per-type inspector forms.

use leptos::prelude::*;
use leptos_icons::Icon;
use leptos_use::use_interval_fn;

use crate::api;
use crate::components::page_header::PageHeader;
use crate::icons::*;

type DisplaysResource = LocalResource<Result<Vec<api::DisplaySummary>, String>>;

/// Polling cadence for the live preview JPEG, in milliseconds.
const PREVIEW_POLL_INTERVAL_MS: u64 = 500;

#[component]
pub fn DisplaysPage() -> impl IntoView {
    let displays: DisplaysResource = LocalResource::new(api::fetch_displays);
    let (selected_id, set_selected_id) = signal(None::<String>);

    // Auto-select the first display once the list loads so the workspace
    // isn't empty on first render.
    Effect::new(move |_| {
        if selected_id.with(Option::is_some) {
            return;
        }
        if let Some(Ok(items)) = displays.get().as_ref()
            && let Some(first) = items.first()
        {
            set_selected_id.set(Some(first.id.clone()));
        }
    });

    let selected_display = Memo::new(move |_| {
        let id = selected_id.get()?;
        let snapshot = displays.get();
        let items = snapshot.as_ref()?.as_ref().ok()?;
        items.iter().find(|display| display.id == id).cloned()
    });

    view! {
        <div class="flex h-full flex-col overflow-hidden animate-fade-in">
            <div class="shrink-0 glass-subtle border-b border-edge-subtle/15">
                <div class="px-6 pt-5 pb-4">
                    <PageHeader
                        icon=LuMonitor
                        title="Displays"
                        subtitle="Compose clocks, sensors, and widgets on top of your LCD-equipped devices."
                        accent_rgb="225, 53, 255"
                        gradient="linear-gradient(105deg,#e135ff 0%,#e8f4ff 55%,#80ffea 100%)"
                    />
                </div>
            </div>
            <div class="grid min-h-0 flex-1 grid-cols-[260px_minmax(0,1fr)_320px] gap-3 p-3">
                <DisplayPicker
                    displays=displays
                    selected_id=selected_id
                    set_selected_id=set_selected_id
                />
                <DisplayWorkspace selected_display=selected_display />
                <OverlayStackPanel selected_id=selected_id />
            </div>
        </div>
    }
}

#[component]
fn DisplayPicker(
    displays: DisplaysResource,
    selected_id: ReadSignal<Option<String>>,
    set_selected_id: WriteSignal<Option<String>>,
) -> impl IntoView {
    view! {
        <aside class="flex min-h-0 flex-col overflow-hidden rounded-lg border border-edge-subtle bg-surface-raised">
            <header class="flex items-center justify-between border-b border-edge-subtle px-3 py-2">
                <div class="flex items-center gap-2 text-xs uppercase tracking-wider text-fg-tertiary">
                    <Icon icon=LuMonitor width="14" height="14" />
                    "Displays"
                </div>
                <button
                    type="button"
                    class="rounded-sm p-1 text-fg-tertiary transition hover:text-accent-primary"
                    title="Refresh displays"
                    on:click=move |_| displays.refetch()
                >
                    <Icon icon=LuRefreshCw width="14" height="14" />
                </button>
            </header>
            <div class="min-h-0 flex-1 overflow-y-auto">
                <Suspense fallback=move || view! { <PickerPlaceholder message="Loading displays...".to_string() /> }>
                    {move || {
                        let snapshot = displays.get();
                        let Some(result) = snapshot.as_ref() else {
                            return view! {
                                <PickerPlaceholder message="Loading displays...".to_string() />
                            }
                            .into_any();
                        };
                        match result {
                            Err(error) => view! {
                                <PickerPlaceholder message=error.clone() />
                            }
                            .into_any(),
                            Ok(items) if items.is_empty() => view! {
                                <PickerPlaceholder
                                    message="No LCD devices connected. Connect a Corsair iCUE LINK pump, an Ableton Push 2, or add a virtual display simulator.".to_string()
                                />
                            }
                            .into_any(),
                            Ok(items) => {
                                let rows = items
                                    .clone()
                                    .into_iter()
                                    .map(|display| {
                                        render_picker_row(display, selected_id, set_selected_id)
                                    })
                                    .collect_view();
                                view! { <ul class="divide-y divide-edge-subtle">{rows}</ul> }
                                    .into_any()
                            }
                        }
                    }}
                </Suspense>
            </div>
        </aside>
    }
}

fn render_picker_row(
    display: api::DisplaySummary,
    selected_id: ReadSignal<Option<String>>,
    set_selected_id: WriteSignal<Option<String>>,
) -> impl IntoView {
    let id = display.id.clone();
    let id_for_click = id.clone();
    let id_for_active = id.clone();
    let is_active = Signal::derive(move || {
        selected_id.with(|current| current.as_deref() == Some(id_for_active.as_str()))
    });
    let shape_label = if display.circular {
        "Round"
    } else if display.width > display.height * 2 {
        "Wide"
    } else {
        "Rect"
    };
    let dimensions = format!("{}x{}", display.width, display.height);
    let overlay_summary = if display.overlay_count == 0 {
        "No overlays".to_string()
    } else {
        format!(
            "{} / {} overlays",
            display.enabled_overlay_count, display.overlay_count
        )
    };
    let vendor = display.vendor;
    let name = display.name;

    view! {
        <li>
            <button
                type="button"
                class="flex w-full flex-col items-start gap-1 px-3 py-2 text-left transition hover:bg-surface-overlay"
                class:bg-surface-overlay=move || is_active.get()
                class:border-l-2=move || is_active.get()
                class:border-accent-primary=move || is_active.get()
                on:click=move |_| set_selected_id.set(Some(id_for_click.clone()))
            >
                <span class="flex w-full items-center justify-between gap-2">
                    <span class="truncate text-sm font-medium text-fg-primary">{name}</span>
                    <span class="shrink-0 rounded-sm bg-surface-overlay px-1.5 py-0.5 text-[10px] uppercase tracking-wider text-fg-tertiary">
                        {shape_label}
                    </span>
                </span>
                <span class="flex items-center gap-2 text-[11px] text-fg-tertiary">
                    <span>{vendor}</span>
                    <span class="text-edge-subtle">"·"</span>
                    <span>{dimensions}</span>
                </span>
                <span class="text-[11px] text-fg-tertiary">{overlay_summary}</span>
            </button>
        </li>
    }
}

#[component]
fn PickerPlaceholder(#[prop(into)] message: String) -> impl IntoView {
    view! {
        <div class="px-3 py-6 text-xs leading-relaxed text-fg-tertiary">{message}</div>
    }
}

#[component]
fn DisplayWorkspace(selected_display: Memo<Option<api::DisplaySummary>>) -> impl IntoView {
    let (poll_counter, set_poll_counter) = signal(0_u64);

    // Steady-state polling of the preview JPEG. Tick the counter on every
    // cycle so the <img> src cache-busts; the daemon returns cheap 304s when
    // the frame hasn't advanced. The interval is paused automatically when
    // no display is selected so idle pages stop generating traffic.
    let interval = use_interval_fn(
        move || {
            set_poll_counter.update(|value| *value = value.wrapping_add(1));
        },
        PREVIEW_POLL_INTERVAL_MS,
    );
    let pause = interval.pause.clone();
    let resume = interval.resume.clone();
    Effect::new(move |_| {
        if selected_display.with(Option::is_some) {
            resume();
        } else {
            pause();
        }
    });

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
                format!("{} · {}x{} · {}", summary.vendor, summary.width, summary.height, shape)
            })
        })
    });

    view! {
        <section class="flex min-h-0 flex-col overflow-hidden rounded-lg border border-edge-subtle bg-surface-raised">
            <header class="flex items-center justify-between border-b border-edge-subtle px-3 py-2">
                <div class="text-xs uppercase tracking-wider text-fg-tertiary">
                    "Live preview"
                </div>
                <div class="text-[11px] text-fg-tertiary">
                    {move || subtitle.get().unwrap_or_default()}
                </div>
            </header>
            <div class="flex min-h-0 flex-1 items-center justify-center p-4">
                {move || {
                    let Some(display) = selected_display.get() else {
                        return view! {
                            <p class="text-xs text-fg-tertiary">"Select a display to begin."</p>
                        }.into_any();
                    };
                    let ts = poll_counter.get();
                    let src = api::display_preview_url(&display.id, Some(ts));
                    let aspect = format!("{} / {}", display.width, display.height);
                    let rounded_class = if display.circular {
                        "rounded-full"
                    } else {
                        "rounded-md"
                    };
                    let alt_text = format!("Live preview of {}", display.name);
                    let img_class = format!(
                        "max-h-full max-w-full object-contain border border-edge-subtle bg-black shadow-lg {rounded_class}"
                    );
                    view! {
                        <img
                            class=img_class
                            src=src
                            alt=alt_text
                            style=move || format!("aspect-ratio: {aspect};")
                            loading="eager"
                            decoding="async"
                        />
                    }.into_any()
                }}
            </div>
        </section>
    }
}

#[component]
fn OverlayStackPanel(selected_id: ReadSignal<Option<String>>) -> impl IntoView {
    view! {
        <aside class="flex min-h-0 flex-col overflow-hidden rounded-lg border border-edge-subtle bg-surface-raised">
            <header class="border-b border-edge-subtle px-3 py-2 text-xs uppercase tracking-wider text-fg-tertiary">
                "Overlay stack"
            </header>
            <div class="min-h-0 flex-1 overflow-y-auto p-3 text-xs text-fg-tertiary">
                {move || match selected_id.get() {
                    Some(_) => "Stack list and inspector land in the following tasks.",
                    None => "Select a display to view its overlay stack.",
                }}
            </div>
        </aside>
    }
}
