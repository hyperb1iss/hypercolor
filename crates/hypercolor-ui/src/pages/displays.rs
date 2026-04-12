//! `/displays` — LCD-equipped devices, HTML faces, and overlay stacks.
//!
//! Three-pane workspace (picker, live preview, overlay stack + inspector)
//! for assigning full-screen faces to LCD devices and layering optional
//! clock/sensor/image/text widgets on top before frames reach the panel.

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
use web_sys::PointerEvent;

use crate::api;
use crate::components::page_header::PageHeader;
use crate::icons::*;
use crate::toasts;

type DisplaysResource = LocalResource<Result<Vec<api::DisplaySummary>, String>>;

/// Polling cadence for the live preview JPEG, in milliseconds.
const PREVIEW_POLL_INTERVAL_MS: u64 = 500;

/// Live drag state for a slot being repositioned on the preview canvas.
#[derive(Clone)]
struct DragState {
    slot_id: OverlaySlotId,
    /// Resolved slot top-left in display pixels at drag start.
    start_slot_x: f64,
    start_slot_y: f64,
    /// Pointer position in CSS pixels relative to the container at drag start.
    start_pointer_x: f64,
    start_pointer_y: f64,
    /// Current pointer position in CSS pixels relative to the container.
    current_pointer_x: f64,
    current_pointer_y: f64,
    /// Display native resolution (needed for scale factor computation).
    display_width: u32,
    display_height: u32,
    /// Slot size in display pixels (carried through to anchor computation).
    slot_width: u32,
    slot_height: u32,
}

impl DragState {
    /// Current slot top-left position in display pixels, accounting for
    /// the pointer delta scaled from CSS pixels to display coordinates.
    fn current_display_position(&self, container_width: f64) -> (f64, f64) {
        let scale = if container_width > 0.0 {
            f64::from(self.display_width) / container_width
        } else {
            1.0
        };
        let dx = (self.current_pointer_x - self.start_pointer_x) * scale;
        let dy = (self.current_pointer_y - self.start_pointer_y) * scale;
        (self.start_slot_x + dx, self.start_slot_y + dy)
    }

    /// Convert the current display-pixel position back to an anchor +
    /// offset by finding the 9-point anchor that minimizes the residual
    /// offset magnitude.
    fn resolved_position(&self, container_width: f64) -> OverlayPosition {
        let (x, y) = self.current_display_position(container_width);
        let dw = self.display_width as i32;
        let dh = self.display_height as i32;
        let sw = self.slot_width as i32;
        let sh = self.slot_height as i32;

        let mut best_anchor = Anchor::TopLeft;
        let mut best_distance = i64::MAX;
        for row in &ANCHOR_GRID {
            for &anchor in row {
                let (base_x, base_y) = anchor_origin_ui(anchor, dw, dh, sw, sh);
                let off_x = x.round() as i64 - i64::from(base_x);
                let off_y = y.round() as i64 - i64::from(base_y);
                let distance = off_x.abs() + off_y.abs();
                if distance < best_distance {
                    best_distance = distance;
                    best_anchor = anchor;
                }
            }
        }

        let (base_x, base_y) = anchor_origin_ui(best_anchor, dw, dh, sw, sh);
        OverlayPosition::Anchored {
            anchor: best_anchor,
            offset_x: (x.round() as i32).saturating_sub(base_x),
            offset_y: (y.round() as i32).saturating_sub(base_y),
            width: self.slot_width,
            height: self.slot_height,
        }
    }
}

#[component]
pub fn DisplaysPage() -> impl IntoView {
    let displays: DisplaysResource = LocalResource::new(api::fetch_displays);
    let (selected_id, set_selected_id) = signal(None::<String>);
    let (simulator_modal_open, set_simulator_modal_open) = signal(false);
    let (editing_simulator, set_editing_simulator) = signal(None::<api::DisplaySummary>);

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
    let open_simulator_modal = Callback::new(move |_| set_simulator_modal_open.set(true));
    let close_simulator_modal = Callback::new(move |_| set_simulator_modal_open.set(false));
    let open_simulator_editor =
        Callback::new(move |display: api::DisplaySummary| set_editing_simulator.set(Some(display)));
    let close_simulator_editor = Callback::new(move |_| set_editing_simulator.set(None));
    let on_simulator_created = Callback::new(move |summary: api::SimulatedDisplaySummary| {
        displays.refetch();
        set_selected_id.set(Some(summary.id));
        set_simulator_modal_open.set(false);
    });
    let on_simulator_updated = Callback::new(move |summary: api::SimulatedDisplaySummary| {
        displays.refetch();
        set_selected_id.set(Some(summary.id));
        set_editing_simulator.set(None);
    });
    let on_simulator_deleted = Callback::new(move |id: String| {
        displays.refetch();
        if selected_id.get_untracked().as_deref() == Some(id.as_str()) {
            set_selected_id.set(None);
        }
        set_editing_simulator.set(None);
    });

    view! {
        <div class="flex h-full flex-col overflow-hidden animate-fade-in">
            <div class="shrink-0 glass-subtle border-b border-edge-default">
                <div class="px-6 pt-5 pb-4">
                    <PageHeader
                        icon=LuMonitor
                        title="Displays"
                        subtitle="Assign HTML faces to LCD devices and layer optional overlays on top."
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
                    on_create_simulator=open_simulator_modal
                    on_manage_simulator=open_simulator_editor
                />
                <DisplayWorkspace selected_display=selected_display />
                <OverlayStackPanel selected_id=selected_id />
            </div>
            <Show when=move || simulator_modal_open.get() fallback=|| ()>
                <CreateSimulatorModal
                    on_created=on_simulator_created
                    on_close=close_simulator_modal
                />
            </Show>
            <Show when=move || editing_simulator.with(Option::is_some) fallback=|| ()>
                {move || {
                    editing_simulator.get().map(|display| {
                        view! {
                            <EditSimulatorModal
                                display=display
                                on_updated=on_simulator_updated
                                on_deleted=on_simulator_deleted
                                on_close=close_simulator_editor
                            />
                        }
                    })
                }}
            </Show>
        </div>
    }
}

