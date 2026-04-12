//! `/displays` — LCD-equipped devices and their overlay stacks.
//!
//! Three-pane workspace (picker, live preview, overlay stack + inspector)
//! for composing clock/sensor/image/text widgets on top of effect frames
//! before they reach pixel-addressable displays. Wave A delivers the page
//! shell plus the display picker; later tasks fill in the preview canvas,
//! catalog modal, and per-type inspector forms.

use hypercolor_types::overlay::{
    Anchor, ClockConfig, ClockStyle, DisplayOverlayConfig, HourFormat, ImageFit,
    ImageOverlayConfig, OverlayBlendMode, OverlayPosition, OverlaySlot, OverlaySlotId,
    OverlaySource, SensorDisplayStyle, SensorOverlayConfig, TextAlign, TextOverlayConfig,
};
use icondata_core::Icon as IconData;
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_icons::Icon;
use leptos_use::{use_debounce_fn_with_arg, use_interval_fn};

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
                format!(
                    "{} · {}x{} · {}",
                    summary.vendor, summary.width, summary.height, shape
                )
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
    let (overlay_config, set_overlay_config) = signal(None::<Result<DisplayOverlayConfig, String>>);
    let (catalog_open, set_catalog_open) = signal(false);
    let (selected_slot_id, set_selected_slot_id) = signal(None::<OverlaySlotId>);

    // Reload the overlay stack whenever the selected display changes. We
    // keep the last-good config in place until the new one arrives so the
    // panel doesn't flash an empty state mid-swap. Also clear any pinned
    // slot selection so the inspector doesn't linger on a stale id.
    Effect::new(move |_| {
        let Some(display_id) = selected_id.get() else {
            set_overlay_config.set(None);
            set_selected_slot_id.set(None);
            return;
        };
        set_selected_slot_id.set(None);
        let set_overlay_config = set_overlay_config;
        spawn_local(async move {
            let result = api::fetch_display_overlays(&display_id).await;
            set_overlay_config.set(Some(result));
        });
    });

    // If the selected slot disappears (deleted externally or via our own
    // delete button) clear the pinned selection so the inspector closes.
    Effect::new(move |_| {
        let Some(slot_id) = selected_slot_id.get() else {
            return;
        };
        if let Some(Ok(config)) = overlay_config.with(|value| value.clone())
            && !config.overlays.iter().any(|slot| slot.id == slot_id)
        {
            set_selected_slot_id.set(None);
        }
    });

    let selected_slot = Memo::new(move |_| {
        let slot_id = selected_slot_id.get()?;
        let config = overlay_config.with(|value| value.clone())?.ok()?;
        config.overlays.into_iter().find(|slot| slot.id == slot_id)
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
                {
                    let refresh_for_view = refresh.clone();
                    move || {
                        if selected_id.with(Option::is_none) {
                            return view! {
                                <StackPlaceholder message="Select a display to view its overlay stack." />
                            }
                            .into_any();
                        }
                        if let Some(slot) = selected_slot.get() {
                            return overlay_slot_inspector(
                                slot,
                                selected_id,
                                refresh_for_view.clone(),
                                move || set_selected_slot_id.set(None),
                            )
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
                                        render_slot_row(
                                            slot,
                                            selected_id,
                                            refresh_for_view.clone(),
                                            set_selected_slot_id,
                                        )
                                    })
                                    .collect_view();
                                view! { <ul class="flex flex-col gap-2">{rows}</ul> }
                                    .into_any()
                            }
                        }
                    }
                }
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
    set_selected_slot_id: WriteSignal<Option<OverlaySlotId>>,
) -> impl IntoView {
    let slot_id = slot.id;
    let enabled = slot.enabled;
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::as_conversions,
        reason = "opacity is bounded to [0,1] and rendered as a 0-100 integer label"
    )]
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
        <li>
            <button
                type="button"
                class="flex w-full flex-col gap-2 rounded-md border border-edge-subtle bg-surface-overlay/60 p-2.5 text-left transition hover:border-accent-primary/60"
                on:click=move |_| set_selected_slot_id.set(Some(slot_id))
            >
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
                    <div class="flex items-center gap-1" on:click=|event| event.stop_propagation()>
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
            </button>
        </li>
    }
}

