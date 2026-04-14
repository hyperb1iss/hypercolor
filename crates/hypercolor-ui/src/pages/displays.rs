//! `/displays` — LCD-equipped devices and full-screen HTML faces.
//!
//! Three-region workspace (picker, cinematic preview, resizable control
//! column) for assigning full-screen faces to LCD devices and tuning
//! face controls.

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_icons::Icon;
use leptos_use::{use_debounce_fn, use_debounce_fn_with_arg};

use crate::api;
use crate::app::EffectsContext;
use crate::components::control_panel::ControlPanel;
use crate::components::page_header::PageHeader;
use crate::components::resize_handle::ResizeHandle;
use crate::icons::*;
use crate::toasts;
use hypercolor_types::scene::{DisplayFaceBlendMode, SceneKind, SceneMutationMode};

type DisplaysResource = LocalResource<Result<Vec<api::DisplaySummary>, String>>;

/// Resizable right-side control column — bounds and localStorage key.
const MIN_RIGHT_WIDTH: f64 = 280.0;
const MAX_RIGHT_WIDTH: f64 = 600.0;
const DEFAULT_RIGHT_WIDTH: f64 = 360.0;
const LS_KEY_RIGHT_WIDTH: &str = "hc-disp-right-width";

#[derive(Clone, Copy, PartialEq, Eq)]
struct FaceBlendOption {
    mode: DisplayFaceBlendMode,
    label: &'static str,
    blurb: &'static str,
}

const FACE_BLEND_OPTIONS: [FaceBlendOption; 9] = [
    FaceBlendOption {
        mode: DisplayFaceBlendMode::Replace,
        label: "Replace",
        blurb: "Render the face directly with no effect influence.",
    },
    FaceBlendOption {
        mode: DisplayFaceBlendMode::Alpha,
        label: "Alpha",
        blurb: "Use face transparency as a clean reveal into the live effect layer.",
    },
    FaceBlendOption {
        mode: DisplayFaceBlendMode::Screen,
        label: "Screen",
        blurb: "Fuse face highlights with the effect for luminous neon glass.",
    },
    FaceBlendOption {
        mode: DisplayFaceBlendMode::Add,
        label: "Add",
        blurb: "Push both layers together for hotter, flashier glow.",
    },
    FaceBlendOption {
        mode: DisplayFaceBlendMode::Multiply,
        label: "Multiply",
        blurb: "Turn the face into tinted glass that darkens and colors the effect.",
    },
    FaceBlendOption {
        mode: DisplayFaceBlendMode::Overlay,
        label: "Overlay",
        blurb: "Blend contrast-rich UI material that pops without flattening the effect.",
    },
    FaceBlendOption {
        mode: DisplayFaceBlendMode::SoftLight,
        label: "Soft Light",
        blurb: "Keep the effect alive under a softer satin face treatment.",
    },
    FaceBlendOption {
        mode: DisplayFaceBlendMode::ColorDodge,
        label: "Color Dodge",
        blurb: "Turn bright face areas into intense reactive highlights.",
    },
    FaceBlendOption {
        mode: DisplayFaceBlendMode::Difference,
        label: "Difference",
        blurb: "Create reactive inversions for wilder holographic looks.",
    },
];

fn face_blend_option(mode: DisplayFaceBlendMode) -> FaceBlendOption {
    FACE_BLEND_OPTIONS
        .iter()
        .copied()
        .find(|option| option.mode == mode)
        .unwrap_or(FACE_BLEND_OPTIONS[0])
}

fn sync_face_composition_from_server(
    display_face: ReadSignal<Option<Result<Option<api::DisplayFaceResponse>, String>>>,
    set_local_blend_mode: WriteSignal<DisplayFaceBlendMode>,
    set_local_opacity: WriteSignal<f32>,
) {
    let target = display_face
        .get_untracked()
        .and_then(Result::ok)
        .flatten()
        .and_then(|face| face.group.display_target);
    if let Some(target) = target {
        set_local_blend_mode.set(target.blend_mode);
        set_local_opacity.set(target.opacity.clamp(0.0, 1.0));
    } else {
        set_local_blend_mode.set(DisplayFaceBlendMode::Replace);
        set_local_opacity.set(1.0);
    }
}