#[component]
fn DisplayPicker(
    displays: DisplaysResource,
    selected_id: ReadSignal<Option<String>>,
    set_selected_id: WriteSignal<Option<String>>,
    on_create_simulator: Callback<()>,
    on_manage_simulator: Callback<api::DisplaySummary>,
) -> impl IntoView {
    view! {
        <aside class="flex min-h-0 flex-col overflow-hidden rounded-lg border border-edge-subtle bg-surface-raised">
            <header class="flex items-center justify-between border-b border-edge-subtle px-3 py-2">
                <div class="flex items-center gap-2 text-xs uppercase tracking-wider text-fg-tertiary">
                    <Icon icon=LuMonitor width="14" height="14" />
                    "Displays"
                </div>
                <div class="flex items-center gap-1">
                    <button
                        type="button"
                        class="flex items-center gap-1 rounded-sm px-2 py-1 text-[10px] uppercase tracking-wider text-fg-tertiary transition hover:text-accent-primary"
                        title="Add a virtual display simulator"
                        on:click=move |_| on_create_simulator.run(())
                    >
                        <Icon icon=LuPlus width="12" height="12" />
                        "Simulator"
                    </button>
                    <button
                        type="button"
                        class="rounded-sm p-1 text-fg-tertiary transition hover:text-accent-primary"
                        title="Refresh displays"
                        on:click=move |_| displays.refetch()
                    >
                        <Icon icon=LuRefreshCw width="14" height="14" />
                    </button>
                </div>
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
                                <div class="flex flex-col gap-3">
                                    <PickerPlaceholder
                                        message="No LCD devices connected. Connect a Corsair iCUE LINK pump, an Ableton Push 2, or add a virtual display simulator.".to_string()
                                    />
                                    <button
                                        type="button"
                                        class="inline-flex items-center gap-2 self-start rounded-md border border-accent-primary/35 bg-accent-primary/10 px-3 py-1.5 text-[11px] font-medium uppercase tracking-wider text-accent-primary transition hover:bg-accent-primary/15"
                                        on:click=move |_| on_create_simulator.run(())
                                    >
                                        <Icon icon=LuPlus width="12" height="12" />
                                        "Create simulator"
                                    </button>
                                </div>
                            }
                            .into_any(),
                            Ok(items) => {
                                let rows = items
                                    .clone()
                                    .into_iter()
                                    .map(|display| {
                                        render_picker_row(
                                            display,
                                            selected_id,
                                            set_selected_id,
                                            on_manage_simulator,
                                        )
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
    on_manage_simulator: Callback<api::DisplaySummary>,
) -> impl IntoView {
    let id = display.id.clone();
    let id_for_click = id.clone();
    let id_for_active = id.clone();
    let is_active = Signal::derive(move || {
        selected_id.with(|current| current.as_deref() == Some(id_for_active.as_str()))
    });
    let is_simulator = is_simulator_display(&display);
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
    let display_for_manage = display.clone();
    let vendor = display.vendor;
    let name = display.name;

    view! {
        <li class="flex items-stretch">
            <button
                type="button"
                class="flex min-w-0 flex-1 flex-col items-start gap-1 px-3 py-2 text-left transition hover:bg-surface-overlay"
                class:bg-surface-overlay=move || is_active.get()
                class:border-l-2=move || is_active.get()
                class:border-accent-primary=move || is_active.get()
                on:click=move |_| set_selected_id.set(Some(id_for_click.clone()))
            >
                <span class="flex w-full items-center justify-between gap-2">
                    <span class="flex min-w-0 items-center gap-2">
                        <span class="truncate text-sm font-medium text-fg-primary">{name}</span>
                        {if is_simulator {
                            view! {
                                <span class="shrink-0 rounded-sm border border-accent-primary/25 bg-accent-primary/10 px-1.5 py-0.5 text-[10px] uppercase tracking-wider text-accent-primary">
                                    "Simulator"
                                </span>
                            }
                            .into_any()
                        } else {
                            ().into_any()
                        }}
                    </span>
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
            {if is_simulator {
                view! {
                    <button
                        type="button"
                        class="mx-2 my-2 shrink-0 self-start rounded-md border border-edge-subtle bg-surface-overlay/60 p-2 text-fg-tertiary transition hover:border-accent-primary/40 hover:text-accent-primary"
                        title="Edit simulator"
                        on:click=move |_| on_manage_simulator.run(display_for_manage.clone())
                    >
                        <Icon icon=LuSettings2 width="13" height="13" />
                    </button>
                }
                .into_any()
            } else {
                ().into_any()
            }}
        </li>
    }
}

pub(crate) fn is_simulator_display(display: &api::DisplaySummary) -> bool {
    display.family.eq_ignore_ascii_case("simulator")
}

pub(crate) fn parse_simulator_dimension(raw: &str, label: &str) -> Result<u32, String> {
    raw.trim()
        .parse::<u32>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| format!("{label} must be a positive number."))
}

pub(crate) fn display_preview_shell_url(display_id: &str) -> String {
    format!("/preview?display={display_id}")
}

#[component]
fn PickerPlaceholder(#[prop(into)] message: String) -> impl IntoView {
    view! {
        <div class="px-3 py-6 text-xs leading-relaxed text-fg-tertiary">{message}</div>
    }
}

#[component]
fn CreateSimulatorModal(
    #[prop(into)] on_created: Callback<api::SimulatedDisplaySummary>,
    #[prop(into)] on_close: Callback<()>,
) -> impl IntoView {
    let (name, set_name) = signal("Preview Simulator".to_string());
    let (width, set_width) = signal("480".to_string());
    let (height, set_height) = signal("480".to_string());
    let (circular, set_circular) = signal(true);
    let (submitting, set_submitting) = signal(false);
    let (error, set_error) = signal(None::<String>);

    let submit = {
        let on_created = on_created.clone();
        move |event: leptos::ev::SubmitEvent| {
            event.prevent_default();
            if submitting.get_untracked() {
                return;
            }

            let width_raw = width.get_untracked();
            let Ok(width) = parse_simulator_dimension(&width_raw, "Width") else {
                set_error.set(parse_simulator_dimension(&width_raw, "Width").err());
                return;
            };
            let height_raw = height.get_untracked();
            let Ok(height) = parse_simulator_dimension(&height_raw, "Height") else {
                set_error.set(parse_simulator_dimension(&height_raw, "Height").err());
                return;
            };

            set_submitting.set(true);
            set_error.set(None);
            let request = api::CreateSimulatedDisplayRequest {
                name: name.get_untracked(),
                width,
                height,
                circular: circular.get_untracked(),
                enabled: true,
            };

            spawn_local(async move {
                match api::create_simulated_display(&request).await {
                    Ok(summary) => {
                        toasts::toast_success("Simulator created");
                        on_created.run(summary);
                    }
                    Err(message) => {
                        set_error.set(Some(message));
                        set_submitting.set(false);
                    }
                }
            });
        }
    };

    let close_backdrop = on_close.clone();
    let close_button = on_close.clone();

    view! {
        <div
            class="absolute inset-0 z-20 flex items-center justify-center bg-black/60 p-4 backdrop-blur-sm"
            on:click=move |_| close_backdrop.run(())
        >
            <div
                class="w-full max-w-md rounded-xl border border-edge-subtle bg-surface-raised p-4 shadow-2xl"
                on:click=|event| event.stop_propagation()
            >
                <div class="mb-4 flex items-start justify-between gap-3">
                    <div>
                        <h2 class="text-sm font-semibold text-fg-primary">"Create simulator"</h2>
                        <p class="mt-1 text-[11px] leading-relaxed text-fg-tertiary">
                            "Spin up a software LCD that appears in the display list and preview pipeline."
                        </p>
                    </div>
                    <button
                        type="button"
                        class="rounded-sm p-1 text-fg-tertiary transition hover:text-accent-primary"
                        title="Close"
                        on:click=move |_| close_button.run(())
                    >
                        <Icon icon=LuX width="14" height="14" />
                    </button>
                </div>

                <form class="flex flex-col gap-3" on:submit=submit>
                    <label class="flex flex-col gap-1">
                        <span class="text-[11px] uppercase tracking-wider text-fg-tertiary">
                            "Name"
                        </span>
                        <input
                            type="text"
                            class="rounded-md border border-edge-subtle bg-surface-overlay px-3 py-2 text-sm text-fg-primary outline-none transition focus:border-accent-primary"
                            prop:value=move || name.get()
                            on:input=move |event| set_name.set(event_target_value(&event))
                        />
                    </label>

                    <div class="grid grid-cols-2 gap-3">
                        <label class="flex flex-col gap-1">
                            <span class="text-[11px] uppercase tracking-wider text-fg-tertiary">
                                "Width"
                            </span>
                            <input
                                type="number"
                                min="1"
                                max="4096"
                                class="rounded-md border border-edge-subtle bg-surface-overlay px-3 py-2 text-sm text-fg-primary outline-none transition focus:border-accent-primary"
                                prop:value=move || width.get()
                                on:input=move |event| set_width.set(event_target_value(&event))
                            />
                        </label>
                        <label class="flex flex-col gap-1">
                            <span class="text-[11px] uppercase tracking-wider text-fg-tertiary">
                                "Height"
                            </span>
                            <input
                                type="number"
                                min="1"
                                max="4096"
                                class="rounded-md border border-edge-subtle bg-surface-overlay px-3 py-2 text-sm text-fg-primary outline-none transition focus:border-accent-primary"
                                prop:value=move || height.get()
                                on:input=move |event| set_height.set(event_target_value(&event))
                            />
                        </label>
                    </div>

                    <div class="flex flex-col gap-1">
                        <span class="text-[11px] uppercase tracking-wider text-fg-tertiary">
                            "Shape"
                        </span>
                        <div class="grid grid-cols-2 gap-2">
                            <button
                                type="button"
                                class=move || {
                                    if circular.get() {
                                        "flex items-center justify-center gap-2 rounded-md border border-accent-primary bg-accent-primary/10 px-3 py-2 text-sm text-accent-primary transition"
                                    } else {
                                        "flex items-center justify-center gap-2 rounded-md border border-edge-subtle bg-surface-overlay px-3 py-2 text-sm text-fg-tertiary transition hover:border-accent-primary/35"
                                    }
                                }
                                on:click=move |_| set_circular.set(true)
                            >
                                <Icon icon=LuCircle width="13" height="13" />
                                "Round"
                            </button>
                            <button
                                type="button"
                                class=move || {
                                    if circular.get() {
                                        "flex items-center justify-center gap-2 rounded-md border border-edge-subtle bg-surface-overlay px-3 py-2 text-sm text-fg-tertiary transition hover:border-accent-primary/35"
                                    } else {
                                        "flex items-center justify-center gap-2 rounded-md border border-accent-primary bg-accent-primary/10 px-3 py-2 text-sm text-accent-primary transition"
                                    }
                                }
                                on:click=move |_| set_circular.set(false)
                            >
                                <Icon icon=LuSquare width="13" height="13" />
                                "Square"
                            </button>
                        </div>
                    </div>

                    <Show when=move || error.with(Option::is_some) fallback=|| ()>
                        <div class="rounded-md border border-status-error/35 bg-status-error/10 px-3 py-2 text-xs text-status-error">
                            {move || error.get().unwrap_or_default()}
                        </div>
                    </Show>

                    <div class="mt-1 flex items-center justify-end gap-2">
                        <button
                            type="button"
                            class="rounded-md px-3 py-2 text-xs uppercase tracking-wider text-fg-tertiary transition hover:text-fg-primary"
                            on:click=move |_| on_close.run(())
                        >
                            "Cancel"
                        </button>
                        <button
                            type="submit"
                            class="inline-flex items-center gap-2 rounded-md border border-accent-primary/40 bg-accent-primary/12 px-3 py-2 text-xs font-medium uppercase tracking-wider text-accent-primary transition hover:bg-accent-primary/18 disabled:cursor-not-allowed disabled:opacity-50"
                            disabled=move || submitting.get()
                        >
                            <Icon icon=LuPlus width="12" height="12" />
                            {move || if submitting.get() { "Creating..." } else { "Create simulator" }}
                        </button>
                    </div>
                </form>
            </div>
        </div>
    }
}

#[component]
fn EditSimulatorModal(
    display: api::DisplaySummary,
    #[prop(into)] on_updated: Callback<api::SimulatedDisplaySummary>,
    #[prop(into)] on_deleted: Callback<String>,
    #[prop(into)] on_close: Callback<()>,
) -> impl IntoView {
    let display_id = display.id.clone();
    let display_name = display.name.clone();
    let (name, set_name) = signal(display.name.clone());
    let (width, set_width) = signal(display.width.to_string());
    let (height, set_height) = signal(display.height.to_string());
    let (circular, set_circular) = signal(display.circular);
    let (submitting, set_submitting) = signal(false);
    let (error, set_error) = signal(None::<String>);

    let submit = {
        let on_updated = on_updated.clone();
        let display_id = display_id.clone();
        move |event: leptos::ev::SubmitEvent| {
            event.prevent_default();
            if submitting.get_untracked() {
                return;
            }

            let width_raw = width.get_untracked();
            let Ok(width) = parse_simulator_dimension(&width_raw, "Width") else {
                set_error.set(parse_simulator_dimension(&width_raw, "Width").err());
                return;
            };
            let height_raw = height.get_untracked();
            let Ok(height) = parse_simulator_dimension(&height_raw, "Height") else {
                set_error.set(parse_simulator_dimension(&height_raw, "Height").err());
                return;
            };

            set_submitting.set(true);
            set_error.set(None);
            let request = api::UpdateSimulatedDisplayRequest {
                name: Some(name.get_untracked()),
                width: Some(width),
                height: Some(height),
                circular: Some(circular.get_untracked()),
                enabled: None,
            };

            let display_id = display_id.clone();
            let success_name = name.get_untracked();
            spawn_local(async move {
                match api::patch_simulated_display(&display_id, &request).await {
                    Ok(summary) => {
                        toasts::toast_success(&format!("Updated {success_name}"));
                        on_updated.run(summary);
                    }
                    Err(message) => {
                        set_error.set(Some(message));
                        set_submitting.set(false);
                    }
                }
            });
        }
    };

    let delete_simulator = {
        let on_deleted = on_deleted.clone();
        let display_id = display_id.clone();
        move |_| {
            if submitting.get_untracked() {
                return;
            }

            set_submitting.set(true);
            set_error.set(None);
            let deleted_id = display_id.clone();
            let deleted_name = display_name.clone();
            spawn_local(async move {
                match api::delete_simulated_display(&deleted_id).await {
                    Ok(()) => {
                        toasts::toast_success(&format!("Deleted {deleted_name}"));
                        on_deleted.run(deleted_id.clone());
                    }
                    Err(message) => {
                        set_error.set(Some(message));
                        set_submitting.set(false);
                    }
                }
            });
        }
    };

    let close_backdrop = on_close.clone();
    let close_button = on_close.clone();

    view! {
        <div
            class="absolute inset-0 z-20 flex items-center justify-center bg-black/60 p-4 backdrop-blur-sm"
            on:click=move |_| close_backdrop.run(())
        >
            <div
                class="w-full max-w-md rounded-xl border border-edge-subtle bg-surface-raised p-4 shadow-2xl"
                on:click=|event| event.stop_propagation()
            >
                <div class="mb-4 flex items-start justify-between gap-3">
                    <div>
                        <h2 class="text-sm font-semibold text-fg-primary">"Simulator settings"</h2>
                        <p class="mt-1 text-[11px] leading-relaxed text-fg-tertiary">
                            "Adjust this virtual LCD or remove it from the daemon."
                        </p>
                    </div>
                    <button
                        type="button"
                        class="rounded-sm p-1 text-fg-tertiary transition hover:text-accent-primary"
                        title="Close"
                        on:click=move |_| close_button.run(())
                    >
                        <Icon icon=LuX width="14" height="14" />
                    </button>
                </div>

                <form class="flex flex-col gap-3" on:submit=submit>
                    <label class="flex flex-col gap-1">
                        <span class="text-[11px] uppercase tracking-wider text-fg-tertiary">
                            "Name"
                        </span>
                        <input
                            type="text"
                            class="rounded-md border border-edge-subtle bg-surface-overlay px-3 py-2 text-sm text-fg-primary outline-none transition focus:border-accent-primary"
                            prop:value=move || name.get()
                            on:input=move |event| set_name.set(event_target_value(&event))
                        />
                    </label>

                    <div class="grid grid-cols-2 gap-3">
                        <label class="flex flex-col gap-1">
                            <span class="text-[11px] uppercase tracking-wider text-fg-tertiary">
                                "Width"
                            </span>
                            <input
                                type="number"
                                min="1"
                                max="4096"
                                class="rounded-md border border-edge-subtle bg-surface-overlay px-3 py-2 text-sm text-fg-primary outline-none transition focus:border-accent-primary"
                                prop:value=move || width.get()
                                on:input=move |event| set_width.set(event_target_value(&event))
                            />
                        </label>
                        <label class="flex flex-col gap-1">
                            <span class="text-[11px] uppercase tracking-wider text-fg-tertiary">
                                "Height"
                            </span>
                            <input
                                type="number"
                                min="1"
                                max="4096"
                                class="rounded-md border border-edge-subtle bg-surface-overlay px-3 py-2 text-sm text-fg-primary outline-none transition focus:border-accent-primary"
                                prop:value=move || height.get()
                                on:input=move |event| set_height.set(event_target_value(&event))
                            />
                        </label>
                    </div>

                    <div class="flex flex-col gap-1">
                        <span class="text-[11px] uppercase tracking-wider text-fg-tertiary">
                            "Shape"
                        </span>
                        <div class="grid grid-cols-2 gap-2">
                            <button
                                type="button"
                                class=move || {
                                    if circular.get() {
                                        "flex items-center justify-center gap-2 rounded-md border border-accent-primary bg-accent-primary/10 px-3 py-2 text-sm text-accent-primary transition"
                                    } else {
                                        "flex items-center justify-center gap-2 rounded-md border border-edge-subtle bg-surface-overlay px-3 py-2 text-sm text-fg-tertiary transition hover:border-accent-primary/35"
                                    }
                                }
                                on:click=move |_| set_circular.set(true)
                            >
                                <Icon icon=LuCircle width="13" height="13" />
                                "Round"
                            </button>
                            <button
                                type="button"
                                class=move || {
                                    if circular.get() {
                                        "flex items-center justify-center gap-2 rounded-md border border-edge-subtle bg-surface-overlay px-3 py-2 text-sm text-fg-tertiary transition hover:border-accent-primary/35"
                                    } else {
                                        "flex items-center justify-center gap-2 rounded-md border border-accent-primary bg-accent-primary/10 px-3 py-2 text-sm text-accent-primary transition"
                                    }
                                }
                                on:click=move |_| set_circular.set(false)
                            >
                                <Icon icon=LuSquare width="13" height="13" />
                                "Square"
                            </button>
                        </div>
                    </div>

                    <Show when=move || error.with(Option::is_some) fallback=|| ()>
                        <div class="rounded-md border border-status-error/35 bg-status-error/10 px-3 py-2 text-xs text-status-error">
                            {move || error.get().unwrap_or_default()}
                        </div>
                    </Show>

                    <div class="mt-1 flex items-center justify-between gap-2">
                        <button
                            type="button"
                            class="inline-flex items-center gap-2 rounded-md border border-status-error/35 bg-status-error/10 px-3 py-2 text-xs font-medium uppercase tracking-wider text-status-error transition hover:bg-status-error/15 disabled:cursor-not-allowed disabled:opacity-50"
                            disabled=move || submitting.get()
                            on:click=delete_simulator
                        >
                            <Icon icon=LuTrash2 width="12" height="12" />
                            "Delete simulator"
                        </button>
                        <div class="flex items-center gap-2">
                            <button
                                type="button"
                                class="rounded-md px-3 py-2 text-xs uppercase tracking-wider text-fg-tertiary transition hover:text-fg-primary"
                                on:click=move |_| on_close.run(())
                            >
                                "Cancel"
                            </button>
                            <button
                                type="submit"
                                class="inline-flex items-center gap-2 rounded-md border border-accent-primary/40 bg-accent-primary/12 px-3 py-2 text-xs font-medium uppercase tracking-wider text-accent-primary transition hover:bg-accent-primary/18 disabled:cursor-not-allowed disabled:opacity-50"
                                disabled=move || submitting.get()
                            >
                                <Icon icon=LuSave width="12" height="12" />
                                {move || if submitting.get() { "Saving..." } else { "Save simulator" }}
                            </button>
                        </div>
                    </div>
                </form>
            </div>
        </div>
    }
}

#[component]
fn DisplayWorkspace(selected_display: Memo<Option<api::DisplaySummary>>) -> impl IntoView {
    let (poll_counter, set_poll_counter) = signal(0_u64);
    let (display_face, set_display_face) =
        signal(None::<Result<Option<api::DisplayFaceResponse>, String>>);
    let (face_catalog, set_face_catalog) = signal(None::<Result<Vec<api::EffectSummary>, String>>);
    let (face_picker_open, set_face_picker_open) = signal(false);
    let (face_assignment_pending, set_face_assignment_pending) = signal(false);

    // Independent overlay config fetch so we can paint slot outlines on the
    // preview image without coupling to OverlayStackPanel's internal state.
    let (workspace_overlay_config, set_workspace_overlay_config) =
        signal(None::<DisplayOverlayConfig>);
    let (drag_state, set_drag_state) = signal(None::<DragState>);
    let container_ref = NodeRef::<leptos::html::Div>::new();
    Effect::new(move |_| {
        let Some(display) = selected_display.get() else {
            set_workspace_overlay_config.set(None);
            set_display_face.set(None);
            set_face_picker_open.set(false);
            set_face_assignment_pending.set(false);
            return;
        };
        let display_id = display.id.clone();
        let requested_id = display_id.clone();
        spawn_local(async move {
            if let Ok(config) = api::fetch_display_overlays(&display_id).await {
                set_workspace_overlay_config.set(Some(config));
            }
        });
        spawn_local(async move {
            let result = api::fetch_display_face(&requested_id).await;
            if selected_display
                .get_untracked()
                .as_ref()
                .is_some_and(|current| current.id == requested_id)
            {
                set_display_face.set(Some(result));
            }
        });
    });
    // Refetch overlays alongside each preview poll so newly-added slots
    // appear on the canvas within one poll cycle.
    Effect::new(move |_| {
        let _ts = poll_counter.get();
        let Some(display) = selected_display.get_untracked() else {
            return;
        };
        let display_id = display.id.clone();
        spawn_local(async move {
            if let Ok(config) = api::fetch_display_overlays(&display_id).await {
                set_workspace_overlay_config.set(Some(config));
            }
        });
    });
    Effect::new(move |_| {
        if !face_picker_open.get() || face_catalog.with(Option::is_some) {
            return;
        }
        spawn_local(async move {
            let result = api::fetch_effects_by_category("display").await;
            set_face_catalog.set(Some(result));
        });
    });

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
    let current_face_name = Signal::derive(move || match display_face.get() {
        None => "Loading face...".to_owned(),
        Some(Ok(Some(face))) => face.effect.name,
        Some(Ok(None)) => "No face assigned".to_owned(),
        Some(Err(_)) => "Face unavailable".to_owned(),
    });
    let current_face_description = Signal::derive(move || match display_face.get() {
        Some(Ok(Some(face))) => face.effect.description,
        Some(Err(error)) => error,
        _ => "Display faces run as dedicated HTML effects at the panel's native resolution."
            .to_owned(),
    });
    let current_face_id = Signal::derive(move || {
        display_face
            .get()
            .and_then(Result::ok)
            .flatten()
            .map(|face| face.effect.id)
    });
    let assign_face = Callback::new(move |effect_id: String| {
        let Some(display) = selected_display.get_untracked() else {
            return;
        };
        let display_id = display.id.clone();
        let display_name = display.name.clone();
        set_face_assignment_pending.set(true);
        spawn_local(async move {
            match api::set_display_face(&display_id, &effect_id).await {
                Ok(face) => {
                    let assigned_name = face.effect.name.clone();
                    set_display_face.set(Some(Ok(Some(face))));
                    set_face_picker_open.set(false);
                    set_face_assignment_pending.set(false);
                    set_poll_counter.update(|value| *value = value.wrapping_add(1));
                    toasts::toast_success(&format!("Assigned {assigned_name} to {display_name}"));
                }
                Err(error) => {
                    set_face_assignment_pending.set(false);
                    toasts::toast_error(&format!("Face assignment failed: {error}"));
                }
            }
        });
    });
    let clear_face = Callback::new(move |_| {
        let Some(display) = selected_display.get_untracked() else {
            return;
        };
        let display_id = display.id.clone();
        let display_name = display.name.clone();
        set_face_assignment_pending.set(true);
        spawn_local(async move {
            match api::delete_display_face(&display_id).await {
                Ok(()) => {
                    set_display_face.set(Some(Ok(None)));
                    set_face_assignment_pending.set(false);
                    set_poll_counter.update(|value| *value = value.wrapping_add(1));
                    toasts::toast_success(&format!("Cleared face from {display_name}"));
                }
                Err(error) => {
                    set_face_assignment_pending.set(false);
                    toasts::toast_error(&format!("Could not clear display face: {error}"));
                }
            }
        });
    });

    view! {
        <section class="flex min-h-0 flex-col overflow-hidden rounded-lg border border-edge-subtle bg-surface-raised">
            <header class="flex flex-wrap items-start justify-between gap-3 border-b border-edge-subtle px-3 py-3">
                <div class="flex min-w-0 flex-1 flex-col gap-2">
                    <div class="text-xs uppercase tracking-wider text-fg-tertiary">
                        "Live preview"
                    </div>
                    <Show when=move || selected_display.with(Option::is_some) fallback=|| ()>
                        <div class="flex min-w-0 flex-wrap items-center gap-2">
                            <div class="rounded-full border border-edge-subtle bg-surface-overlay/60 px-2.5 py-1 text-[11px] text-fg-secondary">
                                {move || subtitle.get().unwrap_or_default()}
                            </div>
                            <div class="inline-flex min-w-0 items-center gap-2 rounded-full border border-coral/20 bg-coral/10 px-2.5 py-1 text-[11px] text-coral">
                                <Icon icon=LuLayers width="11" height="11" />
                                <span class="truncate">{move || current_face_name.get()}</span>
                            </div>
                        </div>
                        <p class="text-[11px] leading-relaxed text-fg-tertiary">
                            {move || current_face_description.get()}
                        </p>
                    </Show>
                </div>
                <div class="flex items-center gap-2">
                    <Show when=move || selected_display.with(Option::is_some) fallback=|| ()>
                        <button
                            type="button"
                            class="inline-flex items-center gap-1.5 rounded-md border border-accent-primary/35 bg-accent-primary/10 px-3 py-1.5 text-[11px] uppercase tracking-wider text-accent-primary transition hover:bg-accent-primary/15 disabled:cursor-not-allowed disabled:opacity-50"
                            disabled=move || face_assignment_pending.get()
                            on:click=move |_| set_face_picker_open.set(true)
                        >
                            <Icon icon=LuLayers width="12" height="12" />
                            "Choose face"
                        </button>
                        <button
                            type="button"
                            class="inline-flex items-center gap-1.5 rounded-md border border-edge-subtle bg-surface-overlay px-3 py-1.5 text-[11px] uppercase tracking-wider text-fg-tertiary transition hover:border-accent-primary/35 hover:text-accent-primary disabled:cursor-not-allowed disabled:opacity-40"
                            disabled=move || {
                                face_assignment_pending.get() || current_face_id.get().is_none()
                            }
                            on:click=move |_| clear_face.run(())
                        >
                            <Icon icon=LuX width="12" height="12" />
                            "Clear"
                        </button>
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
                    </Show>
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
                    let display_id = display.id.clone();
                    let src = api::display_preview_url(&display.id, Some(ts));
                    let aspect = format!("{} / {}", display.width, display.height);
                    let rounded_class = if display.circular {
                        "rounded-full"
                    } else {
                        "rounded-md"
                    };
                    let alt_text = format!("Live preview of {}", display.name);
                    let container_class = format!(
                        "relative max-h-full max-w-full overflow-hidden border border-edge-subtle bg-black shadow-lg select-none {rounded_class}"
                    );
                    let dw = display.width;
                    let dh = display.height;
                    let overlays = workspace_overlay_config
                        .get()
                        .map(|config| {
                            config
                                .overlays
                                .into_iter()
                                .filter(|slot| slot.enabled)
                                .map(move |slot| {
                                    render_slot_outline(
                                        slot,
                                        dw,
                                        dh,
                                        drag_state,
                                        set_drag_state,
                                        container_ref,
                                    )
                                })
                                .collect_view()
                        });

                    let on_pointermove = move |event: PointerEvent| {
                        if drag_state.with(Option::is_none) {
                            return;
                        }
                        let Some(container) = container_ref.get() else {
                            return;
                        };
                        let rect = container.get_bounding_client_rect();
                        let x = event.client_x() as f64 - rect.left();
                        let y = event.client_y() as f64 - rect.top();
                        set_drag_state.update(|state| {
                            if let Some(state) = state {
                                state.current_pointer_x = x;
                                state.current_pointer_y = y;
                            }
                        });
                    };

                    let on_pointerup = {
                        let display_id = display_id.clone();
                        move |_: PointerEvent| {
                            let Some(state) = drag_state.get() else {
                                return;
                            };
                            let Some(container) = container_ref.get() else {
                                set_drag_state.set(None);
                                return;
                            };
                            let container_width = container.get_bounding_client_rect().width();
                            let new_position = state.resolved_position(container_width);
                            let slot_id = state.slot_id;
                            let display_id = display_id.clone();
                            set_drag_state.set(None);
                            spawn_local(async move {
                                let body = api::UpdateOverlaySlotRequest {
                                    position: Some(new_position),
                                    ..Default::default()
                                };
                                if let Err(error) =
                                    api::patch_overlay_slot(&display_id, slot_id, &body).await
                                {
                                    toasts::toast_error(&format!(
                                        "Reposition failed: {error}"
                                    ));
                                }
                            });
                        }
                    };

                    let on_pointerleave = move |_: PointerEvent| {
                        set_drag_state.set(None);
                    };

                    view! {
                        <div
                            node_ref=container_ref
                            class=container_class
                            style=move || format!("aspect-ratio: {aspect}; cursor: {};", if drag_state.with(Option::is_some) { "grabbing" } else { "default" })
                            on:pointermove=on_pointermove
                            on:pointerup=on_pointerup
                            on:pointerleave=on_pointerleave
                        >
                            <img
                                class="pointer-events-none h-full w-full object-cover"
                                src=src
                                alt=alt_text
                                loading="eager"
                                decoding="async"
                                draggable="false"
                            />
                            {overlays}
                        </div>
                    }.into_any()
                }}
            </div>
            <Show when=move || face_picker_open.get() fallback=|| ()>
                {move || {
                    selected_display.get().map(|display| {
                        view! {
                            <DisplayFacePickerModal
                                display_name=display.name
                                faces=face_catalog
                                current_face_id=current_face_id
                                assigning=face_assignment_pending
                                on_select=assign_face
                                on_close=move || set_face_picker_open.set(false)
                            />
                        }
                    })
                }}
            </Show>
        </section>
    }
}