fn overlay_slot_inspector<R, B>(
    overlay_slot: OverlaySlot,
    selected_id: ReadSignal<Option<String>>,
    refresh: R,
    on_back: B,
) -> impl IntoView
where
    R: Fn() + Clone + Send + Sync + 'static,
    B: Fn() + Clone + Send + Sync + 'static,
{
    let slot_id = overlay_slot.id;
    let (source_label, source_icon) = overlay_source_descriptor(&overlay_slot.source);
    let source_label_owned = source_label.to_string();

    let refresh_immediate = refresh.clone();
    let patch = move |body: api::UpdateOverlaySlotRequest| {
        let Some(display_id) = selected_id.get_untracked() else {
            return;
        };
        let refresh = refresh_immediate.clone();
        spawn_local(async move {
            if api::patch_overlay_slot(&display_id, slot_id, &body)
                .await
                .is_ok()
            {
                refresh();
            }
        });
    };

    let patch_for_debounce = patch.clone();
    let patch_debounced_raw = use_debounce_fn_with_arg(
        move |body: api::UpdateOverlaySlotRequest| patch_for_debounce(body),
        75.0,
    );
    let patch_debounced = move |body: api::UpdateOverlaySlotRequest| {
        patch_debounced_raw(body);
    };

    // ── Shared field handlers ─────────────────────────────────────────
    let patch_for_name = patch.clone();
    let on_name_change = move |event: leptos::ev::Event| {
        let value = event_target_value(&event);
        patch_for_name(api::UpdateOverlaySlotRequest {
            name: Some(value),
            ..Default::default()
        });
    };

    let patch_for_enabled = patch.clone();
    let slot_for_enabled = overlay_slot.clone();
    let on_enabled_toggle = move |_| {
        patch_for_enabled(api::UpdateOverlaySlotRequest {
            enabled: Some(!slot_for_enabled.enabled),
            ..Default::default()
        });
    };

    let patch_debounced_opacity = patch_debounced.clone();
    let on_opacity_input = move |event: leptos::ev::Event| {
        let Ok(raw) = event_target_value(&event).parse::<i32>() else {
            return;
        };
        #[expect(
            clippy::cast_precision_loss,
            clippy::as_conversions,
            reason = "opacity converts from a 0-100 slider integer"
        )]
        let opacity = (raw as f32 / 100.0).clamp(0.0, 1.0);
        patch_debounced_opacity(api::UpdateOverlaySlotRequest {
            opacity: Some(opacity),
            ..Default::default()
        });
    };

    let patch_for_blend = patch.clone();
    let on_blend_change = move |event: leptos::ev::Event| {
        let value = event_target_value(&event);
        let mode = match value.as_str() {
            "add" => OverlayBlendMode::Add,
            "screen" => OverlayBlendMode::Screen,
            _ => OverlayBlendMode::Normal,
        };
        patch_for_blend(api::UpdateOverlaySlotRequest {
            blend_mode: Some(mode),
            ..Default::default()
        });
    };

    // Source-specific fields live in inner helpers so each closure only
    // borrows what it needs and the overall inspector stays readable.
    // These are plain functions (not #[component]) because the Leptos props
    // builder treats `Fn(_)` generic props as reactive functions, which
    // rejects closures that return `()`.
    let source_editor = match overlay_slot.source.clone() {
        OverlaySource::Clock(config) => clock_inspector_fields(config, patch.clone()).into_any(),
        OverlaySource::Sensor(config) => {
            sensor_inspector_fields(config, patch.clone(), patch_debounced.clone()).into_any()
        }
        OverlaySource::Image(config) => image_inspector_fields(config).into_any(),
        OverlaySource::Text(config) => {
            text_inspector_fields(config, patch.clone(), patch_debounced.clone()).into_any()
        }
        OverlaySource::Html(_) => view! {
            <div class="rounded-md border border-edge-subtle bg-surface-overlay/50 p-3 text-[11px] leading-relaxed text-fg-tertiary">
                "HTML overlays are gated until Servo supports multi-session rendering alongside HTML effects."
            </div>
        }
        .into_any(),
    };

    let opacity_value = (overlay_slot.opacity.clamp(0.0, 1.0) * 100.0).round() as i32;
    let opacity_value_str = opacity_value.to_string();
    let slot_name = overlay_slot.name.clone();
    let blend_value = match overlay_slot.blend_mode {
        OverlayBlendMode::Normal => "normal",
        OverlayBlendMode::Add => "add",
        OverlayBlendMode::Screen => "screen",
    };

    view! {
        <div class="flex flex-col gap-3">
            <div class="flex items-center gap-2">
                <button
                    type="button"
                    class="rounded-sm p-1 text-fg-tertiary transition hover:text-accent-primary"
                    title="Back to stack"
                    on:click=move |_| on_back()
                >
                    <Icon icon=LuChevronLeft width="14" height="14" />
                </button>
                <span class="flex h-7 w-7 items-center justify-center rounded-sm bg-surface-raised text-accent-primary">
                    <Icon icon=source_icon width="14" height="14" />
                </span>
                <div class="min-w-0 flex-1">
                    <div class="text-[10px] uppercase tracking-wider text-fg-tertiary">
                        {source_label_owned}
                    </div>
                    <div class="truncate text-sm font-medium text-fg-primary">
                        {slot_name.clone()}
                    </div>
                </div>
            </div>

            <InspectorField label="Name">
                <input
                    type="text"
                    class="w-full rounded-sm border border-edge-subtle bg-surface-overlay/60 px-2 py-1 text-xs text-fg-primary focus:border-accent-primary focus:outline-none"
                    prop:value=slot_name
                    on:change=on_name_change
                />
            </InspectorField>

            <InspectorField label="Enabled">
                <button
                    type="button"
                    class=move || {
                        if overlay_slot.enabled {
                            "flex items-center gap-1 rounded-sm bg-emerald-500/15 px-2 py-1 text-xs text-emerald-300 transition hover:bg-emerald-500/25"
                        } else {
                            "flex items-center gap-1 rounded-sm bg-surface-overlay/60 px-2 py-1 text-xs text-fg-tertiary transition hover:bg-surface-overlay"
                        }
                    }
                    on:click=on_enabled_toggle
                >
                    <Icon icon=if overlay_slot.enabled { LuEye } else { LuEyeOff } width="12" height="12" />
                    {if overlay_slot.enabled { "Active" } else { "Disabled" }}
                </button>
            </InspectorField>

            <InspectorField label=format!("Opacity {opacity_value}%")>
                <input
                    type="range"
                    min="0"
                    max="100"
                    step="1"
                    class="w-full accent-accent-primary"
                    prop:value=opacity_value_str
                    on:input=on_opacity_input
                />
            </InspectorField>

            <InspectorField label="Blend mode">
                <select
                    class="w-full rounded-sm border border-edge-subtle bg-surface-overlay/60 px-2 py-1 text-xs text-fg-primary focus:border-accent-primary focus:outline-none"
                    on:change=on_blend_change
                >
                    <option value="normal" selected=blend_value == "normal">"Normal"</option>
                    <option value="add" selected=blend_value == "add">"Add"</option>
                    <option value="screen" selected=blend_value == "screen">"Screen"</option>
                </select>
            </InspectorField>

            <div class="flex flex-col gap-2 border-t border-edge-subtle pt-2">
                <div class="text-[10px] uppercase tracking-wider text-fg-tertiary">
                    {format!("{source_label} settings")}
                </div>
                {source_editor}
            </div>
        </div>
    }
}