#[component]
pub fn DisplaysPage() -> impl IntoView {
    let fx = expect_context::<EffectsContext>();
    let displays: DisplaysResource = LocalResource::new(api::fetch_displays);
    let (selected_id, set_selected_id) = signal(None::<String>);
    let (simulator_modal_open, set_simulator_modal_open) = signal(false);
    let (editing_simulator, set_editing_simulator) = signal(None::<api::DisplaySummary>);

    // Face state — hoisted to the page level so the right-column face card
    // and the center preview (which shows the assigned face name) stay in
    // sync without cross-component prop drilling.
    let (display_face, set_display_face) =
        signal(None::<Result<Option<api::DisplayFaceResponse>, String>>);
    let (face_catalog, set_face_catalog) = signal(None::<Result<Vec<api::EffectSummary>, String>>);
    let (face_picker_open, set_face_picker_open) = signal(false);
    let (face_assignment_pending, set_face_assignment_pending) = signal(false);
    // Monotonic counter used when external scene changes require an explicit
    // face reload for the selected display.
    let (face_refresh_tick, set_face_refresh_tick) = signal(0_u64);
    // Panel sizing — persisted to localStorage so the column width
    // survives reloads.
    let (right_width, set_right_width) = signal(crate::storage::get_clamped(
        LS_KEY_RIGHT_WIDTH,
        DEFAULT_RIGHT_WIDTH,
        MIN_RIGHT_WIDTH,
        MAX_RIGHT_WIDTH,
    ));

    let (on_right_drag_start, on_right_drag, on_right_drag_end) = drag_callbacks(
        right_width,
        set_right_width,
        MIN_RIGHT_WIDTH,
        MAX_RIGHT_WIDTH,
        LS_KEY_RIGHT_WIDTH,
    );

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

    // Refetch the assigned face whenever the selected display changes or an
    // external scene mutation lands. Last-good is preserved while the request
    // is in flight so the face card doesn't flash empty state during swaps.
    Effect::new(move |_| {
        let Some(display) = selected_display.get() else {
            set_display_face.set(None);
            set_face_picker_open.set(false);
            set_face_assignment_pending.set(false);
            return;
        };
        let _tick = face_refresh_tick.get();
        let display_id = display.id.clone();
        let requested_id = display_id.clone();
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

    // Lazy-load the face catalog the first time the picker opens.
    Effect::new(move |_| {
        if !face_picker_open.get() || face_catalog.with(Option::is_some) {
            return;
        }
        spawn_local(async move {
            let result = api::fetch_effects_by_category("display").await;
            set_face_catalog.set(Some(result));
        });
    });

    let current_face_id = Signal::derive(move || {
        display_face
            .get()
            .and_then(Result::ok)
            .flatten()
            .map(|face| face.effect.id)
    });

    let assign_face = Callback::new(move |effect_id: String| {
        if let Some(message) =
            snapshot_scene_lock_message(Some(fx), "assigning or changing display faces")
        {
            toasts::toast_error(&message);
            return;
        }
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
        if let Some(message) = snapshot_scene_lock_message(Some(fx), "clearing display faces") {
            toasts::toast_error(&message);
            return;
        }
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
                    toasts::toast_success(&format!("Cleared face from {display_name}"));
                }
                Err(error) => {
                    set_face_assignment_pending.set(false);
                    toasts::toast_error(&format!("Could not clear display face: {error}"));
                }
            }
        });
    });
    let open_face_picker = Callback::new(move |_| {
        if let Some(message) =
            snapshot_scene_lock_message(Some(fx), "assigning or changing display faces")
        {
            toasts::toast_error(&message);
            return;
        }
        set_face_picker_open.set(true);
    });
    let close_face_picker = Callback::new(move |_| set_face_picker_open.set(false));

    let open_simulator_modal = Callback::new(move |_| set_simulator_modal_open.set(true));
    let close_simulator_modal = Callback::new(move |_| set_simulator_modal_open.set(false));
    let open_simulator_editor =
        Callback::new(move |display: api::DisplaySummary| set_editing_simulator.set(Some(display)));
    let close_simulator_editor = Callback::new(move |_| set_editing_simulator.set(None));
    let named_scene_warning = Memo::new(move |_| {
        (fx.active_scene_kind.get() == Some(SceneKind::Named)).then(|| {
            (
                fx.active_scene_name
                    .get()
                    .unwrap_or_else(|| "Active scene".to_owned()),
                fx.active_scene_mutation_mode.get() == Some(SceneMutationMode::Snapshot),
            )
        })
    });
    let (returning_to_default, set_returning_to_default) = signal(false);
    let on_return_to_default = {
        let fx = fx;
        Callback::new(move |_| {
            if returning_to_default.get_untracked() {
                return;
            }
            set_returning_to_default.set(true);
            spawn_local(async move {
                if api::deactivate_scene().await.is_ok() {
                    fx.refresh_active_scene();
                    set_face_picker_open.set(false);
                    set_face_refresh_tick.update(|value| *value = value.wrapping_add(1));
                    toasts::toast_success("Returned to Default scene.");
                } else {
                    toasts::toast_error("Couldn't return to Default scene.");
                }
                set_returning_to_default.set(false);
            });
        })
    };
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
        <div class="flex h-full min-h-0 flex-col overflow-hidden animate-fade-in">
            <div class="shrink-0 glass-subtle border-b border-edge-default">
                <div class="px-6 pt-5 pb-4">
                    <PageHeader
                        icon=LuMonitor
                        title="Displays"
                        subtitle="Assign HTML faces to LCD devices and tune them live."
                        accent_rgb="225, 53, 255"
                        gradient="linear-gradient(105deg,#e135ff 0%,#e8f4ff 55%,#80ffea 100%)"
                    />
                </div>
                {move || named_scene_warning.get().map(|(scene_name, snapshot_locked)| view! {
                    <div class="px-6 pb-4">
                        <div class="rounded-xl border border-[rgba(241,250,140,0.24)] bg-[rgba(241,250,140,0.08)] px-4 py-3 shadow-[0_0_24px_rgba(241,250,140,0.08)]">
                            <div class="flex items-start gap-3">
                                <div class="mt-0.5 shrink-0 text-[rgba(241,250,140,0.9)]">
                                    <Icon icon=LuTriangleAlert width="14px" height="14px" />
                                </div>
                                <div class="min-w-0 flex-1">
                                    <div class="text-[11px] font-semibold uppercase tracking-[0.16em] text-[rgba(241,250,140,0.82)]">
                                        {if snapshot_locked { "Snapshot Scene Locked" } else { "Named Scene Active" }}
                                    </div>
                                    <div class="mt-1 text-sm leading-5 text-fg-secondary">
                                        <span class="text-fg-primary">{scene_name.clone()}</span>
                                        {if snapshot_locked {
                                            " is snapshot-locked. Return to Default before assigning, clearing, or tuning a display face."
                                        } else {
                                            " is active. Assigning, clearing, or tuning a face here rewrites that scene’s display group."
                                        }}
                                    </div>
                                </div>
                                <button
                                    class="shrink-0 rounded-lg border border-[rgba(241,250,140,0.28)] px-3 py-1.5 text-[11px] font-medium text-[rgba(241,250,140,0.92)] transition-all duration-200 hover:bg-[rgba(241,250,140,0.08)] disabled:cursor-wait disabled:opacity-60"
                                    disabled=move || returning_to_default.get()
                                    on:click=move |_| on_return_to_default.run(())
                                >
                                    {move || if returning_to_default.get() {
                                        "Returning..."
                                    } else {
                                        "Return to Default"
                                    }}
                                </button>
                            </div>
                        </div>
                    </div>
                })}
            </div>
            <div class="relative flex min-h-0 flex-1 gap-3 p-3">
                <aside class="flex min-h-0 w-[260px] shrink-0 flex-col">
                    <DisplayPicker
                        displays=displays
                        selected_id=selected_id
                        set_selected_id=set_selected_id
                        on_create_simulator=open_simulator_modal
                        on_manage_simulator=open_simulator_editor
                    />
                </aside>
                <div class="flex min-h-0 min-w-0 flex-1 flex-col">
                    <DisplayWorkspace
                        selected_display=selected_display
                        display_face=display_face
                    />
                </div>
                <Show when=move || selected_display.with(Option::is_some) fallback=|| ()>
                    <ResizeHandle
                        on_drag_start=on_right_drag_start
                        on_drag=on_right_drag
                        on_drag_end=on_right_drag_end
                    />
                    <aside
                        class="flex min-h-0 shrink-0 flex-col"
                        style=move || format!("width: {}px", right_width.get())
                    >
                        <DisplayRightPanel
                            selected_display=selected_display
                            display_face=display_face
                            set_display_face=set_display_face
                            face_assignment_pending=face_assignment_pending
                            on_choose_face=open_face_picker
                            on_clear_face=clear_face
                        />
                    </aside>
                </Show>
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
                                    on_clear=clear_face
                                    on_close=close_face_picker
                                />
                            }
                        })
                    }}
                </Show>
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

