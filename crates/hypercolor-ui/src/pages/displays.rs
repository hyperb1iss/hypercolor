//! `/displays` — LCD-equipped devices and full-screen HTML faces.
//!
//! Three-region workspace (picker, cinematic preview, resizable control
//! column) for assigning full-screen faces to LCD devices and tuning
//! face controls.

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api;
use crate::app::{DisplaysContext, EffectsContext, WsContext};
use crate::components::modal::Modal;
use crate::components::page_header::{PageAccent, PageHeader};
use crate::components::resize_handle::ResizeHandle;
use crate::components::status_banner::{StatusBanner, StatusBannerTone};
use crate::display_preview_state::use_display_face_resource;
use crate::icons::*;
use crate::toasts;
use hypercolor_leptos_ext::events::document as browser_document;
use hypercolor_types::scene::{SceneKind, SceneMutationMode, ZoneRole};

mod display_picker;
mod face_composition;
mod face_controls;
mod face_picker;
mod right_panel;
mod simulator_modal;
mod workspace;

use display_picker::DisplayPicker;
use face_picker::DisplayFacePickerModal;
use right_panel::DisplayRightPanel;
use simulator_modal::{CreateSimulatorModal, EditSimulatorModal};
use workspace::{DisplayEmptyWorkspace, DisplayWorkspace};

type DisplaysResource = LocalResource<Result<Vec<api::DisplaySummary>, String>>;

/// Resizable right-side control column — bounds and localStorage key.
const MIN_RIGHT_WIDTH: f64 = 280.0;
const MAX_RIGHT_WIDTH: f64 = 600.0;
const DEFAULT_RIGHT_WIDTH: f64 = 360.0;
const LS_KEY_RIGHT_WIDTH: &str = "hc-disp-right-width";