#[component]
fn InspectorField(#[prop(into)] label: String, children: Children) -> impl IntoView {
    view! {
        <label class="flex flex-col gap-1">
            <span class="text-[10px] uppercase tracking-wider text-fg-tertiary">{label}</span>
            {children()}
        </label>
    }
}

fn clock_inspector_fields<F>(config: ClockConfig, patch: F) -> impl IntoView
where
    F: Fn(api::UpdateOverlaySlotRequest) + Clone + Send + Sync + 'static,
{
    let patch_style = patch.clone();
    let config_for_style = config.clone();
    let on_style = move |event: leptos::ev::Event| {
        let value = event_target_value(&event);
        let style = if value == "analog" {
            ClockStyle::Analog
        } else {
            ClockStyle::Digital
        };
        let mut updated = config_for_style.clone();
        updated.style = style;
        patch_style(api::UpdateOverlaySlotRequest {
            source: Some(OverlaySource::Clock(updated)),
            ..Default::default()
        });
    };

    let patch_format = patch.clone();
    let config_for_format = config.clone();
    let on_format = move |event: leptos::ev::Event| {
        let value = event_target_value(&event);
        let format = if value == "12" {
            HourFormat::Twelve
        } else {
            HourFormat::TwentyFour
        };
        let mut updated = config_for_format.clone();
        updated.hour_format = format;
        patch_format(api::UpdateOverlaySlotRequest {
            source: Some(OverlaySource::Clock(updated)),
            ..Default::default()
        });
    };

    let patch_seconds = patch.clone();
    let config_for_seconds = config.clone();
    let on_seconds = move |_| {
        let mut updated = config_for_seconds.clone();
        updated.show_seconds = !updated.show_seconds;
        patch_seconds(api::UpdateOverlaySlotRequest {
            source: Some(OverlaySource::Clock(updated)),
            ..Default::default()
        });
    };

    let patch_color = patch.clone();
    let config_for_color = config.clone();
    let on_color = move |event: leptos::ev::Event| {
        let value = event_target_value(&event);
        let mut updated = config_for_color.clone();
        updated.color = value;
        patch_color(api::UpdateOverlaySlotRequest {
            source: Some(OverlaySource::Clock(updated)),
            ..Default::default()
        });
    };

    let style_value = match config.style {
        ClockStyle::Digital => "digital",
        ClockStyle::Analog => "analog",
    };
    let format_value = match config.hour_format {
        HourFormat::Twelve => "12",
        HourFormat::TwentyFour => "24",
    };
    let show_seconds = config.show_seconds;
    let color = config.color.clone();

    view! {
        <InspectorField label="Style">
            <select
                class="w-full rounded-sm border border-edge-subtle bg-surface-overlay/60 px-2 py-1 text-xs text-fg-primary focus:border-accent-primary focus:outline-none"
                on:change=on_style
            >
                <option value="digital" selected=style_value == "digital">"Digital"</option>
                <option value="analog" selected=style_value == "analog">"Analog"</option>
            </select>
        </InspectorField>
        <InspectorField label="Hour format">
            <select
                class="w-full rounded-sm border border-edge-subtle bg-surface-overlay/60 px-2 py-1 text-xs text-fg-primary focus:border-accent-primary focus:outline-none"
                on:change=on_format
            >
                <option value="24" selected=format_value == "24">"24 hour"</option>
                <option value="12" selected=format_value == "12">"12 hour"</option>
            </select>
        </InspectorField>
        <InspectorField label="Show seconds">
            <button
                type="button"
                class="self-start rounded-sm bg-surface-overlay/60 px-2 py-1 text-xs text-fg-primary transition hover:bg-surface-overlay"
                on:click=on_seconds
            >
                {if show_seconds { "On" } else { "Off" }}
            </button>
        </InspectorField>
        <InspectorField label="Color">
            <input
                type="color"
                class="h-7 w-16 rounded-sm border border-edge-subtle bg-transparent"
                prop:value=color
                on:change=on_color
            />
        </InspectorField>
    }
}