/// Shared drag callback builder for the right-panel resize handle. Mirrors
/// the effects page pattern so both pages feel identical.
fn drag_callbacks(
    width: ReadSignal<f64>,
    set_width: WriteSignal<f64>,
    min: f64,
    max: f64,
    storage_key: &'static str,
) -> (Callback<()>, Callback<f64>, Callback<()>) {
    let drag_start = StoredValue::new(0.0_f64);

    let on_start = Callback::new(move |()| {
        drag_start.set_value(width.get_untracked());
        toggle_body_resizing(true);
    });
    let on_drag = Callback::new(move |delta_x: f64| {
        // Right-side panel: dragging right shrinks the panel.
        let new_w = (drag_start.get_value() - delta_x).clamp(min, max);
        set_width.set(new_w);
    });
    let on_end = Callback::new(move |()| {
        toggle_body_resizing(false);
        crate::storage::set(storage_key, &width.get_untracked().to_string());
    });

    (on_start, on_drag, on_end)
}

#[component]
fn DisplaysModalBackdrop(
    #[prop(optional)] wide: bool,
    #[prop(into)] on_close: Callback<()>,
    children: Children,
) -> impl IntoView {
    let close_backdrop = on_close.clone();

    view! {
        <div class="fixed inset-0 z-50 grid place-items-center p-4 animate-fade-in">
            <div
                class="absolute inset-0 bg-black/65 backdrop-blur-sm"
                on:click=move |_| close_backdrop.run(())
            />
            <div
                class="relative"
                style=move || {
                    let width_style = if wide {
                        "width: min(64rem, calc(100vw - 2rem));"
                    } else {
                        "width: min(28rem, calc(100vw - 2rem));"
                    };
                    format!(
                        "position: relative; max-height: calc(100vh - 2rem); {width_style}"
                    )
                }
            >
                {children()}
            </div>
        </div>
    }
}

fn snapshot_scene_lock_message(ctx: Option<EffectsContext>, action: &str) -> Option<String> {
    let ctx = ctx?;
    if ctx.active_scene_kind.get_untracked() != Some(SceneKind::Named)
        || ctx.active_scene_mutation_mode.get_untracked() != Some(SceneMutationMode::Snapshot)
    {
        return None;
    }

    let scene_name = ctx
        .active_scene_name
        .get_untracked()
        .unwrap_or_else(|| "Active scene".to_owned());
    Some(format!(
        "{scene_name} is snapshot-locked. Return to Default before {action}."
    ))
}