#[component]
pub fn DisplaysPage() -> impl IntoView {
    let fx = expect_context::<EffectsContext>();
    let ws = expect_context::<WsContext>();
    let displays = expect_context::<DisplaysContext>().displays_resource;
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

    // Keep the workspace anchored to a real display whenever the list changes.
    // This covers first load, reconnects, and deletions that invalidate the
    // previously selected id.
    Effect::new(move |_| {
        let Some(Ok(items)) = displays.get() else {
            return;
        };

        let current_id = selected_id.get();
        if current_id
            .as_ref()
            .is_some_and(|id| items.iter().any(|display| display.id == *id))
        {
            return;
        }

        let next_id = items.first().map(|display| display.id.clone());
        if current_id != next_id {
            set_selected_id.set(next_id);
        }
    });

    let selected_display = Memo::new(move |_| {
        let id = selected_id.get()?;
        let snapshot = displays.get();
        let items = snapshot.as_ref()?.as_ref().ok()?;
        items.iter().find(|display| display.id == id).cloned()
    });
    let selected_display_id = Signal::derive(move || {
        selected_display.with(|display| display.as_ref().map(|item| item.id.clone()))
    });
    let display_face_resource = use_display_face_resource(
        selected_display_id,
        Signal::derive(move || face_refresh_tick.get()),
    );

    Effect::new(move |_| {
        if selected_display_id.get().is_none() {
            set_display_face.set(None);
            set_face_picker_open.set(false);
            set_face_assignment_pending.set(false);
            return;
        }

        if let Some(result) = display_face_resource.get() {
            set_display_face.set(Some(result));
        }
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

    Effect::new(
        move |previous_scene_event: Option<Option<crate::ws::SceneEventHint>>| {
            let current_scene_event = ws.last_scene_event.get();
            if previous_scene_event.as_ref() == Some(&current_scene_event) {
                return current_scene_event;
            }

            if current_scene_event.as_ref().is_some_and(|scene_event| {
                scene_event.event_type == "active_scene_changed"
                    || scene_event.render_group_role == Some(ZoneRole::Display)
            }) {
                set_face_refresh_tick.update(|value| *value = value.wrapping_add(1));
            }

            current_scene_event
        },
    );

    let current_face_id = Signal::derive(move || {
        display_face
            .get()
            .and_then(Result::ok)
            .flatten()
            .map(|face| face.effect.id)
    });
    let degraded_face = Memo::new(move |_| {
        let effect_error = fx.last_effect_error.get()?;
        let effect = fx.effects_index.with(|effects| {
            effects
                .iter()
                .find(|entry| entry.effect.id == effect_error.effect_id)
                .map(|entry| entry.effect.clone())
        })?;
        if !effect.category.eq_ignore_ascii_case("display") {
            return None;
        }

        Some((
            effect.name,
            match effect_error.fallback.as_deref() {
                Some("clear_groups") => {
                    "The daemon cleared this face assignment after a render failure.".to_owned()
                }
                Some(fallback) if !fallback.is_empty() => {
                    format!("The daemon applied fallback \"{fallback}\" after a render failure.")
                }
                _ => "The daemon reported a render failure for this face.".to_owned(),
            },
        ))
    });

    let (assign_scope, set_assign_scope) = signal(api::DisplayFaceScope::Default);
    let assign_face = Callback::new(move |effect_id: String| {
        let scope = assign_scope.get_untracked();
        // Default-layer writes never touch the scene, so the snapshot lock
        // only gates scene-scoped assignment.
        if scope == api::DisplayFaceScope::Scene
            && let Some(message) =
                snapshot_scene_lock_message(Some(fx), "assigning or changing display faces")
        {
            toasts::toast_error(&message);
            return;
        }
        fx.set_last_effect_error.set(None);
        let Some(display) = selected_display.get_untracked() else {
            return;
        };
        let display_id = display.id.clone();
        let display_name = display.name.clone();
        set_face_assignment_pending.set(true);
        spawn_local(async move {
            match api::set_display_face(&display_id, &effect_id, scope).await {
                Ok(face) => {
                    let assigned_name = face.effect.name.clone();
                    set_display_face.set(Some(Ok(Some(face))));
                    set_face_refresh_tick.update(|value| *value = value.wrapping_add(1));
                    set_face_picker_open.set(false);
                    set_face_assignment_pending.set(false);
                    let suffix = match scope {
                        api::DisplayFaceScope::Default => "as its default face",
                        api::DisplayFaceScope::Scene => "for this scene",
                    };
                    toasts::toast_success(&format!(
                        "Assigned {assigned_name} to {display_name} {suffix}"
                    ));
                }
                Err(error) => {
                    set_face_assignment_pending.set(false);
                    toasts::toast_error(&format!("Face assignment failed: {error}"));
                }
            }
        });
    });
    let clear_face = Callback::new(move |_| {
        let scope = assign_scope.get_untracked();
        if scope == api::DisplayFaceScope::Scene
            && let Some(message) = snapshot_scene_lock_message(Some(fx), "clearing display faces")
        {
            toasts::toast_error(&message);
            return;
        }
        fx.set_last_effect_error.set(None);
        let Some(display) = selected_display.get_untracked() else {
            return;
        };
        let display_id = display.id.clone();
        let display_name = display.name.clone();
        set_face_assignment_pending.set(true);
        spawn_local(async move {
            match api::delete_display_face(&display_id, scope).await {
                Ok(()) => {
                    set_display_face.set(None);
                    set_face_refresh_tick.update(|value| *value = value.wrapping_add(1));
                    set_face_assignment_pending.set(false);
                    let suffix = match scope {
                        api::DisplayFaceScope::Default => "default face",
                        api::DisplayFaceScope::Scene => "scene face",
                    };
                    toasts::toast_success(&format!("Cleared {suffix} from {display_name}"));
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
    // Only snapshot-locked scenes get a banner: the daemon rejects their
    // mutations outright, so the unblock path has to be visible. A merely
    // active named scene is normal operation, not a warning.
    let snapshot_locked_warning = Memo::new(move |_| {
        (fx.active_scene_kind.get() == Some(SceneKind::Named)
            && fx.active_scene_mutation_mode.get() == Some(SceneMutationMode::Snapshot))
        .then(|| {
            fx.active_scene_name
                .get()
                .unwrap_or_else(|| "Active scene".to_owned())
        })
    });
    let (returning_to_default, set_returning_to_default) = signal(false);
    let on_return_to_default = {
        Callback::new(move |_| {
            if returning_to_default.get_untracked() {
                return;
            }
            set_returning_to_default.set(true);
            spawn_local(async move {
                if api::deactivate_scene().await.is_ok() {
                    fx.set_last_effect_error.set(None);
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
        <div class="flex h-full min-h-0 flex-col overflow-hidden">
            <PageHeader
                icon=LuMonitor
                title="Displays"
                tagline="Assign faces to LCD screens"
                accent=PageAccent::Green
            />
            {move || snapshot_locked_warning.get().map(|scene_name| view! {
                <div class="px-6 pt-3">
                    <StatusBanner
                        tone=StatusBannerTone::Warning
                        title="Snapshot Scene Locked"
                        subject=scene_name
                        detail=" is snapshot-locked. Return to Default before assigning, clearing, or tuning a display face."
                    >
                        <button
                            class="shrink-0 rounded-lg border border-status-warning/28 px-3 py-1.5 text-[11px] font-medium text-status-warning/92 transition-all duration-200 hover:bg-status-warning/8 disabled:cursor-wait disabled:opacity-60"
                            disabled=move || returning_to_default.get()
                            on:click=move |_| on_return_to_default.run(())
                        >
                            {move || if returning_to_default.get() {
                                "Returning..."
                            } else {
                                "Return to Default"
                            }}
                        </button>
                    </StatusBanner>
                </div>
            })}
            {move || degraded_face.get().map(|(effect_name, detail)| view! {
                <div class="px-6 pt-3">
                    <StatusBanner
                        tone=StatusBannerTone::Error
                        title="Degraded Face"
                        subject=effect_name
                        detail=format!(" is degraded. {detail}")
                    />
                </div>
            })}
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
                    <Show
                        when=move || selected_display.with(Option::is_some)
                        fallback=|| view! { <DisplayEmptyWorkspace /> }
                    >
                        <DisplayWorkspace
                            selected_display=selected_display
                            display_face=display_face
                            face_refresh_tick=face_refresh_tick
                        />
                    </Show>
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
                            set_face_refresh_tick=set_face_refresh_tick
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
                                    scope=assign_scope
                                    set_scope=set_assign_scope
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
    #[prop(into, optional)] label: MaybeProp<String>,
    children: Children,
) -> impl IntoView {
    view! {
        <Modal
            on_close=on_close
            label=label
            container_class="fixed inset-0 z-50 grid place-items-center p-4 animate-enter-fade"
            backdrop_class="absolute inset-0 bg-black/65 backdrop-blur-sm"
        >
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
        </Modal>
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
    if let Some(body) = browser_document().and_then(|d| d.body()) {
        if active {
            let _ = body.class_list().add_1("resizing");
        } else {
            let _ = body.class_list().remove_1("resizing");
        }
    }
}