fn sensor_inspector_fields<F, D>(
    config: SensorOverlayConfig,
    patch: F,
    patch_debounced: D,
) -> impl IntoView
where
    F: Fn(api::UpdateOverlaySlotRequest) + Clone + Send + Sync + 'static,
    D: Fn(api::UpdateOverlaySlotRequest) + Clone + Send + Sync + 'static,
{
    let patch_sensor = patch.clone();
    let config_for_sensor = config.clone();
    let on_sensor = move |event: leptos::ev::Event| {
        let mut updated = config_for_sensor.clone();
        updated.sensor = event_target_value(&event);
        patch_sensor(api::UpdateOverlaySlotRequest {
            source: Some(OverlaySource::Sensor(updated)),
            ..Default::default()
        });
    };

    let patch_style = patch.clone();
    let config_for_style = config.clone();
    let on_style = move |event: leptos::ev::Event| {
        let mut updated = config_for_style.clone();
        updated.style = match event_target_value(&event).as_str() {
            "gauge" => SensorDisplayStyle::Gauge,
            "bar" => SensorDisplayStyle::Bar,
            "minimal" => SensorDisplayStyle::Minimal,
            _ => SensorDisplayStyle::Numeric,
        };
        patch_style(api::UpdateOverlaySlotRequest {
            source: Some(OverlaySource::Sensor(updated)),
            ..Default::default()
        });
    };

    let patch_min = patch_debounced.clone();
    let config_for_min = config.clone();
    let on_min = move |event: leptos::ev::Event| {
        let Ok(value) = event_target_value(&event).parse::<f32>() else {
            return;
        };
        let mut updated = config_for_min.clone();
        updated.range_min = value;
        patch_min(api::UpdateOverlaySlotRequest {
            source: Some(OverlaySource::Sensor(updated)),
            ..Default::default()
        });
    };

    let patch_max = patch_debounced.clone();
    let config_for_max = config.clone();
    let on_max = move |event: leptos::ev::Event| {
        let Ok(value) = event_target_value(&event).parse::<f32>() else {
            return;
        };
        let mut updated = config_for_max.clone();
        updated.range_max = value;
        patch_max(api::UpdateOverlaySlotRequest {
            source: Some(OverlaySource::Sensor(updated)),
            ..Default::default()
        });
    };

    let patch_unit = patch.clone();
    let config_for_unit = config.clone();
    let on_unit = move |event: leptos::ev::Event| {
        let raw = event_target_value(&event);
        let trimmed = raw.trim();
        let mut updated = config_for_unit.clone();
        updated.unit_label = if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        };
        patch_unit(api::UpdateOverlaySlotRequest {
            source: Some(OverlaySource::Sensor(updated)),
            ..Default::default()
        });
    };

    let sensor = config.sensor.clone();
    let unit = config.unit_label.clone().unwrap_or_default();
    let range_min = config.range_min.to_string();
    let range_max = config.range_max.to_string();
    let style_value = match config.style {
        SensorDisplayStyle::Numeric => "numeric",
        SensorDisplayStyle::Gauge => "gauge",
        SensorDisplayStyle::Bar => "bar",
        SensorDisplayStyle::Minimal => "minimal",
    };

    view! {
        <InspectorField label="Sensor label">
            <input
                type="text"
                class="w-full rounded-sm border border-edge-subtle bg-surface-overlay/60 px-2 py-1 text-xs text-fg-primary focus:border-accent-primary focus:outline-none"
                prop:value=sensor
                placeholder="cpu_temp"
                on:change=on_sensor
            />
        </InspectorField>
        <InspectorField label="Style">
            <select
                class="w-full rounded-sm border border-edge-subtle bg-surface-overlay/60 px-2 py-1 text-xs text-fg-primary focus:border-accent-primary focus:outline-none"
                on:change=on_style
            >
                <option value="numeric" selected=style_value == "numeric">"Numeric"</option>
                <option value="gauge" selected=style_value == "gauge">"Gauge"</option>
                <option value="bar" selected=style_value == "bar">"Bar"</option>
                <option value="minimal" selected=style_value == "minimal">"Minimal"</option>
            </select>
        </InspectorField>
        <div class="grid grid-cols-2 gap-2">
            <InspectorField label="Range min">
                <input
                    type="number"
                    class="w-full rounded-sm border border-edge-subtle bg-surface-overlay/60 px-2 py-1 text-xs text-fg-primary focus:border-accent-primary focus:outline-none"
                    prop:value=range_min
                    on:input=on_min
                />
            </InspectorField>
            <InspectorField label="Range max">
                <input
                    type="number"
                    class="w-full rounded-sm border border-edge-subtle bg-surface-overlay/60 px-2 py-1 text-xs text-fg-primary focus:border-accent-primary focus:outline-none"
                    prop:value=range_max
                    on:input=on_max
                />
            </InspectorField>
        </div>
        <InspectorField label="Unit label">
            <input
                type="text"
                class="w-full rounded-sm border border-edge-subtle bg-surface-overlay/60 px-2 py-1 text-xs text-fg-primary focus:border-accent-primary focus:outline-none"
                prop:value=unit
                placeholder="°C"
                on:change=on_unit
            />
        </InspectorField>
    }
}