fn toggle_body_resizing(active: bool) {
    if let Some(body) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.body())
    {
        if active {
            let _ = body.class_list().add_1("resizing");
        } else {
            let _ = body.class_list().remove_1("resizing");
        }
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
        // Display picker reads as neutral chrome — the sidebar is a
        // navigation list, not the primary accented region. Just the
        // subtle edge-glow is enough.
        <div class="flex min-h-0 flex-1 flex-col overflow-hidden rounded-xl border border-edge-subtle bg-surface-raised/80 edge-glow">
            <header class="flex items-center justify-between border-b border-edge-subtle/50 px-3 py-2">
                <div class="flex items-center gap-2 text-[11px] uppercase tracking-wider text-fg-secondary">
                    <div class="flex h-5 w-5 items-center justify-center rounded-md bg-surface-overlay text-fg-secondary">
                        <Icon icon=LuMonitor width="11" height="11" />
                    </div>
                    <span class="font-semibold">"Displays"</span>
                </div>
                <button
                    type="button"
                    class="rounded-sm p-1 text-fg-tertiary transition hover:text-accent-primary"
                    title="Refresh displays"
                    on:click=move |_| displays.refetch()
                >
                    <Icon icon=LuRefreshCw width="12" height="12" />
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
        </div>
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

use crate::display_utils::{
    display_preview_shell_url, is_simulator_display, json_to_face_control_value,
    parse_simulator_dimension,
};

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

    let close_button = on_close.clone();

    view! {
        <DisplaysModalBackdrop on_close=on_close>
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
        </DisplaysModalBackdrop>
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

    let close_button = on_close.clone();

    view! {
        <DisplaysModalBackdrop on_close=on_close>
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
        </DisplaysModalBackdrop>
    }
}

#[component]
fn DisplayWorkspace(
    selected_display: Memo<Option<api::DisplaySummary>>,
    display_face: ReadSignal<Option<Result<Option<api::DisplayFaceResponse>, String>>>,
) -> impl IntoView {
    let ws = use_context::<crate::app::WsContext>();

    // Subscribe the `display_preview` WS channel to whichever display is
    // currently selected. The WsManager handles actual subscribe/unsubscribe
    // messages and clears `display_preview_frame` on deselect.
    Effect::new(move |_| {
        let Some(ctx) = ws else { return };
        let device_id = selected_display.with(|d| d.as_ref().map(|s| s.id.clone()));
        ctx.set_display_preview_device.set(device_id);
    });
    on_cleanup(move || {
        if let Some(ctx) = ws {
            ctx.set_display_preview_device.set(None);
        }
    });

    // Blob URL lifecycle for the WS display preview frame. Every incoming
    // JPEG frame gets a fresh object URL; the previous URL is revoked on
    // the next tick so we don't leak blob memory. When no frame is
    // available, the signal is None and the <img> falls back to the
    // REST preview endpoint (cached JPEG, useful while the WS warms up
    // or when the daemon isn't yet pushing frames).
    let (preview_blob_url, set_preview_blob_url) = signal(None::<String>);
    Effect::new(move |previous: Option<Option<String>>| {
        if let Some(Some(old_url)) = previous.as_ref() {
            let _ = web_sys::Url::revoke_object_url(old_url);
        }
        let Some(ctx) = ws else { return None };
        let Some(frame) = ctx.display_preview_frame.get() else {
            set_preview_blob_url.set(None);
            return None;
        };
        if !matches!(
            frame.pixel_format(),
            crate::ws::messages::CanvasPixelFormat::Jpeg
        ) {
            set_preview_blob_url.set(None);
            return None;
        }

        let parts = js_sys::Array::new();
        parts.push(frame.pixels_js());
        let options = web_sys::BlobPropertyBag::new();
        options.set_type("image/jpeg");
        let blob = match web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &options) {
            Ok(blob) => blob,
            Err(_) => {
                set_preview_blob_url.set(None);
                return None;
            }
        };
        let url = match web_sys::Url::create_object_url_with_blob(&blob) {
            Ok(url) => url,
            Err(_) => {
                set_preview_blob_url.set(None);
                return None;
            }
        };
        set_preview_blob_url.set(Some(url.clone()));
        Some(url)
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
                    // Prefer the live WS blob URL when a frame has arrived;
                    // fall back to the REST preview endpoint while the WS
                    // channel is warming up or disconnected so the user
                    // never sees a black rectangle mid-connection.
                    let src = preview_blob_url
                        .get()
                        .unwrap_or_else(|| api::display_preview_url(&display.id, None));
                    let aspect = format!("{} / {}", display.width, display.height);
                    let rounded_class = if display.circular {
                        "rounded-full"
                    } else {
                        "rounded-lg"
                    };
                    let alt_text = format!("Live preview of {}", display.name);
                    let container_class = format!(
                        "max-h-full max-w-full overflow-hidden border border-edge-default bg-black shadow-2xl {rounded_class}"
                    );

                    view! {
                        <div
                            class=container_class
                            style=move || format!("aspect-ratio: {aspect};")
                        >
                            <img
                                class="h-full w-full object-cover"
                                src=src
                                alt=alt_text
                                loading="eager"
                                decoding="async"
                                draggable="false"
                            />
                        </div>
                    }.into_any()
                }}
            </div>
        </section>
    }
}

/// Right-side control column.
///
/// Stacks three sections vertically: a compact face-assignment card (always
/// visible when a display is selected) and a live `ControlPanel` bound to
/// the assigned face's controls. The whole column is wrapped in a scrollable
/// container so long control panels can exceed the viewport.
#[component]
fn DisplayRightPanel(
    selected_display: Memo<Option<api::DisplaySummary>>,
    display_face: ReadSignal<Option<Result<Option<api::DisplayFaceResponse>, String>>>,
    set_display_face: WriteSignal<Option<Result<Option<api::DisplayFaceResponse>, String>>>,
    face_assignment_pending: ReadSignal<bool>,
    on_choose_face: Callback<()>,
    on_clear_face: Callback<()>,
) -> impl IntoView {
    view! {
        <div class="flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto pr-0.5" style="overscroll-behavior: contain;">
            <FaceAssignmentCard
                selected_display=selected_display
                display_face=display_face
                face_assignment_pending=face_assignment_pending
                on_choose_face=on_choose_face
                on_clear_face=on_clear_face
            />
            <FaceCompositionSection
                selected_display=selected_display
                display_face=display_face
                set_display_face=set_display_face
            />
            <FaceControlsSection
                selected_display=selected_display
                display_face=display_face
                set_display_face=set_display_face
            />
        </div>
    }
}