#[component]
fn DisplayFacePickerModal(
    display_name: String,
    faces: ReadSignal<Option<Result<Vec<api::EffectSummary>, String>>>,
    current_face_id: Signal<Option<String>>,
    assigning: ReadSignal<bool>,
    #[prop(into)] on_select: Callback<String>,
    #[prop(into)] on_close: Callback<()>,
) -> impl IntoView {
    let (search, set_search) = signal(String::new());
    let close_backdrop = on_close.clone();
    let close_button = on_close.clone();

    view! {
        <div
            class="absolute inset-0 z-20 flex items-center justify-center bg-black/60 p-4 backdrop-blur-sm"
            on:click=move |_| close_backdrop.run(())
        >
            <div
                class="flex w-full max-w-2xl flex-col overflow-hidden rounded-xl border border-edge-subtle bg-surface-raised shadow-2xl"
                on:click=|event| event.stop_propagation()
            >
                <div class="flex items-start justify-between gap-3 border-b border-edge-subtle px-4 py-4">
                    <div>
                        <h2 class="text-sm font-semibold text-fg-primary">"Choose display face"</h2>
                        <p class="mt-1 text-[11px] leading-relaxed text-fg-tertiary">
                            {format!(
                                "Assign a full-screen HTML face to {display_name}. Overlays stay available for optional gauges and labels on top."
                            )}
                        </p>
                    </div>
                    <button
                        type="button"
                        class="rounded-sm p-1 text-fg-tertiary transition hover:text-accent-primary"
                        title="Close"
                        on:click=move |_| close_button.run(())
                    >
                        <Icon icon=LuX width="14" height="14" />
                    </button>
                </div>
                <div class="border-b border-edge-subtle px-4 py-3">
                    <input
                        type="search"
                        class="w-full rounded-md border border-edge-subtle bg-surface-overlay px-3 py-2 text-sm text-fg-primary outline-none transition focus:border-accent-primary"
                        placeholder="Search faces by name, author, or description"
                        prop:value=move || search.get()
                        on:input=move |event| set_search.set(event_target_value(&event))
                    />
                </div>
                <div class="min-h-0 max-h-[60vh] overflow-y-auto p-4">
                    {move || {
                        let query = search.get().trim().to_lowercase();
                        match faces.get() {
                            None => view! {
                                <div class="py-8 text-center text-sm text-fg-tertiary">
                                    "Loading display faces..."
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
                                        <div class="py-8 text-center text-sm text-fg-tertiary">
                                            "No display faces match that search."
                                        </div>
                                    }
                                    .into_any();
                                }

                                filtered
                                    .into_iter()
                                    .map(|effect| {
                                        let effect_id = effect.id.clone();
                                        let is_current = Signal::derive({
                                            let effect_id = effect_id.clone();
                                            move || {
                                                current_face_id.get().as_deref()
                                                    == Some(effect_id.as_str())
                                            }
                                        });
                                        let card_class = move || {
                                            if is_current.get() {
                                                "flex w-full flex-col gap-2 rounded-lg border border-coral/40 bg-coral/10 px-4 py-3 text-left transition"
                                            } else {
                                                "flex w-full flex-col gap-2 rounded-lg border border-edge-subtle bg-surface-overlay/40 px-4 py-3 text-left transition hover:border-accent-primary/35 hover:bg-surface-overlay"
                                            }
                                        };
                                        let name = effect.name;
                                        let author = effect.author;
                                        let description = effect.description;
                                        view! {
                                            <button
                                                type="button"
                                                class=card_class
                                                disabled=move || assigning.get()
                                                on:click=move |_| on_select.run(effect_id.clone())
                                            >
                                                <div class="flex items-start justify-between gap-3">
                                                    <div class="min-w-0">
                                                        <div class="flex flex-wrap items-center gap-2">
                                                            <span class="text-sm font-medium text-fg-primary">
                                                                {name}
                                                            </span>
                                                            <Show when=move || is_current.get() fallback=|| ()>
                                                                <span class="rounded-full border border-coral/30 bg-coral/15 px-2 py-0.5 text-[10px] uppercase tracking-wider text-coral">
                                                                    "Assigned"
                                                                </span>
                                                            </Show>
                                                        </div>
                                                        <div class="mt-1 text-[11px] uppercase tracking-wider text-fg-tertiary">
                                                            {author}
                                                        </div>
                                                    </div>
                                                    <span class="rounded-full border border-edge-subtle bg-surface-raised px-2 py-0.5 text-[10px] uppercase tracking-wider text-fg-tertiary">
                                                        "Display"
                                                    </span>
                                                </div>
                                                <p class="text-sm leading-relaxed text-fg-secondary">
                                                    {description}
                                                </p>
                                            </button>
                                        }
                                    })
                                    .collect_view()
                                    .into_any()
                            }
                        }
                    }}
                </div>
            </div>
        </div>
    }
}

