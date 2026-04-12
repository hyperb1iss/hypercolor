//! `/displays` — LCD-equipped devices and their overlay stacks.
//!
//! Three-pane workspace (picker, live preview, overlay stack + inspector)
//! for composing clock/sensor/image/text widgets on top of effect frames
//! before they reach pixel-addressable displays. Wave A delivers the page
//! shell plus the display picker; later tasks fill in the preview canvas,
//! catalog modal, and per-type inspector forms.

use hypercolor_types::overlay::{
    Anchor, ClockConfig, ClockStyle, DisplayOverlayConfig, HourFormat, ImageFit,
    ImageOverlayConfig, OverlayBlendMode, OverlayPosition, OverlaySlot, OverlaySource,
    SensorDisplayStyle, SensorOverlayConfig, TextAlign, TextOverlayConfig,
};
use icondata_core::Icon as IconData;
use leptos::prelude::*;
use leptos::task::spawn_local;
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
            <div class="grid min-h-0 flex-1 grid-cols-[260px_minmax(0,1fr)_340px] gap-3 p-3">
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
    let (overlay_config, set_overlay_config) =
        signal(None::<Result<DisplayOverlayConfig, String>>);
    let (catalog_open, set_catalog_open) = signal(false);

    // Reload the overlay stack whenever the selected display changes. We
    // keep the last-good config in place until the new one arrives so the
    // panel doesn't flash an empty state mid-swap.
    Effect::new(move |_| {
        let Some(display_id) = selected_id.get() else {
            set_overlay_config.set(None);
            return;
        };
        let set_overlay_config = set_overlay_config;
        spawn_local(async move {
            let result = api::fetch_display_overlays(&display_id).await;
            set_overlay_config.set(Some(result));
        });
    });

    let refresh = move || {
        let Some(display_id) = selected_id.get_untracked() else {
            return;
        };
        let set_overlay_config = set_overlay_config;
        spawn_local(async move {
            let result = api::fetch_display_overlays(&display_id).await;
            set_overlay_config.set(Some(result));
        });
    };

    let add_disabled = Signal::derive(move || selected_id.with(Option::is_none));
    let refresh_for_catalog = refresh.clone();
    let on_catalog_select = move |kind: OverlayKind| {
        let Some(display_id) = selected_id.get_untracked() else {
            return;
        };
        let refresh = refresh_for_catalog.clone();
        spawn_local(async move {
            let body = kind.default_create_request();
            if api::create_overlay_slot(&display_id, &body).await.is_ok() {
                refresh();
            }
            set_catalog_open.set(false);
        });
    };

    view! {
        <aside class="relative flex min-h-0 flex-col overflow-hidden rounded-lg border border-edge-subtle bg-surface-raised">
            <header class="flex items-center justify-between border-b border-edge-subtle px-3 py-2">
                <div class="text-xs uppercase tracking-wider text-fg-tertiary">
                    "Overlay stack"
                </div>
                <button
                    type="button"
                    class="flex items-center gap-1 rounded-sm px-2 py-0.5 text-[11px] uppercase tracking-wider transition hover:text-accent-primary disabled:cursor-not-allowed disabled:opacity-40"
                    title="Add a new overlay to this display"
                    disabled=move || add_disabled.get()
                    on:click=move |_| set_catalog_open.set(true)
                >
                    <Icon icon=LuPlus width="12" height="12" />
                    "Add"
                </button>
            </header>
            <div class="min-h-0 flex-1 overflow-y-auto p-2">
                {move || {
                    if selected_id.with(Option::is_none) {
                        return view! {
                            <StackPlaceholder message="Select a display to view its overlay stack." />
                        }
                        .into_any();
                    }
                    match overlay_config.get() {
                        None => view! {
                            <StackPlaceholder message="Loading overlays..." />
                        }
                        .into_any(),
                        Some(Err(error)) => view! {
                            <StackPlaceholder message=error />
                        }
                        .into_any(),
                        Some(Ok(config)) if config.overlays.is_empty() => view! {
                            <StackPlaceholder message="No overlays yet. Add a clock, sensor, image, or text widget." />
                        }
                        .into_any(),
                        Some(Ok(config)) => {
                            let rows = config
                                .overlays
                                .iter()
                                .rev()
                                .cloned()
                                .map(|slot| {
                                    render_slot_row(slot, selected_id, refresh)
                                })
                                .collect_view();
                            view! { <ul class="flex flex-col gap-2">{rows}</ul> }.into_any()
                        }
                    }
                }}
            </div>
            <Show when=move || catalog_open.get() fallback=|| ()>
                <OverlayCatalogModal
                    on_select=on_catalog_select.clone()
                    on_close=move || set_catalog_open.set(false)
                />
            </Show>
        </aside>
    }
}