/// Compact card showing the currently assigned face and the primary
/// actions: choose a face, clear the assignment, open the full-screen
/// preview in a new tab.
#[component]
fn FaceAssignmentCard(
    selected_display: Memo<Option<api::DisplaySummary>>,
    display_face: ReadSignal<Option<Result<Option<api::DisplayFaceResponse>, String>>>,
    face_assignment_pending: ReadSignal<bool>,
    on_choose_face: Callback<()>,
    on_clear_face: Callback<()>,
) -> impl IntoView {
    let face_name = Signal::derive(move || match display_face.get() {
        None => "Loading...".to_owned(),
        Some(Ok(Some(face))) => face.effect.name,
        Some(Ok(None)) => "No face assigned".to_owned(),
        Some(Err(_)) => "Face unavailable".to_owned(),
    });
    let face_author = Signal::derive(move || match display_face.get() {
        Some(Ok(Some(face))) => Some(face.effect.author),
        _ => None,
    });
    let face_description = Signal::derive(move || {
        match display_face.get() {
        Some(Ok(Some(face))) => face.effect.description,
        Some(Err(error)) => error,
        _ => {
            "Display faces render full-screen HTML at the panel's native resolution. Pick one to get started."
                .to_owned()
        }
    }
    });
    let has_face = Signal::derive(move || matches!(display_face.get(), Some(Ok(Some(_)))));

    view! {
        <div class="rounded-xl border border-t-2 border-edge-subtle border-t-coral/25 bg-surface-raised/80 p-3 edge-glow">
            <div class="flex items-center gap-2 border-b border-edge-subtle/50 pb-2">
                <div class="flex h-6 w-6 items-center justify-center rounded-md bg-coral/10 text-coral/80">
                    <Icon icon=LuLayers width="13" height="13" />
                </div>
                <h3 class="text-[11px] font-semibold uppercase tracking-wide text-fg-secondary">
                    "Face"
                </h3>
                <div class="flex-1" />
                <Show when=move || selected_display.with(Option::is_some) fallback=|| ()>
                    {move || selected_display.get().map(|display| {
                        let href = display_preview_shell_url(&display.id);
                        view! {
                            <a
                                href=href
                                target="_blank"
                                rel="noopener"
                                class="rounded-md p-1 text-fg-tertiary transition hover:text-coral"
                                title="Open full-screen preview"
                            >
                                <Icon icon=LuExternalLink width="11" height="11" />
                            </a>
                        }
                    })}
                </Show>
            </div>
            <div class="flex flex-col gap-2 pt-2.5">
                <div class="flex items-start justify-between gap-2">
                    <div class="min-w-0">
                        <div class="truncate text-sm font-medium text-fg-primary">
                            {move || face_name.get()}
                        </div>
                        {move || face_author.get().map(|author| view! {
                            <div class="mt-0.5 text-[10px] uppercase tracking-wider text-fg-tertiary">
                                {"by "}{author}
                            </div>
                        })}
                    </div>
                </div>
                <p class="text-[11px] leading-relaxed text-fg-secondary">
                    {move || face_description.get()}
                </p>
                <div class="mt-1 flex items-center gap-2">
                    <button
                        type="button"
                        class="inline-flex flex-1 items-center justify-center gap-1.5 rounded-md border border-coral/40 bg-coral/12 px-3 py-1.5 text-[11px] font-medium uppercase tracking-wider text-coral transition hover:bg-coral/20 disabled:cursor-not-allowed disabled:opacity-50"
                        disabled=move || face_assignment_pending.get()
                        on:click=move |_| on_choose_face.run(())
                    >
                        <Icon icon=LuLayers width="12" height="12" />
                        {move || if has_face.get() { "Change face" } else { "Choose face" }}
                    </button>
                    <button
                        type="button"
                        class="inline-flex items-center justify-center gap-1.5 rounded-md border border-edge-subtle bg-surface-overlay px-3 py-1.5 text-[11px] uppercase tracking-wider text-fg-tertiary transition hover:border-status-error/35 hover:text-status-error disabled:cursor-not-allowed disabled:opacity-40"
                        disabled=move || face_assignment_pending.get() || !has_face.get()
                        on:click=move |_| on_clear_face.run(())
                        title="Clear face assignment"
                    >
                        <Icon icon=LuX width="12" height="12" />
                        "Clear"
                    </button>
                </div>
            </div>
        </div>
    }
}