#[component]
fn OverlayStackPanel(selected_id: ReadSignal<Option<String>>) -> impl IntoView {
    let (overlay_config, set_overlay_config) = signal(None::<Result<DisplayOverlayConfig, String>>);
    let (catalog_open, set_catalog_open) = signal(false);
    let (selected_slot_id, set_selected_slot_id) = signal(None::<OverlaySlotId>);
    let (runtime_map, set_runtime_map) = signal(std::collections::HashMap::<
        OverlaySlotId,
        api::OverlayRuntimeResponse,
    >::new());

    // Reload the overlay stack whenever the selected display changes. We
    // keep the last-good config in place until the new one arrives so the
    // panel doesn't flash an empty state mid-swap. Also clear any pinned
    // slot selection so the inspector doesn't linger on a stale id.
    Effect::new(move |_| {
        let Some(display_id) = selected_id.get() else {
            set_overlay_config.set(None);
            set_selected_slot_id.set(None);
            set_runtime_map.set(std::collections::HashMap::new());
            return;
        };
        set_selected_slot_id.set(None);
        set_runtime_map.set(std::collections::HashMap::new());
        let set_overlay_config = set_overlay_config;
        spawn_local(async move {
            let result = api::fetch_display_overlays(&display_id).await;
            set_overlay_config.set(Some(result));
        });
    });

    // Batched runtime polling. One request every 2s pulls status for
    // every slot, so stack list pills can show Failed/HtmlGated states
    // without issuing one GET per row.
    let fetch_runtime_map = move || {
        let Some(display_id) = selected_id.get_untracked() else {
            return;
        };
        let set_runtime_map = set_runtime_map;
        let requested_id = display_id.clone();
        spawn_local(async move {
            let Ok(entries) = api::fetch_overlay_runtimes(&display_id).await else {
                return;
            };
            // Only apply the response if the user is still looking at the
            // same display. Without this guard a late response from the
            // previous display would paint its runtime on the current one
            // until the next 2s tick, showing wrong status pills.
            if selected_id.get_untracked().as_deref() != Some(requested_id.as_str()) {
                return;
            }
            let map = entries
                .into_iter()
                .map(|entry| (entry.slot_id, entry.runtime))
                .collect();
            set_runtime_map.set(map);
        });
    };
    // Seed immediately on display change.
    Effect::new({
        let fetch_runtime_map = fetch_runtime_map.clone();
        move |_| {
            if selected_id.with(Option::is_some) {
                fetch_runtime_map();
            }
        }
    });
    let _runtime_map_interval = use_interval_fn(
        {
            let fetch_runtime_map = fetch_runtime_map.clone();
            move || fetch_runtime_map()
        },
        2000_u64,
    );

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

    let refresh = {
        let fetch_runtime_map = fetch_runtime_map.clone();
        move || {
            let Some(display_id) = selected_id.get_untracked() else {
                return;
            };
            let set_overlay_config = set_overlay_config;
            let requested_id = display_id.clone();
            spawn_local(async move {
                let result = api::fetch_display_overlays(&display_id).await;
                // Drop the response if the display selection flipped while
                // the request was in flight. Otherwise we'd paint the old
                // display's stack onto the new one.
                if selected_id.get_untracked().as_deref() != Some(requested_id.as_str()) {
                    return;
                }
                set_overlay_config.set(Some(result));
            });
            // Also refetch runtime so row pills catch up to the mutation
            // immediately rather than waiting up to two seconds for the
            // next interval tick.
            fetch_runtime_map();
        }
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
            match api::create_overlay_slot(&display_id, &body).await {
                Ok(_) => refresh(),
                Err(error) => toasts::toast_error(&format!("Could not add overlay: {error}")),
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
                                        let runtime_status = runtime_map
                                            .with(|map| map.get(&slot.id).map(|r| r.status));
                                        render_slot_row(
                                            slot,
                                            runtime_status,
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
                    " Rich HTML visuals now live in display faces. Use overlays here for clocks, gauges, labels, and images layered on top."
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
    runtime_status: Option<api::OverlaySlotStatus>,
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
    // Prefer the runtime-reported status from the batched aggregate
    // endpoint when it's available so Failed and HtmlGated states show
    // up on list rows. Fall back to the enabled flag while the first
    // runtime fetch is still in flight.
    let (status_label, status_class) = match runtime_status {
        Some(api::OverlaySlotStatus::Active) => ("Active", "bg-emerald-500/15 text-emerald-300"),
        Some(api::OverlaySlotStatus::Disabled) => {
            ("Disabled", "bg-fg-tertiary/15 text-fg-tertiary")
        }
        Some(api::OverlaySlotStatus::Failed) => ("Failed", "bg-status-error/20 text-status-error"),
        Some(api::OverlaySlotStatus::HtmlGated) => ("HTML gated", "bg-amber-500/20 text-amber-300"),
        None if slot.enabled => ("Active", "bg-emerald-500/15 text-emerald-300"),
        None => ("Disabled", "bg-fg-tertiary/15 text-fg-tertiary"),
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
                match api::patch_overlay_slot(&display_id, slot_id, &body).await {
                    Ok(_) => refresh(),
                    Err(error) => {
                        toasts::toast_error(&format!("Overlay toggle failed: {error}"));
                    }
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
                match api::delete_overlay_slot(&display_id, slot_id).await {
                    Ok(()) => refresh(),
                    Err(error) => {
                        toasts::toast_error(&format!("Overlay delete failed: {error}"));
                    }
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

    // The slot state lives in a StoredValue so each field handler mutates
    // the latest snapshot and subsequent PATCHes compound on top of earlier
    // edits. Without this, consecutive edits to different source fields
    // (e.g. range_min then range_max on a Sensor overlay) would race and
    // revert each other, because every closure captured `config.clone()`
    // at render time.
    let slot_state: StoredValue<OverlaySlot> = StoredValue::new(overlay_slot.clone());

    // Runtime telemetry polled every 2s for the currently-selected slot.
    let (runtime_state, set_runtime_state) = signal(None::<api::OverlayRuntimeResponse>);

    let fetch_runtime = move || {
        let Some(display_id) = selected_id.get_untracked() else {
            return;
        };
        let set_runtime_state = set_runtime_state;
        spawn_local(async move {
            if let Ok(response) = api::fetch_overlay_slot(&display_id, slot_id).await {
                set_runtime_state.set(Some(response.runtime));
            }
        });
    };

    // Fire once immediately so the diagnostics section is not empty on open.
    fetch_runtime();

    let fetch_runtime_interval = fetch_runtime.clone();
    let _runtime_interval = use_interval_fn(move || fetch_runtime_interval(), 2000_u64);

    let refresh_immediate = refresh.clone();
    let fetch_runtime_after_patch = fetch_runtime.clone();
    let patch = move |body: api::UpdateOverlaySlotRequest| {
        let Some(display_id) = selected_id.get_untracked() else {
            return;
        };
        let refresh = refresh_immediate.clone();
        let fetch_runtime_after_patch = fetch_runtime_after_patch.clone();
        spawn_local(async move {
            match api::patch_overlay_slot(&display_id, slot_id, &body).await {
                Ok(_) => {
                    refresh();
                    fetch_runtime_after_patch();
                }
                Err(error) => {
                    toasts::toast_error(&format!("Overlay update failed: {error}"));
                }
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
        OverlaySource::Clock(config) => {
            clock_inspector_fields(config, slot_state, patch.clone()).into_any()
        }
        OverlaySource::Sensor(config) => sensor_inspector_fields(
            config,
            slot_state,
            patch.clone(),
            patch_debounced.clone(),
        )
        .into_any(),
        OverlaySource::Image(config) => {
            image_inspector_fields(config, slot_state, patch.clone()).into_any()
        }
        OverlaySource::Text(config) => {
            text_inspector_fields(config, slot_state, patch.clone(), patch_debounced.clone())
                .into_any()
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
                    "Position"
                </div>
                {position_fields(overlay_slot.position.clone(), slot_state, patch.clone(), patch_debounced.clone())}
            </div>

            <div class="flex flex-col gap-2 border-t border-edge-subtle pt-2">
                <div class="text-[10px] uppercase tracking-wider text-fg-tertiary">
                    {format!("{source_label} settings")}
                </div>
                {source_editor}
            </div>

            <div class="flex flex-col gap-2 border-t border-edge-subtle pt-2">
                <div class="text-[10px] uppercase tracking-wider text-fg-tertiary">
                    "Diagnostics"
                </div>
                {move || render_runtime_diagnostics(runtime_state.get())}
            </div>
        </div>
    }
}

fn render_runtime_diagnostics(runtime: Option<api::OverlayRuntimeResponse>) -> impl IntoView {
    let Some(runtime) = runtime else {
        return view! {
            <div class="text-[11px] text-fg-tertiary">"Waiting for runtime telemetry..."</div>
        }
        .into_any();
    };

    let (status_label, status_class) = match runtime.status {
        api::OverlaySlotStatus::Active => ("Active", "bg-emerald-500/15 text-emerald-300"),
        api::OverlaySlotStatus::Disabled => ("Disabled", "bg-fg-tertiary/15 text-fg-tertiary"),
        api::OverlaySlotStatus::Failed => ("Failed", "bg-status-error/20 text-status-error"),
        api::OverlaySlotStatus::HtmlGated => ("HTML gated", "bg-amber-500/20 text-amber-300"),
    };

    let last_rendered = runtime
        .last_rendered_at
        .clone()
        .unwrap_or_else(|| "never".to_string());
    let failures = runtime.consecutive_failures;
    let backoff_row = runtime.backoff_until.clone().map(|deadline| {
        view! {
            <div class="rounded-sm bg-amber-500/10 px-2 py-1 text-[10px] text-amber-300">
                "Cooling down · retry at "
                <span class="font-mono">{deadline}</span>
            </div>
        }
    });
    let error_row = runtime.last_error.clone().map(|error| {
        view! {
            <div class="rounded-sm border border-status-error/30 bg-status-error/10 p-2 text-[11px] leading-relaxed text-status-error">
                <div class="font-semibold">"Last error"</div>
                <div class="mt-0.5 font-mono text-[10px]">{error}</div>
            </div>
        }
    });

    view! {
        <div class="flex flex-col gap-2">
            <div class="flex items-center gap-2 text-[11px] text-fg-tertiary">
                <span class=format!(
                    "rounded-sm px-1.5 py-0.5 text-[10px] uppercase tracking-wider {status_class}"
                )>
                    {status_label}
                </span>
                {(failures > 0).then(|| view! {
                    <span class="rounded-sm bg-status-error/15 px-1.5 py-0.5 text-[10px] text-status-error">
                        {format!("{failures} consecutive failures")}
                    </span>
                })}
            </div>
            <div class="text-[11px] text-fg-tertiary">
                "Last rendered "
                <span class="font-mono text-fg-secondary">{last_rendered}</span>
            </div>
            {backoff_row}
            {error_row}
        </div>
    }
    .into_any()
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

/// Apply a mutation to the live `OverlayPosition` stored in `slot_state`
/// and return the updated position. Mirrors the `mutate_*_source` helpers
/// so position edits compound the same way source edits do.
fn mutate_position(
    slot_state: StoredValue<OverlaySlot>,
    mutation: impl FnOnce(&mut OverlayPosition),
) -> OverlayPosition {
    slot_state.update_value(|slot| {
        mutation(&mut slot.position);
    });
    slot_state.with_value(|slot| slot.position.clone())
}

const ANCHOR_GRID: [[Anchor; 3]; 3] = [
    [Anchor::TopLeft, Anchor::TopCenter, Anchor::TopRight],
    [Anchor::CenterLeft, Anchor::Center, Anchor::CenterRight],
    [
        Anchor::BottomLeft,
        Anchor::BottomCenter,
        Anchor::BottomRight,
    ],
];

fn anchor_short_label(anchor: Anchor) -> &'static str {
    match anchor {
        Anchor::TopLeft => "TL",
        Anchor::TopCenter => "TC",
        Anchor::TopRight => "TR",
        Anchor::CenterLeft => "CL",
        Anchor::Center => "C",
        Anchor::CenterRight => "CR",
        Anchor::BottomLeft => "BL",
        Anchor::BottomCenter => "BC",
        Anchor::BottomRight => "BR",
    }
}

/// Default Anchored configuration used when toggling out of FullScreen.
/// Mirrors the catalog modal's default size/anchor so new slots feel
/// consistent no matter how the user reached the Anchored branch.
const DEFAULT_ANCHORED: OverlayPosition = OverlayPosition::Anchored {
    anchor: Anchor::Center,
    offset_x: 0,
    offset_y: 0,
    width: 200,
    height: 60,
};

fn position_fields<F, D>(
    position: OverlayPosition,
    slot_state: StoredValue<OverlaySlot>,
    patch: F,
    patch_debounced: D,
) -> impl IntoView
where
    F: Fn(api::UpdateOverlaySlotRequest) + Clone + Send + Sync + 'static,
    D: Fn(api::UpdateOverlaySlotRequest) + Clone + Send + Sync + 'static,
{
    let is_full_screen = matches!(position, OverlayPosition::FullScreen);

    let patch_full_screen = patch.clone();
    let on_toggle_full_screen = move |_| {
        let current = slot_state.with_value(|slot| slot.position.clone());
        let next = match current {
            OverlayPosition::FullScreen => DEFAULT_ANCHORED,
            OverlayPosition::Anchored { .. } => OverlayPosition::FullScreen,
        };
        let updated = mutate_position(slot_state, |value| *value = next);
        patch_full_screen(api::UpdateOverlaySlotRequest {
            position: Some(updated),
            ..Default::default()
        });
    };

    // Pull the current anchored fields. When in FullScreen mode we still
    // want the inputs disabled but the labels to show the defaults that
    // would apply on toggle-back.
    let (current_anchor, offset_x_value, offset_y_value, width_value, height_value) =
        match position.clone() {
            OverlayPosition::FullScreen => (
                Anchor::Center,
                0_i32.to_string(),
                0_i32.to_string(),
                200_u32.to_string(),
                60_u32.to_string(),
            ),
            OverlayPosition::Anchored {
                anchor,
                offset_x,
                offset_y,
                width,
                height,
            } => (
                anchor,
                offset_x.to_string(),
                offset_y.to_string(),
                width.to_string(),
                height.to_string(),
            ),
        };

    let anchor_rows = ANCHOR_GRID
        .into_iter()
        .enumerate()
        .map(|(row_idx, row)| {
            let cells = row
                .into_iter()
                .enumerate()
                .map(|(col_idx, anchor)| {
                    let patch_anchor = patch.clone();
                    let label = anchor_short_label(anchor);
                    let selected = anchor == current_anchor;
                    let on_click = move |_| {
                        let updated = mutate_position(slot_state, |value| match value {
                            OverlayPosition::Anchored {
                                anchor: current, ..
                            } => {
                                *current = anchor;
                            }
                            other @ OverlayPosition::FullScreen => {
                                *other = OverlayPosition::Anchored {
                                    anchor,
                                    offset_x: 0,
                                    offset_y: 0,
                                    width: 200,
                                    height: 60,
                                };
                            }
                        });
                        patch_anchor(api::UpdateOverlaySlotRequest {
                            position: Some(updated),
                            ..Default::default()
                        });
                    };
                    let cell_key = row_idx * 3 + col_idx;
                    let class_name = if selected {
                        "flex aspect-square items-center justify-center rounded-sm border border-accent-primary bg-accent-primary/20 text-[10px] font-semibold text-accent-primary"
                    } else {
                        "flex aspect-square items-center justify-center rounded-sm border border-edge-subtle bg-surface-overlay/60 text-[10px] text-fg-tertiary transition hover:border-accent-primary/60 hover:text-fg-secondary"
                    };
                    view! {
                        <button
                            type="button"
                            class=class_name
                            data-cell=cell_key
                            disabled=is_full_screen
                            on:click=on_click
                        >
                            {label}
                        </button>
                    }
                })
                .collect_view();
            view! { <>{cells}</> }
        })
        .collect_view();

    let patch_offset_x = patch_debounced.clone();
    let on_offset_x = move |event: leptos::ev::Event| {
        let Ok(value) = event_target_value(&event).parse::<i32>() else {
            return;
        };
        let updated = mutate_position(slot_state, |position| {
            if let OverlayPosition::Anchored { offset_x, .. } = position {
                *offset_x = value;
            }
        });
        patch_offset_x(api::UpdateOverlaySlotRequest {
            position: Some(updated),
            ..Default::default()
        });
    };

    let patch_offset_y = patch_debounced.clone();
    let on_offset_y = move |event: leptos::ev::Event| {
        let Ok(value) = event_target_value(&event).parse::<i32>() else {
            return;
        };
        let updated = mutate_position(slot_state, |position| {
            if let OverlayPosition::Anchored { offset_y, .. } = position {
                *offset_y = value;
            }
        });
        patch_offset_y(api::UpdateOverlaySlotRequest {
            position: Some(updated),
            ..Default::default()
        });
    };

    let patch_width = patch_debounced.clone();
    let on_width = move |event: leptos::ev::Event| {
        let Ok(value) = event_target_value(&event).parse::<u32>() else {
            return;
        };
        let updated = mutate_position(slot_state, |position| {
            if let OverlayPosition::Anchored { width, .. } = position {
                *width = value.max(1);
            }
        });
        patch_width(api::UpdateOverlaySlotRequest {
            position: Some(updated),
            ..Default::default()
        });
    };

    let patch_height = patch_debounced.clone();
    let on_height = move |event: leptos::ev::Event| {
        let Ok(value) = event_target_value(&event).parse::<u32>() else {
            return;
        };
        let updated = mutate_position(slot_state, |position| {
            if let OverlayPosition::Anchored { height, .. } = position {
                *height = value.max(1);
            }
        });
        patch_height(api::UpdateOverlaySlotRequest {
            position: Some(updated),
            ..Default::default()
        });
    };

    view! {
        <div class="flex flex-col gap-2">
            <InspectorField label="Full screen">
                <button
                    type="button"
                    class=move || {
                        if is_full_screen {
                            "self-start rounded-sm bg-accent-primary/20 px-2 py-1 text-xs text-accent-primary transition hover:bg-accent-primary/30"
                        } else {
                            "self-start rounded-sm bg-surface-overlay/60 px-2 py-1 text-xs text-fg-tertiary transition hover:bg-surface-overlay"
                        }
                    }
                    on:click=on_toggle_full_screen
                >
                    {if is_full_screen { "On" } else { "Off" }}
                </button>
            </InspectorField>

            <InspectorField label="Anchor">
                <div class="grid grid-cols-3 gap-1">{anchor_rows}</div>
            </InspectorField>

            <div class="grid grid-cols-2 gap-2">
                <InspectorField label="Offset X">
                    <input
                        type="number"
                        class="w-full rounded-sm border border-edge-subtle bg-surface-overlay/60 px-2 py-1 text-xs text-fg-primary focus:border-accent-primary focus:outline-none disabled:opacity-50"
                        prop:value=offset_x_value
                        disabled=is_full_screen
                        on:input=on_offset_x
                    />
                </InspectorField>
                <InspectorField label="Offset Y">
                    <input
                        type="number"
                        class="w-full rounded-sm border border-edge-subtle bg-surface-overlay/60 px-2 py-1 text-xs text-fg-primary focus:border-accent-primary focus:outline-none disabled:opacity-50"
                        prop:value=offset_y_value
                        disabled=is_full_screen
                        on:input=on_offset_y
                    />
                </InspectorField>
                <InspectorField label="Width">
                    <input
                        type="number"
                        min="1"
                        class="w-full rounded-sm border border-edge-subtle bg-surface-overlay/60 px-2 py-1 text-xs text-fg-primary focus:border-accent-primary focus:outline-none disabled:opacity-50"
                        prop:value=width_value
                        disabled=is_full_screen
                        on:input=on_width
                    />
                </InspectorField>
                <InspectorField label="Height">
                    <input
                        type="number"
                        min="1"
                        class="w-full rounded-sm border border-edge-subtle bg-surface-overlay/60 px-2 py-1 text-xs text-fg-primary focus:border-accent-primary focus:outline-none disabled:opacity-50"
                        prop:value=height_value
                        disabled=is_full_screen
                        on:input=on_height
                    />
                </InspectorField>
            </div>
        </div>
    }
}

/// Apply a mutation to the live `ClockConfig` stored in `slot_state` and
/// return the fully-populated updated `OverlaySource`. Returns `None` if
/// the slot's source has been swapped to a different variant (which only
/// happens if the slot was edited externally), in which case the caller
/// skips the PATCH.
fn mutate_clock_source(
    slot_state: StoredValue<OverlaySlot>,
    mutation: impl FnOnce(&mut ClockConfig),
) -> Option<OverlaySource> {
    let mut applied = false;
    slot_state.update_value(|slot| {
        if let OverlaySource::Clock(config) = &mut slot.source {
            mutation(config);
            applied = true;
        }
    });
    applied.then(|| slot_state.with_value(|slot| slot.source.clone()))
}

/// Companion helper for `SensorOverlayConfig`. See `mutate_clock_source`.
fn mutate_sensor_source(
    slot_state: StoredValue<OverlaySlot>,
    mutation: impl FnOnce(&mut SensorOverlayConfig),
) -> Option<OverlaySource> {
    let mut applied = false;
    slot_state.update_value(|slot| {
        if let OverlaySource::Sensor(config) = &mut slot.source {
            mutation(config);
            applied = true;
        }
    });
    applied.then(|| slot_state.with_value(|slot| slot.source.clone()))
}

/// Companion helper for `ImageOverlayConfig`. See `mutate_clock_source`.
fn mutate_image_source(
    slot_state: StoredValue<OverlaySlot>,
    mutation: impl FnOnce(&mut ImageOverlayConfig),
) -> Option<OverlaySource> {
    let mut applied = false;
    slot_state.update_value(|slot| {
        if let OverlaySource::Image(config) = &mut slot.source {
            mutation(config);
            applied = true;
        }
    });
    applied.then(|| slot_state.with_value(|slot| slot.source.clone()))
}

/// Companion helper for `TextOverlayConfig`. See `mutate_clock_source`.
fn mutate_text_source(
    slot_state: StoredValue<OverlaySlot>,
    mutation: impl FnOnce(&mut TextOverlayConfig),
) -> Option<OverlaySource> {
    let mut applied = false;
    slot_state.update_value(|slot| {
        if let OverlaySource::Text(config) = &mut slot.source {
            mutation(config);
            applied = true;
        }
    });
    applied.then(|| slot_state.with_value(|slot| slot.source.clone()))
}

fn clock_inspector_fields<F>(
    config: ClockConfig,
    slot_state: StoredValue<OverlaySlot>,
    patch: F,
) -> impl IntoView
where
    F: Fn(api::UpdateOverlaySlotRequest) + Clone + Send + Sync + 'static,
{
    // Each handler mutates slot_state before building a PATCH body so the
    // request always reflects every prior edit rather than the render-time
    // snapshot.
    let patch_style = patch.clone();
    let on_style = move |event: leptos::ev::Event| {
        let value = event_target_value(&event);
        let style = if value == "analog" {
            ClockStyle::Analog
        } else {
            ClockStyle::Digital
        };
        if let Some(source) = mutate_clock_source(slot_state, |config| config.style = style) {
            patch_style(api::UpdateOverlaySlotRequest {
                source: Some(source),
                ..Default::default()
            });
        }
    };

    let patch_format = patch.clone();
    let on_format = move |event: leptos::ev::Event| {
        let value = event_target_value(&event);
        let format = if value == "12" {
            HourFormat::Twelve
        } else {
            HourFormat::TwentyFour
        };
        if let Some(source) = mutate_clock_source(slot_state, |config| config.hour_format = format)
        {
            patch_format(api::UpdateOverlaySlotRequest {
                source: Some(source),
                ..Default::default()
            });
        }
    };

    let patch_seconds = patch.clone();
    let on_seconds = move |_| {
        if let Some(source) = mutate_clock_source(slot_state, |config| {
            config.show_seconds = !config.show_seconds
        }) {
            patch_seconds(api::UpdateOverlaySlotRequest {
                source: Some(source),
                ..Default::default()
            });
        }
    };

    let patch_color = patch.clone();
    let on_color = move |event: leptos::ev::Event| {
        let value = event_target_value(&event);
        if let Some(source) = mutate_clock_source(slot_state, move |config| {
            config.color = value.clone();
        }) {
            patch_color(api::UpdateOverlaySlotRequest {
                source: Some(source),
                ..Default::default()
            });
        }
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
    slot_state: StoredValue<OverlaySlot>,
    patch: F,
    patch_debounced: D,
) -> impl IntoView
where
    F: Fn(api::UpdateOverlaySlotRequest) + Clone + Send + Sync + 'static,
    D: Fn(api::UpdateOverlaySlotRequest) + Clone + Send + Sync + 'static,
{
    // Live sensor snapshot polled every 2s so the user can see which
    // labels are available on the host and what each reports right now.
    // Clicking a row sets the slot's sensor label to that reading.
    let (sensor_snapshot, set_sensor_snapshot) =
        signal(None::<hypercolor_types::sensor::SystemSnapshot>);
    let fetch_sensors = move || {
        let set_sensor_snapshot = set_sensor_snapshot;
        spawn_local(async move {
            if let Ok(snapshot) = api::fetch_system_sensors().await {
                set_sensor_snapshot.set(Some(snapshot));
            }
        });
    };
    fetch_sensors();
    let fetch_sensors_interval = fetch_sensors.clone();
    let _sensor_interval = use_interval_fn(move || fetch_sensors_interval(), 2000_u64);

    // Unify the text input and the pick buttons through a single reactive
    // signal so a click on a picked sensor cannot race a blur-triggered
    // `on:change` from the text input — both paths write the signal and a
    // single debounced PATCH reconciles the latest value with the daemon.
    let (sensor_label_signal, set_sensor_label_signal) = signal(config.sensor.clone());
    Effect::new({
        let patch_debounced = patch_debounced.clone();
        move |previous: Option<String>| {
            let current = sensor_label_signal.get();
            // Skip the initial render so we don't PATCH the slot with its
            // own seed value.
            if previous.is_none() || previous.as_ref() == Some(&current) {
                return current;
            }
            if let Some(source) = mutate_sensor_source(slot_state, |config| {
                config.sensor = current.clone();
            }) {
                patch_debounced(api::UpdateOverlaySlotRequest {
                    source: Some(source),
                    ..Default::default()
                });
            }
            current
        }
    });
    let on_sensor = move |event: leptos::ev::Event| {
        set_sensor_label_signal.set(event_target_value(&event));
    };
    let on_pick_sensor = move |label: String| {
        set_sensor_label_signal.set(label);
    };

    let patch_style = patch.clone();
    let on_style = move |event: leptos::ev::Event| {
        let style = match event_target_value(&event).as_str() {
            "gauge" => SensorDisplayStyle::Gauge,
            "bar" => SensorDisplayStyle::Bar,
            "minimal" => SensorDisplayStyle::Minimal,
            _ => SensorDisplayStyle::Numeric,
        };
        if let Some(source) = mutate_sensor_source(slot_state, |config| config.style = style) {
            patch_style(api::UpdateOverlaySlotRequest {
                source: Some(source),
                ..Default::default()
            });
        }
    };

    let patch_min = patch_debounced.clone();
    let on_min = move |event: leptos::ev::Event| {
        let Ok(value) = event_target_value(&event).parse::<f32>() else {
            return;
        };
        if let Some(source) =
            mutate_sensor_source(slot_state, move |config| config.range_min = value)
        {
            patch_min(api::UpdateOverlaySlotRequest {
                source: Some(source),
                ..Default::default()
            });
        }
    };

    let patch_max = patch_debounced.clone();
    let on_max = move |event: leptos::ev::Event| {
        let Ok(value) = event_target_value(&event).parse::<f32>() else {
            return;
        };
        if let Some(source) =
            mutate_sensor_source(slot_state, move |config| config.range_max = value)
        {
            patch_max(api::UpdateOverlaySlotRequest {
                source: Some(source),
                ..Default::default()
            });
        }
    };

    let patch_unit = patch.clone();
    let on_unit = move |event: leptos::ev::Event| {
        let raw = event_target_value(&event);
        let trimmed = raw.trim().to_string();
        if let Some(source) = mutate_sensor_source(slot_state, move |config| {
            config.unit_label = if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            };
        }) {
            patch_unit(api::UpdateOverlaySlotRequest {
                source: Some(source),
                ..Default::default()
            });
        }
    };

    let unit = config.unit_label.clone().unwrap_or_default();
    let range_min = config.range_min.to_string();
    let range_max = config.range_max.to_string();
    let style_value = match config.style {
        SensorDisplayStyle::Numeric => "numeric",
        SensorDisplayStyle::Gauge => "gauge",
        SensorDisplayStyle::Bar => "bar",
        SensorDisplayStyle::Minimal => "minimal",
    };

    let sensor_browser = move || {
        let snapshot = sensor_snapshot.get()?;
        let readings = snapshot.readings();
        if readings.is_empty() {
            return None;
        }
        // Highlight the active label from the signal so clicks feel
        // responsive and stay in lockstep with the input field. Using
        // slot_state here would make the row only re-highlight after
        // the PATCH refetch lands.
        let selected = sensor_label_signal.get();
        let rows = readings
            .into_iter()
            .map(|reading| {
                let is_selected = reading.label == selected;
                let label = reading.label.clone();
                let label_for_click = label.clone();
                let formatted_value = format_sensor_value(reading.value, reading.unit);
                let pick = on_pick_sensor.clone();
                let row_class = if is_selected {
                    "flex w-full items-center justify-between gap-2 rounded-sm border border-accent-primary/40 bg-accent-primary/10 px-2 py-1 text-[11px] text-fg-primary"
                } else {
                    "flex w-full items-center justify-between gap-2 rounded-sm border border-transparent px-2 py-1 text-[11px] text-fg-tertiary transition hover:border-edge-subtle hover:bg-surface-overlay/60 hover:text-fg-secondary"
                };
                view! {
                    <li>
                        <button
                            type="button"
                            class=row_class
                            on:click=move |_| pick(label_for_click.clone())
                        >
                            <span class="font-mono">{label}</span>
                            <span class="font-mono">{formatted_value}</span>
                        </button>
                    </li>
                }
            })
            .collect_view();
        Some(view! {
            <div class="rounded-sm border border-edge-subtle bg-surface-overlay/40 p-1">
                <div class="px-1 pb-1 text-[10px] uppercase tracking-wider text-fg-tertiary">
                    "Available sensors"
                </div>
                <ul class="flex max-h-48 flex-col gap-0.5 overflow-y-auto">{rows}</ul>
            </div>
        })
    };

    view! {
        <InspectorField label="Sensor label">
            <input
                type="text"
                class="w-full rounded-sm border border-edge-subtle bg-surface-overlay/60 px-2 py-1 text-xs text-fg-primary focus:border-accent-primary focus:outline-none"
                prop:value=move || sensor_label_signal.get()
                placeholder="cpu_temp"
                on:input=on_sensor
            />
        </InspectorField>
        {sensor_browser}
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

fn image_inspector_fields<F>(
    config: ImageOverlayConfig,
    slot_state: StoredValue<OverlaySlot>,
    patch: F,
) -> impl IntoView
where
    F: Fn(api::UpdateOverlaySlotRequest) + Clone + Send + Sync + 'static,
{
    let patch_path = patch.clone();
    let on_path = move |event: leptos::ev::Event| {
        let value = event_target_value(&event);
        if let Some(source) =
            mutate_image_source(slot_state, move |config| config.path = value.clone())
        {
            patch_path(api::UpdateOverlaySlotRequest {
                source: Some(source),
                ..Default::default()
            });
        }
    };

    let patch_fit = patch.clone();
    let on_fit = move |event: leptos::ev::Event| {
        let fit = match event_target_value(&event).as_str() {
            "cover" => ImageFit::Cover,
            "stretch" => ImageFit::Stretch,
            "original" => ImageFit::Original,
            _ => ImageFit::Contain,
        };
        if let Some(source) = mutate_image_source(slot_state, move |config| config.fit = fit) {
            patch_fit(api::UpdateOverlaySlotRequest {
                source: Some(source),
                ..Default::default()
            });
        }
    };

    let path = config.path.clone();
    let fit_value = match config.fit {
        ImageFit::Cover => "cover",
        ImageFit::Contain => "contain",
        ImageFit::Stretch => "stretch",
        ImageFit::Original => "original",
    };

    view! {
        <InspectorField label="Image path">
            <input
                type="text"
                class="w-full rounded-sm border border-edge-subtle bg-surface-overlay/60 px-2 py-1 text-xs text-fg-primary focus:border-accent-primary focus:outline-none"
                prop:value=path
                placeholder="/path/to/image.png"
                on:change=on_path
            />
        </InspectorField>
        <InspectorField label="Fit">
            <select
                class="w-full rounded-sm border border-edge-subtle bg-surface-overlay/60 px-2 py-1 text-xs text-fg-primary focus:border-accent-primary focus:outline-none"
                on:change=on_fit
            >
                <option value="contain" selected=fit_value == "contain">"Contain"</option>
                <option value="cover" selected=fit_value == "cover">"Cover"</option>
                <option value="stretch" selected=fit_value == "stretch">"Stretch"</option>
                <option value="original" selected=fit_value == "original">"Original"</option>
            </select>
        </InspectorField>
        <p class="text-[11px] leading-relaxed text-fg-tertiary">
            "Point at a PNG, JPEG, WebP, or GIF on the daemon host. Upload UX lands in a follow-up."
        </p>
    }
}

fn text_inspector_fields<F, D>(
    config: TextOverlayConfig,
    slot_state: StoredValue<OverlaySlot>,
    patch: F,
    patch_debounced: D,
) -> impl IntoView
where
    F: Fn(api::UpdateOverlaySlotRequest) + Clone + Send + Sync + 'static,
    D: Fn(api::UpdateOverlaySlotRequest) + Clone + Send + Sync + 'static,
{
    let patch_text = patch.clone();
    let on_text = move |event: leptos::ev::Event| {
        let value = event_target_value(&event);
        if let Some(source) =
            mutate_text_source(slot_state, move |config| config.text = value.clone())
        {
            patch_text(api::UpdateOverlaySlotRequest {
                source: Some(source),
                ..Default::default()
            });
        }
    };

    let patch_size = patch_debounced.clone();
    let on_size = move |event: leptos::ev::Event| {
        let Ok(value) = event_target_value(&event).parse::<f32>() else {
            return;
        };
        if let Some(source) = mutate_text_source(slot_state, move |config| {
            config.font_size = value.max(1.0);
        }) {
            patch_size(api::UpdateOverlaySlotRequest {
                source: Some(source),
                ..Default::default()
            });
        }
    };

    let patch_color = patch.clone();
    let on_color = move |event: leptos::ev::Event| {
        let value = event_target_value(&event);
        if let Some(source) =
            mutate_text_source(slot_state, move |config| config.color = value.clone())
        {
            patch_color(api::UpdateOverlaySlotRequest {
                source: Some(source),
                ..Default::default()
            });
        }
    };

    let patch_align = patch.clone();
    let on_align = move |event: leptos::ev::Event| {
        let align = match event_target_value(&event).as_str() {
            "left" => TextAlign::Left,
            "right" => TextAlign::Right,
            _ => TextAlign::Center,
        };
        if let Some(source) = mutate_text_source(slot_state, move |config| config.align = align) {
            patch_align(api::UpdateOverlaySlotRequest {
                source: Some(source),
                ..Default::default()
            });
        }
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

/// Compute the top-left origin of a slot rectangle given its anchor, the
/// display dimensions, and the slot size. Mirrors the daemon's
/// `anchor_origin` in display_output/overlay.rs.
fn anchor_origin_ui(
    anchor: Anchor,
    display_width: i32,
    display_height: i32,
    slot_width: i32,
    slot_height: i32,
) -> (i32, i32) {
    match anchor {
        Anchor::TopLeft => (0, 0),
        Anchor::TopCenter => ((display_width - slot_width) / 2, 0),
        Anchor::TopRight => (display_width - slot_width, 0),
        Anchor::CenterLeft => (0, (display_height - slot_height) / 2),
        Anchor::Center => (
            (display_width - slot_width) / 2,
            (display_height - slot_height) / 2,
        ),
        Anchor::CenterRight => (
            display_width - slot_width,
            (display_height - slot_height) / 2,
        ),
        Anchor::BottomLeft => (0, display_height - slot_height),
        Anchor::BottomCenter => (
            (display_width - slot_width) / 2,
            display_height - slot_height,
        ),
        Anchor::BottomRight => (display_width - slot_width, display_height - slot_height),
    }
}

/// Resolve a slot's position to CSS percentage coordinates on the preview.
fn resolve_slot_css_pct(
    position: &OverlayPosition,
    display_width: u32,
    display_height: u32,
) -> (f64, f64, f64, f64) {
    match position {
        OverlayPosition::FullScreen => (0.0, 0.0, 100.0, 100.0),
        OverlayPosition::Anchored {
            anchor,
            offset_x,
            offset_y,
            width,
            height,
        } => {
            #[expect(
                clippy::as_conversions,
                reason = "coordinate math on bounded display pixel values"
            )]
            let (dw, dh, sw, sh) = (
                display_width as i32,
                display_height as i32,
                *width as i32,
                *height as i32,
            );
            let (base_x, base_y) = anchor_origin_ui(*anchor, dw, dh, sw, sh);
            let x = base_x + offset_x;
            let y = base_y + offset_y;
            #[expect(clippy::as_conversions, reason = "percentage conversion from i32")]
            let left = (x as f64 / dw as f64) * 100.0;
            #[expect(clippy::as_conversions, reason = "percentage conversion from i32")]
            let top = (y as f64 / dh as f64) * 100.0;
            #[expect(clippy::as_conversions, reason = "percentage conversion from i32")]
            let w = (sw as f64 / dw as f64) * 100.0;
            #[expect(clippy::as_conversions, reason = "percentage conversion from i32")]
            let h = (sh as f64 / dh as f64) * 100.0;
            (left, top, w, h)
        }
    }
}

/// Render a draggable outline on the preview canvas.
fn render_slot_outline(
    slot: OverlaySlot,
    display_width: u32,
    display_height: u32,
    drag_state: ReadSignal<Option<DragState>>,
    set_drag_state: WriteSignal<Option<DragState>>,
    container_ref: NodeRef<leptos::html::Div>,
) -> impl IntoView {
    let slot_id = slot.id;
    let (_, source_icon) = overlay_source_descriptor(&slot.source);
    let name = slot.name.clone();

    let (slot_width, slot_height) = match &slot.position {
        OverlayPosition::FullScreen => (display_width, display_height),
        OverlayPosition::Anchored { width, height, .. } => (*width, *height),
    };

    // Static CSS percentages used when this slot is NOT being dragged.
    let (static_left, static_top, width_pct, height_pct) =
        resolve_slot_css_pct(&slot.position, display_width, display_height);

    // Resolved display-pixel origin for drag start reference.
    let (resolved_x, resolved_y) = match &slot.position {
        OverlayPosition::FullScreen => (0.0, 0.0),
        OverlayPosition::Anchored {
            anchor,
            offset_x,
            offset_y,
            width,
            height,
        } => {
            let (bx, by) = anchor_origin_ui(
                *anchor,
                display_width as i32,
                display_height as i32,
                *width as i32,
                *height as i32,
            );
            ((bx + offset_x) as f64, (by + offset_y) as f64)
        }
    };

    let is_full_screen = matches!(slot.position, OverlayPosition::FullScreen);

    let on_pointerdown = move |event: PointerEvent| {
        if is_full_screen {
            return;
        }
        event.prevent_default();
        let Some(container) = container_ref.get() else {
            return;
        };
        let rect = container.get_bounding_client_rect();
        let pointer_x = event.client_x() as f64 - rect.left();
        let pointer_y = event.client_y() as f64 - rect.top();
        set_drag_state.set(Some(DragState {
            slot_id,
            start_slot_x: resolved_x,
            start_slot_y: resolved_y,
            start_pointer_x: pointer_x,
            start_pointer_y: pointer_y,
            current_pointer_x: pointer_x,
            current_pointer_y: pointer_y,
            display_width,
            display_height,
            slot_width,
            slot_height,
        }));
    };

    let style = Signal::derive(move || {
        let is_dragging = drag_state.with(|d| d.as_ref().is_some_and(|d| d.slot_id == slot_id));
        if is_dragging {
            let container_width = container_ref
                .get()
                .map_or(1.0, |c| c.get_bounding_client_rect().width());
            let (dx, dy) = drag_state.with(|d| {
                d.as_ref()
                    .map(|d| d.current_display_position(container_width))
                    .unwrap_or((resolved_x, resolved_y))
            });
            let dw = f64::from(display_width);
            let dh = f64::from(display_height);
            let left = (dx / dw) * 100.0;
            let top = (dy / dh) * 100.0;
            format!(
                "position:absolute; left:{left:.3}%; top:{top:.3}%; width:{width_pct:.3}%; height:{height_pct:.3}%; z-index:20; cursor:grabbing;"
            )
        } else {
            let cursor = if is_full_screen { "default" } else { "grab" };
            format!(
                "position:absolute; left:{static_left:.3}%; top:{static_top:.3}%; width:{width_pct:.3}%; height:{height_pct:.3}%; cursor:{cursor};"
            )
        }
    });

    let is_dragging_this = Signal::derive(move || {
        drag_state.with(|d| d.as_ref().is_some_and(|d| d.slot_id == slot_id))
    });

    view! {
        <div
            class=move || {
                if is_dragging_this.get() {
                    "flex items-end justify-start border-2 border-accent-primary bg-accent-primary/15"
                } else {
                    "flex items-end justify-start border border-accent-primary/60 bg-accent-primary/8 transition-[left,top] duration-150"
                }
            }
            style=style
            on:pointerdown=on_pointerdown
        >
            <span
                class="inline-flex items-center gap-1 rounded-tr-sm bg-accent-primary/40 px-1 py-0.5 text-[9px] font-medium text-white/90 backdrop-blur-sm"
                style="line-height:1.1;"
            >
                <Icon icon=source_icon width="9" height="9" />
                {name}
            </span>
        </div>
    }
}

fn format_sensor_value(value: f32, unit: hypercolor_types::sensor::SensorUnit) -> String {
    let symbol = unit.symbol();
    // Temperatures and loads sit in the 0-200 range where 1 decimal is
    // enough. Megabyte counts round to whole MB. Integer-looking units
    // like RPM drop the decimals entirely.
    match unit {
        hypercolor_types::sensor::SensorUnit::Celsius
        | hypercolor_types::sensor::SensorUnit::Percent => {
            format!("{value:.1}{symbol}")
        }
        hypercolor_types::sensor::SensorUnit::Megabytes => {
            format!("{} {symbol}", value.round() as i64)
        }
        hypercolor_types::sensor::SensorUnit::Rpm
        | hypercolor_types::sensor::SensorUnit::Mhz
        | hypercolor_types::sensor::SensorUnit::Watts => {
            format!("{} {symbol}", value.round() as i64)
        }
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