fn image_inspector_fields(config: ImageOverlayConfig) -> impl IntoView {
    let path = config.path.clone();
    let display_path = if path.is_empty() {
        "(unset)".to_string()
    } else {
        path
    };

    view! {
        <div class="rounded-md border border-edge-subtle bg-surface-overlay/50 p-3 text-[11px] leading-relaxed text-fg-tertiary">
            "Path: " <span class="font-mono text-fg-secondary">{display_path}</span>
            <br />
            "Image upload UI lands in a follow-up. For now, set the path via the API."
        </div>
    }
}

fn text_inspector_fields<F, D>(
    config: TextOverlayConfig,
    patch: F,
    patch_debounced: D,
) -> impl IntoView
where
    F: Fn(api::UpdateOverlaySlotRequest) + Clone + Send + Sync + 'static,
    D: Fn(api::UpdateOverlaySlotRequest) + Clone + Send + Sync + 'static,
{
    let patch_text = patch.clone();
    let config_for_text = config.clone();
    let on_text = move |event: leptos::ev::Event| {
        let mut updated = config_for_text.clone();
        updated.text = event_target_value(&event);
        patch_text(api::UpdateOverlaySlotRequest {
            source: Some(OverlaySource::Text(updated)),
            ..Default::default()
        });
    };

    let patch_size = patch_debounced.clone();
    let config_for_size = config.clone();
    let on_size = move |event: leptos::ev::Event| {
        let Ok(value) = event_target_value(&event).parse::<f32>() else {
            return;
        };
        let mut updated = config_for_size.clone();
        updated.font_size = value.max(1.0);
        patch_size(api::UpdateOverlaySlotRequest {
            source: Some(OverlaySource::Text(updated)),
            ..Default::default()
        });
    };

    let patch_color = patch.clone();
    let config_for_color = config.clone();
    let on_color = move |event: leptos::ev::Event| {
        let mut updated = config_for_color.clone();
        updated.color = event_target_value(&event);
        patch_color(api::UpdateOverlaySlotRequest {
            source: Some(OverlaySource::Text(updated)),
            ..Default::default()
        });
    };

    let patch_align = patch.clone();
    let config_for_align = config.clone();
    let on_align = move |event: leptos::ev::Event| {
        let mut updated = config_for_align.clone();
        updated.align = match event_target_value(&event).as_str() {
            "left" => TextAlign::Left,
            "right" => TextAlign::Right,
            _ => TextAlign::Center,
        };
        patch_align(api::UpdateOverlaySlotRequest {
            source: Some(OverlaySource::Text(updated)),
            ..Default::default()
        });
    };

    let text = config.text.clone();
    let font_size = config.font_size.to_string();
    let color = config.color.clone();
    let align_value = match config.align {
        TextAlign::Left => "left",
        TextAlign::Center => "center",
        TextAlign::Right => "right",
    };

    view! {
        <InspectorField label="Text">
            <input
                type="text"
                class="w-full rounded-sm border border-edge-subtle bg-surface-overlay/60 px-2 py-1 text-xs text-fg-primary focus:border-accent-primary focus:outline-none"
                prop:value=text
                on:change=on_text
            />
        </InspectorField>
        <InspectorField label="Font size">
            <input
                type="number"
                min="1"
                class="w-full rounded-sm border border-edge-subtle bg-surface-overlay/60 px-2 py-1 text-xs text-fg-primary focus:border-accent-primary focus:outline-none"
                prop:value=font_size
                on:input=on_size
            />
        </InspectorField>
        <InspectorField label="Color">
            <input
                type="color"
                class="h-7 w-16 rounded-sm border border-edge-subtle bg-transparent"
                prop:value=color
                on:change=on_color
            />
        </InspectorField>
        <InspectorField label="Align">
            <select
                class="w-full rounded-sm border border-edge-subtle bg-surface-overlay/60 px-2 py-1 text-xs text-fg-primary focus:border-accent-primary focus:outline-none"
                on:change=on_align
            >
                <option value="left" selected=align_value == "left">"Left"</option>
                <option value="center" selected=align_value == "center">"Center"</option>
                <option value="right" selected=align_value == "right">"Right"</option>
            </select>
        </InspectorField>
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