#[component]
fn FaceCompositionSection(
    selected_display: Memo<Option<api::DisplaySummary>>,
    display_face: ReadSignal<Option<Result<Option<api::DisplayFaceResponse>, String>>>,
    set_display_face: WriteSignal<Option<Result<Option<api::DisplayFaceResponse>, String>>>,
) -> impl IntoView {
    let effects_ctx = use_context::<EffectsContext>();
    let (local_blend_mode, set_local_blend_mode) = signal(DisplayFaceBlendMode::Replace);
    let (local_opacity, set_local_opacity) = signal(1.0_f32);
    let has_face = Signal::derive(move || matches!(display_face.get(), Some(Ok(Some(_)))));
    let selected_blend_option = Memo::new(move |_| face_blend_option(local_blend_mode.get()));

    Effect::new(move |_| {
        sync_face_composition_from_server(display_face, set_local_blend_mode, set_local_opacity);
    });

    let commit_composition = Callback::new(
        move |(blend_mode, opacity): (Option<DisplayFaceBlendMode>, Option<f32>)| {
            if let Some(message) =
                snapshot_scene_lock_message(effects_ctx, "changing display face composition")
            {
                sync_face_composition_from_server(
                    display_face,
                    set_local_blend_mode,
                    set_local_opacity,
                );
                toasts::toast_error(&message);
                return;
            }
            let Some(display) = selected_display.get_untracked() else {
                return;
            };
            let display_id = display.id.clone();
            spawn_local(async move {
                match api::update_display_face_composition(&display_id, blend_mode, opacity).await {
                    Ok(face) => {
                        set_display_face.set(Some(Ok(Some(face))));
                    }
                    Err(error) => {
                        sync_face_composition_from_server(
                            display_face,
                            set_local_blend_mode,
                            set_local_opacity,
                        );
                        toasts::toast_error(&format!("Face composition update failed: {error}"));
                    }
                }
            });
        },
    );

    let commit_opacity = use_debounce_fn(
        move || {
            if !local_blend_mode.get_untracked().blends_with_effect() {
                return;
            }
            commit_composition.run((None, Some(local_opacity.get_untracked())));
        },
        90.0,
    );

    let set_mode = Callback::new(move |mode: DisplayFaceBlendMode| {
        set_local_blend_mode.set(mode);
        let opacity = if mode.blends_with_effect() {
            local_opacity.get_untracked()
        } else {
            1.0
        };
        commit_composition.run((Some(mode), Some(opacity)));
    });

    let on_opacity_input = Callback::new(move |event| {
        let Ok(raw) = event_target_value(&event).parse::<f32>() else {
            return;
        };
        set_local_opacity.set((raw / 100.0).clamp(0.0, 1.0));
        commit_opacity();
    });

    view! {
        <Show when=move || has_face.get() fallback=|| ()>
            <div class="rounded-xl border border-t-2 border-edge-subtle border-t-coral/20 bg-surface-raised/80 p-3 edge-glow">
                <div class="mb-3 flex items-center gap-2 border-b border-edge-subtle/50 pb-2">
                    <div class="flex h-6 w-6 items-center justify-center rounded-md bg-coral/10 text-coral/70">
                        <Icon icon=LuSlidersHorizontal width="13" height="13" />
                    </div>
                    <h3 class="text-[11px] font-semibold uppercase tracking-wide text-fg-secondary">
                        "Composition"
                    </h3>
                </div>
                <p class="text-[11px] leading-relaxed text-fg-secondary">
                    "Replace keeps the face in full control. Fusion modes let the live effect color interact with the face material instead of sitting passively underneath it."
                </p>
                <div class="mt-3 grid grid-cols-2 gap-2">
                    {FACE_BLEND_OPTIONS
                        .iter()
                        .copied()
                        .map(|option| {
                            let mode = option.mode;
                            view! {
                                <button
                                    type="button"
                                    class=move || {
                                        if local_blend_mode.get() == mode {
                                            "rounded-md border border-coral/45 bg-coral/12 px-3 py-2 text-[11px] font-medium uppercase tracking-wider text-coral transition"
                                        } else {
                                            "rounded-md border border-edge-subtle bg-surface-overlay px-3 py-2 text-[11px] uppercase tracking-wider text-fg-tertiary transition hover:border-coral/30 hover:text-fg-primary"
                                        }
                                    }
                                    on:click=move |_| set_mode.run(mode)
                                >
                                    {option.label}
                                </button>
                            }
                        })
                        .collect_view()}
                </div>
                <div class="mt-3 rounded-lg border border-edge-subtle/60 bg-surface-overlay/45 px-3 py-3">
                    <div class="flex items-center justify-between gap-2 text-[11px] uppercase tracking-wider text-fg-tertiary">
                        <span>"Selected Look"</span>
                        <span class="text-coral">{move || selected_blend_option.get().label}</span>
                    </div>
                    <p class="mt-2 text-[11px] leading-relaxed text-fg-secondary">
                        {move || selected_blend_option.get().blurb}
                    </p>
                </div>
                <Show when=move || local_blend_mode.get().blends_with_effect() fallback=|| ()>
                    <div class="mt-3 rounded-lg border border-edge-subtle/60 bg-surface-overlay/45 px-3 py-3">
                        <div class="mb-2 flex items-center justify-between gap-2 text-[11px] uppercase tracking-wider text-fg-tertiary">
                            <span>"Blend amount"</span>
                            <span class="text-coral">
                                {move || format!("{:.0}%", local_opacity.get() * 100.0)}
                            </span>
                        </div>
                        <input
                            type="range"
                            min="0"
                            max="100"
                            step="1"
                            class="w-full accent-[rgb(255,106,193)]"
                            prop:value=move || format!("{:.0}", local_opacity.get() * 100.0)
                            on:input=move |event| on_opacity_input.run(event)
                        />
                    </div>
                </Show>
            </div>
        </Show>
    }
}