#[component]
fn OverlayCatalogModal<F, C>(on_select: F, on_close: C) -> impl IntoView
where
    F: Fn(OverlayKind) + Clone + 'static,
    C: Fn() + Clone + 'static,
{
    let close_backdrop = on_close.clone();
    let close_button = on_close.clone();
    let options = [
        OverlayKind::Clock,
        OverlayKind::Sensor,
        OverlayKind::Image,
        OverlayKind::Text,
    ];

    let tiles = options
        .into_iter()
        .map(|kind| {
            let pick = on_select.clone();
            let descriptor = kind.descriptor();
            view! {
                <button
                    type="button"
                    class="flex flex-col items-start gap-1 rounded-md border border-edge-subtle bg-surface-raised p-3 text-left transition hover:border-accent-primary hover:bg-surface-overlay"
                    on:click=move |_| pick(kind)
                >
                    <span class="flex items-center gap-2 text-sm font-medium text-fg-primary">
                        <Icon icon=descriptor.icon width="14" height="14" />
                        {descriptor.label}
                    </span>
                    <span class="text-[11px] leading-relaxed text-fg-tertiary">
                        {descriptor.blurb}
                    </span>
                </button>
            }
        })
        .collect_view();

    view! {
        <div
            class="absolute inset-0 z-10 flex items-center justify-center bg-black/60 backdrop-blur-sm"
            on:click=move |_| close_backdrop()
        >
            <div
                class="w-[290px] rounded-lg border border-edge-subtle bg-surface-raised p-4 shadow-2xl"
                on:click=|event| event.stop_propagation()
            >
                <div class="mb-3 flex items-center justify-between">
                    <h2 class="text-sm font-semibold text-fg-primary">"Add overlay"</h2>
                    <button
                        type="button"
                        class="rounded-sm p-1 text-fg-tertiary transition hover:text-accent-primary"
                        title="Close"
                        on:click=move |_| close_button()
                    >
                        <Icon icon=LuX width="14" height="14" />
                    </button>
                </div>
                <div class="grid grid-cols-2 gap-2">{tiles}</div>
                <div class="mt-3 rounded-md border border-edge-subtle bg-surface-overlay/50 p-2 text-[11px] leading-relaxed text-fg-tertiary">
                    <Icon icon=LuCode width="12" height="12" />
                    " HTML overlays are gated until Servo supports multi-session rendering."
                </div>
            </div>
        </div>
    }
}

#[component]
fn StackPlaceholder(#[prop(into)] message: String) -> impl IntoView {
    view! {
        <div class="px-2 py-6 text-xs leading-relaxed text-fg-tertiary">{message}</div>
    }
}

fn render_slot_row(
    slot: OverlaySlot,
    selected_id: ReadSignal<Option<String>>,
    refresh: impl Fn() + Clone + 'static,
) -> impl IntoView {
    let slot_id = slot.id;
    let enabled = slot.enabled;
    let opacity_percent = (slot.opacity.clamp(0.0, 1.0) * 100.0).round() as i32;
    let (source_label, source_icon) = overlay_source_descriptor(&slot.source);
    let status_label = if slot.enabled { "Active" } else { "Disabled" };
    let status_class = if slot.enabled {
        "bg-emerald-500/15 text-emerald-300"
    } else {
        "bg-fg-tertiary/15 text-fg-tertiary"
    };
    let slot_name = slot.name.clone();

    let on_toggle = {
        let refresh = refresh.clone();
        move |_| {
            let Some(display_id) = selected_id.get_untracked() else {
                return;
            };
            let refresh = refresh.clone();
            spawn_local(async move {
                let body = api::UpdateOverlaySlotRequest {
                    enabled: Some(!enabled),
                    ..Default::default()
                };
                if api::patch_overlay_slot(&display_id, slot_id, &body)
                    .await
                    .is_ok()
                {
                    refresh();
                }
            });
        }
    };

    let on_delete = {
        let refresh = refresh.clone();
        move |_| {
            let Some(display_id) = selected_id.get_untracked() else {
                return;
            };
            let refresh = refresh.clone();
            spawn_local(async move {
                if api::delete_overlay_slot(&display_id, slot_id).await.is_ok() {
                    refresh();
                }
            });
        }
    };

    view! {
        <li class="flex flex-col gap-2 rounded-md border border-edge-subtle bg-surface-overlay/60 p-2.5">
            <div class="flex items-center gap-2">
                <span class="flex h-7 w-7 items-center justify-center rounded-sm bg-surface-raised text-accent-primary">
                    <Icon icon=source_icon width="14" height="14" />
                </span>
                <div class="min-w-0 flex-1">
                    <div class="truncate text-sm font-medium text-fg-primary">{slot_name}</div>
                    <div class="text-[10px] uppercase tracking-wider text-fg-tertiary">
                        {source_label}
                    </div>
                </div>
                <span class=format!(
                    "rounded-sm px-1.5 py-0.5 text-[10px] uppercase tracking-wider {status_class}"
                )>
                    {status_label}
                </span>
            </div>
            <div class="flex items-center justify-between gap-2 text-[11px] text-fg-tertiary">
                <span>{format!("Opacity {opacity_percent}%")}</span>
                <div class="flex items-center gap-1">
                    <button
                        type="button"
                        class="rounded-sm px-2 py-1 text-fg-tertiary transition hover:text-accent-primary"
                        title=if enabled { "Disable overlay" } else { "Enable overlay" }
                        on:click=on_toggle
                    >
                        <Icon icon=if enabled { LuEye } else { LuEyeOff } width="14" height="14" />
                    </button>
                    <button
                        type="button"
                        class="rounded-sm px-2 py-1 text-fg-tertiary transition hover:text-status-error"
                        title="Delete overlay"
                        on:click=on_delete
                    >
                        <Icon icon=LuTrash2 width="14" height="14" />
                    </button>
                </div>
            </div>
        </li>
    }
}