/// Live face controls panel.
///
/// Renders the assigned face's `ControlPanel` with an optimistic-update
/// model: local control values tick immediately on input change, and
/// PATCH requests are debounced (75ms) before hitting the daemon. The
/// server response reconciles the optimistic state so normalized or
/// rejected values surface in the UI.
#[component]
fn FaceControlsSection(
    selected_display: Memo<Option<api::DisplaySummary>>,
    display_face: ReadSignal<Option<Result<Option<api::DisplayFaceResponse>, String>>>,
    set_display_face: WriteSignal<Option<Result<Option<api::DisplayFaceResponse>, String>>>,
) -> impl IntoView {
    let effects_ctx = use_context::<EffectsContext>();
    // Derived view of the face's control definitions for ControlPanel.
    let face_controls = Signal::derive(move || match display_face.get() {
        Some(Ok(Some(face))) => face.effect.controls,
        _ => Vec::new(),
    });
    let has_controls = Signal::derive(move || !face_controls.get().is_empty());
    let control_count = Signal::derive(move || face_controls.get().len());
    // Bundled presets shipped by the face author (e.g. SilkCircuit Dark,
    // Forge, Arctic for the SilkCircuit HUD). User-saved presets are a
    // future enhancement — the Face SDK currently only exposes bundled.
    let face_presets = Signal::derive(move || match display_face.get() {
        Some(Ok(Some(face))) => face.effect.presets,
        _ => Vec::new(),
    });
    let has_presets = Signal::derive(move || !face_presets.get().is_empty());

    // Local optimistic copy of control values. ControlPanel needs a
    // `HashMap<String, ControlValue>` signal and would thrash if we fed it
    // directly from the face response, because every PATCH would rebuild
    // the whole map on the server round-trip. Instead we seed it from the
    // server response and locally update on user input; the debounced
    // PATCH reconciles via set_display_face → Effect below.
    let (face_control_values, set_face_control_values) = signal(std::collections::HashMap::<
        String,
        hypercolor_types::effect::ControlValue,
    >::new());

    // Keep the local signal in sync when the face changes. We compare the
    // map before setting to avoid re-firing downstream effects on identical
    // data (e.g. after our own PATCH round-trip returns the same values).
    Effect::new(move |_| {
        let next = match display_face.get() {
            Some(Ok(Some(face))) => face.group.controls,
            _ => std::collections::HashMap::new(),
        };
        set_face_control_values.update(|current| {
            if *current != next {
                *current = next;
            }
        });
    });

    // Pending-updates buffer keyed by control name. Each user input
    // overwrites the prior pending value for that control, so a slider
    // drag only sends the final position when the debounce fires.
    let pending_updates: StoredValue<std::collections::HashMap<String, serde_json::Value>> =
        StoredValue::new(std::collections::HashMap::new());
    let show_locked_toast = use_debounce_fn_with_arg(
        move |message: String| {
            toasts::toast_error(&message);
        },
        150.0,
    );

    let flush_updates = use_debounce_fn(
        move || {
            if let Some(message) =
                snapshot_scene_lock_message(effects_ctx, "changing display face controls")
            {
                set_face_control_values.set(match display_face.get_untracked() {
                    Some(Ok(Some(face))) => face.group.controls,
                    _ => std::collections::HashMap::new(),
                });
                let _ = pending_updates.try_update_value(std::mem::take);
                toasts::toast_error(&message);
                return;
            }
            let Some(display) = selected_display.get_untracked() else {
                return;
            };
            let updates = pending_updates
                .try_update_value(std::mem::take)
                .unwrap_or_default();
            if updates.is_empty() {
                return;
            }
            let controls_json = serde_json::Value::Object(updates.into_iter().collect());
            let display_id = display.id;
            spawn_local(async move {
                match api::update_display_face_controls(&display_id, &controls_json).await {
                    Ok(face) => {
                        set_display_face.set(Some(Ok(Some(face))));
                    }
                    Err(error) => {
                        toasts::toast_error(&format!("Face control update failed: {error}"));
                    }
                }
            });
        },
        75.0,
    );

    let on_control_change = Callback::new(move |(name, value): (String, serde_json::Value)| {
        if let Some(message) =
            snapshot_scene_lock_message(effects_ctx, "changing display face controls")
        {
            show_locked_toast(message);
            return;
        }
        // Optimistic local update — mirrors what the ControlPanel expects
        // so sliders/toggles/color pickers feel immediate even before the
        // daemon acknowledges.
        let controls_snapshot = face_controls.get();
        if let Some(control_value) = json_to_face_control_value(&controls_snapshot, &name, &value) {
            set_face_control_values.update(|map| {
                map.insert(name.clone(), control_value);
            });
        }
        pending_updates.update_value(|pending| {
            pending.insert(name, value);
        });
        flush_updates();
    });

    // Preset application — lightweight path that PATCHes the entire
    // preset control map in one round-trip. Cancels any in-flight debounced
    // updates so a slider-drag-then-preset-click doesn't race.
    let apply_preset = Callback::new(
        move |preset_controls: std::collections::HashMap<
            String,
            hypercolor_types::effect::ControlValue,
        >| {
            let Some(display) = selected_display.get_untracked() else {
                return;
            };
            if let Some(message) =
                snapshot_scene_lock_message(effects_ctx, "applying display face presets")
            {
                toasts::toast_error(&message);
                return;
            }
            // Drop any queued per-key PATCH so it doesn't overwrite the
            // preset values we're about to send.
            let _ = pending_updates.try_update_value(std::mem::take);

            // Snapshot the pre-apply values so we can roll back if the
            // server rejects the PATCH. Without this, a failed apply
            // would leave the UI claiming the preset is active even
            // though the daemon never accepted it.
            let previous_values = face_control_values.get_untracked();

            // Optimistic local update so preset pills highlight
            // immediately without waiting for the round-trip.
            set_face_control_values.update(|map| {
                for (key, value) in &preset_controls {
                    map.insert(key.clone(), value.clone());
                }
            });

            let controls_json =
                crate::components::preset_matching::bundled_preset_to_json(&preset_controls);
            let display_id = display.id;
            spawn_local(async move {
                match api::update_display_face_controls(&display_id, &controls_json).await {
                    Ok(face) => {
                        set_display_face.set(Some(Ok(Some(face))));
                    }
                    Err(error) => {
                        // Restore pre-apply state so the "Assigned" pill
                        // no longer claims this preset is active.
                        set_face_control_values.set(previous_values);
                        toasts::toast_error(&format!("Preset apply failed: {error}"));
                    }
                }
            });
        },
    );

    view! {
        <Show when=move || has_controls.get() fallback=|| ()>
            <div class="rounded-xl border border-t-2 border-edge-subtle border-t-coral/20 bg-surface-raised/80 p-3 edge-glow">
                <div class="mb-3 flex items-center gap-2 border-b border-edge-subtle/50 pb-2">
                    <div class="flex h-6 w-6 items-center justify-center rounded-md bg-coral/10 text-coral/70">
                        <Icon icon=LuSettings2 width="13" height="13" />
                    </div>
                    <h3 class="text-[11px] font-semibold uppercase tracking-wide text-fg-secondary">
                        "Controls"
                    </h3>
                    <span class="text-[10px] text-fg-tertiary">
                        {move || format!("· {}", control_count.get())}
                    </span>
                </div>
                <Show when=move || has_presets.get() fallback=|| ()>
                    <FacePresetBar
                        presets=face_presets
                        control_values=Signal::from(face_control_values)
                        on_apply=apply_preset
                    />
                </Show>
                <ControlPanel
                    controls=face_controls
                    control_values=Signal::from(face_control_values)
                    accent_rgb=Signal::derive(|| "255, 106, 193".to_owned())
                    on_change=on_control_change
                />
            </div>
        </Show>
    }
}

#[component]
fn DisplayFacePickerModal(
    display_name: String,
    faces: ReadSignal<Option<Result<Vec<api::EffectSummary>, String>>>,
    current_face_id: Signal<Option<String>>,
    assigning: ReadSignal<bool>,
    #[prop(into)] on_select: Callback<String>,
    #[prop(into)] on_clear: Callback<()>,
    #[prop(into)] on_close: Callback<()>,
) -> impl IntoView {
    let (search, set_search) = signal(String::new());
    let thumbnails = use_context::<crate::thumbnails::ThumbnailStore>();
    let close_button = on_close.clone();
    let clear_button = on_clear.clone();

    view! {
        <DisplaysModalBackdrop wide=true on_close=on_close>
            <div
                class="flex max-h-[85vh] w-full max-w-4xl flex-col overflow-hidden rounded-xl border border-edge-subtle bg-surface-raised shadow-2xl edge-glow-accent accent-coral"
                on:click=|event| event.stop_propagation()
            >
                // ── Header ────────────────────────────────────────────
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

                // ── Search ────────────────────────────────────────────
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
                            on:input=move |event| set_search.set(event_target_value(&event))
                        />
                    </div>
                </div>

                // ── Gallery grid ─────────────────────────────────────
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

/// Visual card used inside the face gallery. Shows a thumbnail (captured
/// from the ThumbnailStore when the face has been rendered before) with a
/// gradient fallback, plus name/author and an "Assigned" pill for the
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
            // Thumbnail slab (4:3 aspect)
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
            // Metadata footer
            <div class="flex min-h-0 flex-col gap-1 border-t border-edge-subtle/50 px-3 py-2.5">
                <div class="truncate text-sm font-medium text-fg-primary">{name}</div>
                <div class="truncate text-[10px] uppercase tracking-wider text-fg-tertiary">
                    {format!("by {author}")}
                </div>
            </div>
        </button>
    }
}

/// Horizontal row of clickable preset pills rendered above the face
/// controls. Applies the preset's control map in one round-trip via
/// `api::update_display_face_controls` and highlights whichever preset
/// (if any) the current control values currently match.
#[component]
fn FacePresetBar(
    presets: Signal<Vec<hypercolor_types::effect::PresetTemplate>>,
    control_values: Signal<
        std::collections::HashMap<String, hypercolor_types::effect::ControlValue>,
    >,
    on_apply: Callback<std::collections::HashMap<String, hypercolor_types::effect::ControlValue>>,
) -> impl IntoView {
    view! {
        <div class="mb-3 flex flex-wrap items-center gap-1.5">
            <span class="text-[10px] uppercase tracking-wider text-fg-tertiary">
                "Presets"
            </span>
            {move || {
                let current = control_values.get();
                presets
                    .get()
                    .into_iter()
                    .map(|preset| {
                        let is_active =
                            crate::components::preset_matching::bundled_preset_matches_controls(
                                &current,
                                &preset.controls,
                            );
                        let preset_controls = preset.controls.clone();
                        let name = preset.name;
                        let pill_class = if is_active {
                            "inline-flex items-center rounded-full border border-coral/50 bg-coral/15 px-2.5 py-1 text-[10px] font-medium text-coral transition"
                        } else {
                            "inline-flex items-center rounded-full border border-edge-subtle bg-surface-overlay/50 px-2.5 py-1 text-[10px] text-fg-secondary transition hover:border-coral/40 hover:text-fg-primary"
                        };
                        view! {
                            <button
                                type="button"
                                class=pill_class
                                aria-pressed=if is_active { "true" } else { "false" }
                                on:click=move |_| on_apply.run(preset_controls.clone())
                            >
                                {name}
                            </button>
                        }
                    })
                    .collect_view()
            }}
        </div>
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
/// identity in the picker even before its first render.
fn face_gradient_fallback(name: &str) -> String {
    // Small FNV-1a-ish hash so the hue is stable per name but evenly
    // distributed across the 360° wheel.
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