fn overlay_source_descriptor(source: &OverlaySource) -> (&'static str, IconData) {
    match source {
        OverlaySource::Clock(_) => ("Clock", LuTimer),
        OverlaySource::Sensor(_) => ("Sensor", LuGauge),
        OverlaySource::Image(_) => ("Image", LuLayers),
        OverlaySource::Text(_) => ("Text", LuType),
        OverlaySource::Html(_) => ("HTML", LuCode),
    }
}

#[derive(Debug, Clone, Copy)]
enum OverlayKind {
    Clock,
    Sensor,
    Image,
    Text,
}

struct OverlayKindDescriptor {
    label: &'static str,
    blurb: &'static str,
    icon: IconData,
}

impl OverlayKind {
    fn descriptor(self) -> OverlayKindDescriptor {
        match self {
            Self::Clock => OverlayKindDescriptor {
                label: "Clock",
                blurb: "Digital or analog time",
                icon: LuTimer,
            },
            Self::Sensor => OverlayKindDescriptor {
                label: "Sensor",
                blurb: "Live system metrics",
                icon: LuGauge,
            },
            Self::Image => OverlayKindDescriptor {
                label: "Image",
                blurb: "Static art or GIFs",
                icon: LuLayers,
            },
            Self::Text => OverlayKindDescriptor {
                label: "Text",
                blurb: "Styled static or scrolling text",
                icon: LuType,
            },
        }
    }

    fn default_create_request(self) -> api::CreateOverlaySlotRequest {
        let (name, source) = match self {
            Self::Clock => (
                "Clock".to_string(),
                OverlaySource::Clock(ClockConfig {
                    style: ClockStyle::Digital,
                    hour_format: HourFormat::TwentyFour,
                    show_seconds: true,
                    show_date: false,
                    date_format: None,
                    font_family: None,
                    color: "#ffffff".to_string(),
                    secondary_color: None,
                    template: None,
                }),
            ),
            Self::Sensor => (
                "CPU temp".to_string(),
                OverlaySource::Sensor(SensorOverlayConfig {
                    sensor: "cpu_temp".to_string(),
                    style: SensorDisplayStyle::Numeric,
                    unit_label: Some("°C".to_string()),
                    range_min: 20.0,
                    range_max: 95.0,
                    color_min: "#80ffea".to_string(),
                    color_max: "#ff6ac1".to_string(),
                    font_family: None,
                    template: None,
                }),
            ),
            Self::Image => (
                "Image".to_string(),
                OverlaySource::Image(ImageOverlayConfig {
                    path: String::new(),
                    speed: 1.0,
                    fit: ImageFit::Contain,
                }),
            ),
            Self::Text => (
                "Label".to_string(),
                OverlaySource::Text(TextOverlayConfig {
                    text: "Hypercolor".to_string(),
                    font_family: None,
                    font_size: 24.0,
                    color: "#ffffff".to_string(),
                    align: TextAlign::Center,
                    scroll: false,
                    scroll_speed: 30.0,
                }),
            ),
        };

        api::CreateOverlaySlotRequest {
            name,
            source,
            position: OverlayPosition::Anchored {
                anchor: Anchor::Center,
                offset_x: 0,
                offset_y: 0,
                width: 200,
                height: 60,
            },
            blend_mode: OverlayBlendMode::Normal,
            opacity: 1.0,
            enabled: true,
        }
    }
}
