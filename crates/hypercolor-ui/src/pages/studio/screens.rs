//! Screens in Studio.
//!
//! A screen is found where its hardware is: a box on its zone's canvas and
//! a card in the zone's section of the device pane. Selecting either, with
//! nothing but screens selected, turns the Stage header's chip and the
//! composition panel to those screens. This module is the Studio side of
//! that: the state every surface shares ([`Screens`]), what it lends the
//! layer panel ([`Screens::host`]), the row a face saved with its screen
//! gets ([`KeptFaceCard`]), and "Copy to every scene"
//! ([`CopyEverywhereHost`]). The rules themselves are in
//! [`super::screen_model`].

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use hypercolor_types::layer::{LayerSource, SceneLayer, SceneLayerId};
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_icons::Icon;

use crate::api::{self, DisplayFaceScope};
use crate::app::{CapabilitiesContext, DevicesContext, DisplaysContext, WsContext};
use crate::components::control_panel::ControlPanel;
use crate::components::layer_panel::source::add_layer_request;
use crate::components::layer_panel::{
    Backdrop, DisplayComposition, NewLayerDraft, ScreenFaceLine, ScreenHost, ScreenView,
    ScreenViewRow,
};
use crate::components::modal::Modal;
use crate::components::section_label::{LabelSize, LabelTone, label_class};
use crate::control_session::{
    ControlPatchConfig, ControlPatchFn, ControlPatchFuture, use_control_patch_session,
};
use crate::icons::*;
use crate::optimistic_controls::ControlValueMap;
use crate::toasts;

use super::StudioContext;
use super::history::{AddOutcome, LayerAdd};
use super::screen_model::{
    Composition, FaceReport, KEPT_CHIP, KEPT_NOTICE, KeptFaceRow, PlacedScreen, ScreenPanel,
    ScreenRef, StackHome, add_outcome_words, card_face, copy_layer_to_scene, copy_prompt,
    is_screen_layer, kept_face_row, placed_screens, plan_add_to_all_screens, plan_screen_adds,
    screen_panel, screens_label, selected_screens, takes_copies, unplaced_screens,
};

/// Flush debounce for face-setting edits, the layer inspector's cadence.
const FACE_CONTROLS_DEBOUNCE_MS: f64 = 120.0;

/// Accent for a face's control groups: the app accent.
const FACE_ACCENT_RGB: &str = "225, 53, 255";

/// A layer to copy onto its screens in every scene.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CopyRequest {
    /// The zone whose stack holds the layer.
    pub zone_id: String,
    pub layer_id: SceneLayerId,
    /// What the layer goes by.
    pub name: String,
}

/// Everything Studio shares about screens.
#[derive(Clone, Copy)]
pub(crate) struct Screens {
    /// Every screen a zone of the scene places.
    pub placed: Memo<Vec<PlacedScreen>>,
    /// The open zone's screens the selection names, when screens are all
    /// it names.
    pub selected: Memo<Vec<PlacedScreen>>,
    /// What the composition panel shows for them.
    pub panel: Memo<Option<ScreenPanel>>,
    /// What the one selected screen's display reports about its face.
    pub face: Signal<Option<api::DisplayFaceResponse>>,
    /// Read that again after a write to the saved face.
    pub refresh_face: Callback<()>,
    /// The saved face's row, while a saved face is what that screen shows.
    pub kept: Memo<Option<KeptFaceRow>>,
    /// The saved faces showing on screens the scene gives nothing of
    /// their own, by display: what those screens' cards name.
    pub kept_names: Signal<HashMap<String, String>>,
    /// Moves to ask the composition panel for its picker on the screens.
    pub add_requests: RwSignal<u64>,
    /// The Stage shows the one selected screen alone, at full size.
    pub focus: RwSignal<bool>,
    /// The layer "Copy to every scene" is asking about.
    pub copy: RwSignal<Option<CopyRequest>>,
}

/// Every screen a zone of `active_scene` places, with where each keeps
/// its layers on the daemon the page talks to.
pub(crate) fn use_placed_screens(
    active_scene: Signal<Option<api::SceneDocument>>,
) -> Memo<Vec<PlacedScreen>> {
    let caps = expect_context::<CapabilitiesContext>();
    let devices = expect_context::<DevicesContext>().devices_resource;
    let layered = Memo::new(move |_| caps.has(api::SCREEN_LAYERS_CAPABILITY));
    let physical_ids = Memo::new(move |_| {
        devices
            .get()
            .and_then(Result::ok)
            .unwrap_or_default()
            .into_iter()
            .map(|device| (device.layout_device_id, device.id))
            .collect::<HashMap<String, String>>()
    });
    Memo::new(move |_| {
        active_scene.with(|scene| {
            scene.as_ref().map_or_else(Vec::new, |scene| {
                physical_ids.with(|ids| placed_screens(&scene.zones, ids, layered.get()))
            })
        })
    })
}

/// The saved faces showing on the screens the scene gives nothing of
/// their own, by display. A card names its screen's face, and for such a
/// screen that face is the saved one, which only the display's face route
/// knows. The list of those screens is what is read, so a control edit
/// elsewhere in the scene reads nothing again; `tick` moves to read after
/// a write to a saved face.
pub(crate) fn use_kept_names(
    active_scene: Signal<Option<api::SceneDocument>>,
    placed: Memo<Vec<PlacedScreen>>,
    tick: ReadSignal<u64>,
) -> Signal<HashMap<String, String>> {
    let ws = expect_context::<WsContext>();
    let scene_generation = Memo::new(move |_| {
        ws.last_scene_event
            .with(|hint| hint.as_ref().map_or(0, |hint| hint.structural_generation))
    });
    let faceless = Memo::new(move |_| {
        placed.with(|placed| {
            active_scene.with(|scene| {
                let Some(scene) = scene.as_ref() else {
                    return Vec::new();
                };
                placed
                    .iter()
                    .filter(|screen| card_face(&scene.zones, screen).is_none())
                    .filter_map(|screen| screen.device_id.clone())
                    .collect::<Vec<_>>()
            })
        })
    });
    let resource = api::daemon_resource(move || {
        let _ = tick.get();
        let _ = scene_generation.get();
        let devices = faceless.get();
        async move {
            let mut names = HashMap::new();
            for device_id in devices {
                if let Ok(Some(face)) = api::fetch_display_face(&device_id).await
                    && let Some(row) = kept_face_row(Some(&FaceReport::from(&face)))
                {
                    names.insert(device_id, row.name);
                }
            }
            Ok::<_, api::ApiError>(names)
        }
    });
    Signal::derive(move || resource.get().and_then(Result::ok).unwrap_or_default())
}

/// One display's face as its display reports it, read again on `tick` and
/// on structural scene events, and the saved face's row when that is what
/// the display shows.
pub(crate) fn use_display_face(
    device_id: Signal<Option<String>>,
    tick: ReadSignal<u64>,
) -> (
    Signal<Option<api::DisplayFaceResponse>>,
    Memo<Option<KeptFaceRow>>,
) {
    let ws = expect_context::<WsContext>();
    let scene_generation = Memo::new(move |_| {
        ws.last_scene_event
            .with(|hint| hint.as_ref().map_or(0, |hint| hint.structural_generation))
    });
    let resource = api::daemon_resource(move || {
        let _ = tick.get();
        let _ = scene_generation.get();
        let device_id = device_id.get();
        async move {
            match device_id {
                Some(device_id) => api::fetch_display_face(&device_id).await,
                None => Ok(None),
            }
        }
    });
    // The resource keeps the last screen's face while the next read is in
    // flight; only a face for the screen asked about now counts.
    let face = Signal::derive(move || {
        let device_id = device_id.get()?;
        resource
            .get()
            .and_then(Result::ok)
            .flatten()
            .filter(|face| face.device_id == device_id)
    });
    let kept = Memo::new(move |_| {
        let report = face.with(|face| face.as_ref().map(FaceReport::from));
        kept_face_row(report.as_ref())
    });
    (face, kept)
}

/// What a saved face's row needs of whoever shows it.
#[derive(Clone, Copy)]
pub(crate) struct KeptFace {
    /// The display the face is saved with.
    pub device_id: Signal<Option<String>>,
    /// What that display reports.
    pub face: Signal<Option<api::DisplayFaceResponse>>,
    /// The row, while the saved face is what the display shows.
    pub row: Signal<Option<KeptFaceRow>>,
    /// Read the face again after a write to it.
    pub refresh_face: Callback<()>,
    /// Read the scene again after the face became a layer of it.
    pub refresh_scene: Callback<()>,
}

impl Screens {
    pub fn new(
        active_scene: Signal<Option<api::SceneDocument>>,
        selected_surface_id: RwSignal<Option<String>>,
        selected_output_ids: RwSignal<HashSet<String>>,
    ) -> Self {
        let placed = use_placed_screens(active_scene);
        let selected = Memo::new(move |_| {
            let Some(zone_id) = selected_surface_id.get() else {
                return Vec::new();
            };
            placed.with(|placed| {
                selected_output_ids.with(|selection| {
                    selected_screens(placed, &zone_id, selection)
                        .into_iter()
                        .cloned()
                        .collect()
                })
            })
        });
        let panel = Memo::new(move |_| {
            selected.with(|selected| {
                let screens: Vec<&PlacedScreen> = selected.iter().collect();
                active_scene.with(|scene| screen_panel(&scene.as_ref()?.zones, &screens))
            })
        });

        // The one selected screen's face rides the display's face route,
        // which reports the scene's assignment or the face saved with the
        // screen, whichever stands. It is read again on selection, after a
        // write from this page, and on structural scene events (the daemon
        // publishes one for a saved face too), so it never needs a timer.
        let face_device = Memo::new(move |_| {
            selected.with(|selected| match selected.as_slice() {
                [one] => one.device_id.clone(),
                _ => None,
            })
        });
        let (face_tick, set_face_tick) = signal(0_u64);
        let (face, kept) = use_display_face(face_device.into(), face_tick);
        let kept_names = use_kept_names(active_scene, placed, face_tick);

        let focus = RwSignal::new(false);
        // Focus is a view of one selected screen: it ends with it.
        Effect::new(move |_| {
            if selected.with(|selected| selected.len() != 1) && focus.get_untracked() {
                focus.set(false);
            }
        });

        Self {
            placed,
            selected,
            panel,
            face,
            refresh_face: Callback::new(move |()| {
                set_face_tick.update(|tick| *tick = tick.wrapping_add(1));
            }),
            kept,
            kept_names,
            add_requests: RwSignal::new(0),
            focus,
            copy: RwSignal::new(None),
        }
    }

    /// The name of the face the selected screens show: the top layer of
    /// their own, else the face saved with the one screen.
    pub fn face_name(
        self,
        studio: StudioContext,
        effect_name: impl Fn(&str) -> Option<String>,
    ) -> Option<String> {
        let top = self.panel.with(|panel| {
            let row = panel.as_ref()?.rows.first()?;
            studio.active_scene.with(|scene| {
                let layer = scene
                    .as_ref()?
                    .zones
                    .iter()
                    .find(|zone| zone.id.to_string() == row.zone_id)?
                    .layers
                    .iter()
                    .find(|layer| layer.id == row.layer_id)?;
                Some(layer_name(layer, &effect_name))
            })
        });
        top.or_else(|| {
            self.kept
                .with(|kept| kept.as_ref().map(|kept| kept.name.clone()))
        })
    }

    /// The face one screen's card names: the top layer of its own, else
    /// the face saved with it.
    pub fn card_face_name(
        self,
        studio: StudioContext,
        screen: &PlacedScreen,
        effect_name: impl Fn(&str) -> Option<String>,
    ) -> Option<String> {
        studio
            .active_scene
            .with(|scene| {
                card_face(&scene.as_ref()?.zones, screen)
                    .map(|layer| layer_name(layer, &effect_name))
            })
            .or_else(|| {
                let device_id = screen.device_id.as_ref()?;
                self.kept_names.with(|names| names.get(device_id).cloned())
            })
    }

    /// Open the composition panel on the screens selected, with the
    /// picker when `add`.
    pub fn open(self, studio: StudioContext, add: bool) {
        studio.composition_open.set(true);
        if add {
            self.add_requests
                .update(|asked| *asked = asked.wrapping_add(1));
        }
    }

    /// What Studio lends the layer panel about screens.
    pub fn host(self, studio: StudioContext) -> ScreenHost {
        let displays = expect_context::<DisplaysContext>().displays_resource;
        let view = Memo::new(move |_| {
            let panel = self.panel.get()?;
            let selected = self.selected.get();
            studio
                .active_scene
                .with(|scene| Some(screen_view(&scene.as_ref()?.zones, &panel, &selected)))
        });
        // "All screens" reaches every placed screen; it is offered while
        // that is more than the ones selected.
        let all_screens = Signal::derive(move || {
            let selected = self.selected.with(Vec::len);
            let placed = self.placed.get();
            let names: Vec<(String, String)> = displays
                .get()
                .and_then(Result::ok)
                .unwrap_or_default()
                .into_iter()
                .map(|display| (display.id, display.name))
                .collect();
            let plan = studio.active_scene.with(|scene| {
                scene.as_ref().map(|scene| {
                    plan_add_to_all_screens(
                        &scene.zones,
                        &placed,
                        &unplaced_screens(&names, &placed),
                    )
                })
            })?;
            (plan.screen_count() > selected).then(|| plan.summary())
        });
        let add = Callback::new(move |(draft, everywhere): (NewLayerDraft, bool)| {
            let Some(scene) = studio.history.current_scene() else {
                toasts::toast_error("No active scene is available");
                return;
            };
            let placed = self.placed.get_untracked();
            let selected = self.selected.get_untracked();
            let screens: Vec<&PlacedScreen> = if everywhere {
                placed.iter().collect()
            } else {
                selected.iter().collect()
            };
            let adds: Vec<LayerAdd> = plan_screen_adds(&scene.zones, &screens)
                .into_iter()
                .map(|plan| {
                    let mut request =
                        add_layer_request(draft.name.clone(), draft.source.clone(), plan.beneath);
                    request.scope = plan.scope;
                    LayerAdd {
                        zone_id: plan.zone_id,
                        request,
                        lands_on: plan.screens,
                    }
                })
                .collect();
            if adds.is_empty() {
                toasts::toast_error("There is no screen here that can take a layer yet");
                return;
            }
            let name = draft.name.clone().unwrap_or_else(|| "Layer".to_owned());
            studio.history.add_layers(
                name.clone(),
                adds,
                Callback::new(move |outcome: AddOutcome| {
                    let words = add_outcome_words(&name, &outcome.landed, &outcome.failed);
                    if outcome.failed.is_empty() {
                        toasts::toast_success(&words);
                    } else {
                        toasts::toast_error(&words);
                    }
                    self.refresh_face.run(());
                }),
            );
        });
        let faces = Memo::new(move |_| {
            let zone_id = studio.selected_surface_id.get()?;
            let placed = self.placed.get();
            studio.active_scene.with(|scene| {
                let zone = scene
                    .as_ref()?
                    .zones
                    .iter()
                    .find(|zone| zone.id.to_string() == zone_id)?;
                Some(
                    zone.layers
                        .iter()
                        .rev()
                        .filter(|layer| is_screen_layer(zone, layer))
                        .filter_map(|layer| {
                            let outputs = &layer.scope.as_ref()?.outputs;
                            let on: Vec<&PlacedScreen> = placed
                                .iter()
                                .filter(|screen| {
                                    screen.zone_id == zone_id && outputs.contains(&screen.output_id)
                                })
                                .collect();
                            (!on.is_empty()).then(|| ScreenFaceLine {
                                layer_id: layer.id,
                                screens: screens_label(&on),
                                outputs: on.iter().map(|screen| screen.output_id.clone()).collect(),
                            })
                        })
                        .collect::<Vec<_>>(),
                )
            })
        });
        ScreenHost {
            view: view.into(),
            add,
            all_screens,
            leave: Callback::new(move |()| {
                studio.selected_output_ids.set(HashSet::new());
            }),
            copy: Callback::new(move |(zone_id, layer_id, name)| {
                self.copy.set(Some(CopyRequest {
                    zone_id,
                    layer_id,
                    name,
                }));
            }),
            lead: {
                let kept = KeptFace {
                    device_id: Signal::derive(move || {
                        self.selected.with(|selected| match selected.as_slice() {
                            [one] => one.device_id.clone(),
                            _ => None,
                        })
                    }),
                    face: self.face,
                    row: self.kept.into(),
                    refresh_face: self.refresh_face,
                    refresh_scene: studio.refresh_scene,
                };
                ViewFn::from(move || view! { <KeptFaceCard kept=kept /> })
            },
            faces: Signal::derive(move || faces.get().unwrap_or_default()),
            select: Callback::new(move |outputs: Vec<String>| {
                studio
                    .selected_output_ids
                    .set(outputs.into_iter().collect());
            }),
            add_requests: self.add_requests.into(),
            replaced: Callback::new(move |(zone_id, old, new): (String, _, _)| {
                studio.history.layer_replaced(&zone_id, old, new);
            }),
        }
    }
}

/// What a layer goes by: its own name, else its effect's, else its kind.
fn layer_name(layer: &SceneLayer, effect_name: &impl Fn(&str) -> Option<String>) -> String {
    let effect = match &layer.source {
        LayerSource::Effect { effect_id, .. } => effect_name(&effect_id.to_string()),
        _ => None,
    };
    crate::zones::surface::layer_label(layer, effect.as_deref())
}

/// The panel's model of `selected`, as the layer panel takes it: each row
/// with how it is addressed, held, and reported on.
pub(crate) fn screen_view(
    zones: &[api::ZoneResource],
    panel: &ScreenPanel,
    selected: &[PlacedScreen],
) -> ScreenView {
    // A screen layer's health is published under the view of each screen
    // it is on.
    let view_of = |output_id: &str| {
        zones.iter().find_map(|zone| {
            zone.view_of
                .as_ref()
                .filter(|view| view.member.0 == output_id)
                .map(|_| zone.id.to_string())
        })
    };
    let layer_of = |zone_id: &str, layer_id: SceneLayerId| {
        zones
            .iter()
            .find(|zone| zone.id.to_string() == zone_id)
            .and_then(|zone| zone.layers.iter().find(|layer| layer.id == layer_id))
    };
    let owner = selected.first().map(|screen| screen.zone_id.as_str());
    let rows = panel
        .rows
        .iter()
        .map(|row| {
            let owned = Some(row.zone_id.as_str()) == owner;
            let health_zones = if owned {
                let scope =
                    layer_of(&row.zone_id, row.layer_id).and_then(|layer| layer.scope.clone());
                selected
                    .iter()
                    .filter(|screen| {
                        scope
                            .as_ref()
                            .is_some_and(|scope| scope.outputs.contains(&screen.output_id))
                    })
                    .filter_map(|screen| view_of(&screen.output_id))
                    .collect()
            } else {
                vec![row.zone_id.clone()]
            };
            let display = match &row.composition {
                Composition::Layer => None,
                Composition::Display(device_id) => zones
                    .iter()
                    .find(|zone| zone.id.to_string() == row.zone_id)
                    .and_then(|zone| zone.display_target.as_ref())
                    .map(|target| DisplayComposition {
                        device_id: device_id.clone(),
                        blend: target.blend_mode,
                        opacity: target.opacity,
                    }),
            };
            ScreenViewRow {
                zone_id: row.zone_id.clone(),
                layer_id: row.layer_id,
                backdrop: row.meets_backdrop.then_some(Backdrop { display }),
                health_zones,
                targets: owned,
                copies: true,
            }
        })
        .collect();
    let several = selected.len() > 1;
    ScreenView {
        heading: panel.heading.to_owned(),
        rows,
        behind_heading: panel.behind_heading.clone(),
        behind: panel.behind.iter().map(|row| row.layer_id).collect(),
        target: panel.title.clone(),
        add_label: if several {
            "Add layer to these screens"
        } else {
            "Add layer to this screen"
        }
        .to_owned(),
        no_add: selected
            .iter()
            .all(|screen| screen.home.is_none())
            .then(|| "This screen is not connected, so it cannot take a layer yet.".to_owned()),
    }
}

/// The controls a stored face's layer holds.
fn face_controls(face: &api::DisplayFaceResponse) -> ControlValueMap {
    face.zone
        .layers
        .iter()
        .find_map(|layer| match &layer.source {
            LayerSource::Effect { controls, .. } => Some(controls.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

/// The row of a face saved with its screen, shown first among the
/// screen's layers while that face is what the screen shows. Its settings
/// are edited here and written to the saved face, so they show in every
/// scene that leaves the screen to it, which the row says. "Use in this
/// scene" makes the scene its own copy as a layer; "Remove" drops the
/// saved face everywhere.
#[component]
fn KeptFaceCard(kept: KeptFace) -> impl IntoView {
    let device_id = kept.device_id;
    let (busy, set_busy) = signal(false);

    // Both actions resolve after an await, possibly once the row has been
    // swapped out by a selection change, so every write after it goes
    // through the fallible forms and a disposed row stays silent.
    let promote = move |_| {
        let (Some(device_id), Some(face)) = (device_id.get_untracked(), kept.face.get_untracked())
        else {
            return;
        };
        set_busy.set(true);
        spawn_local(async move {
            match api::promote_default_face(&device_id, &face).await {
                Ok(_) => toasts::toast_success("Face copied into this scene"),
                Err(error) => {
                    toasts::toast_error(&format!(
                        "Could not copy the face into this scene: {error}"
                    ));
                }
            }
            set_busy.try_set(false);
            kept.refresh_face.try_run(());
            kept.refresh_scene.try_run(());
        });
    };
    let remove = move |_| {
        let Some(device_id) = device_id.get_untracked() else {
            return;
        };
        set_busy.set(true);
        spawn_local(async move {
            match api::delete_display_face(&device_id, DisplayFaceScope::Default).await {
                Ok(()) => toasts::toast_success("Saved face removed"),
                Err(error) => {
                    toasts::toast_error(&format!("Could not remove the saved face: {error}"));
                }
            }
            set_busy.try_set(false);
            kept.refresh_face.try_run(());
            kept.refresh_scene.try_run(());
        });
    };

    view! {
        <Show when=move || kept.row.with(Option::is_some)>
            <article
                class="overflow-hidden rounded-xl border border-edge-subtle/70 bg-surface-sunken/50"
                data-kept-face=""
            >
                <div class="grid grid-cols-[auto_minmax(0,1fr)] items-center gap-x-2.5 px-3 py-2.5">
                    <span
                        class="row-span-2 flex h-8 w-8 shrink-0 items-center justify-center rounded-lg"
                        style="background: rgba(225, 53, 255, 0.14)"
                    >
                        <Icon icon=LuMonitor width="15px" height="15px" style="color: rgb(225, 53, 255)" />
                    </span>
                    <span class="min-w-0 truncate text-sm font-semibold text-fg-primary">
                        {move || kept.row.with(|row| row.as_ref().map(|row| row.name.clone()))}
                    </span>
                    <div class="col-start-2 flex min-w-0 items-center gap-1.5">
                        <span class="shrink-0 text-[10px] font-mono uppercase tracking-[0.12em] text-fg-tertiary/65">
                            "Face"
                        </span>
                        <span
                            class="inline-flex min-w-0 items-center gap-1 rounded-full border border-edge-subtle bg-surface-overlay px-1.5 py-0.5 text-[10px] font-medium text-fg-secondary"
                            title="This face is saved with the screen, not in this scene"
                        >
                            <span class="flex shrink-0 text-accent">
                                <Icon icon=LuSave width="10px" height="10px" />
                            </span>
                            <span class="min-w-0 truncate">{KEPT_CHIP}</span>
                        </span>
                    </div>
                </div>
                // Keyed on the face, not on its controls: an edit reads the
                // face again, and the editor must outlive its own echoes.
                {move || {
                    let device_id = device_id.get()?;
                    let effect_id = kept.row.with(|row| row.as_ref().map(|row| row.effect_id.clone()))?;
                    Some(view! {
                        <div class="space-y-3 border-t border-edge-subtle/45 px-3 py-3">
                            <p class="text-[11px] leading-relaxed text-fg-tertiary" data-kept-notice="">
                                {KEPT_NOTICE}
                            </p>
                            <KeptFaceControls kept=kept device_id=device_id effect_id=effect_id />
                        </div>
                    })
                }}
                <div class="flex flex-wrap items-center gap-2 border-t border-edge-subtle/45 px-3 py-2.5">
                    <button
                        type="button"
                        class="inline-flex items-center gap-1.5 rounded-md border border-accent-muted bg-accent/10 px-2.5 py-1.5 text-[11px] font-medium text-fg-primary transition-colors btn-press hover:bg-accent/15 disabled:cursor-wait disabled:opacity-60"
                        title="Make this scene its own copy of the face, as a layer"
                        disabled=move || busy.get()
                        on:click=promote
                    >
                        <Icon icon=LuCopy width="12px" height="12px" />
                        "Use in this scene"
                    </button>
                    <button
                        type="button"
                        class="inline-flex items-center gap-1.5 rounded-md border border-edge-subtle px-2.5 py-1.5 text-[11px] font-medium text-fg-secondary transition-colors btn-press hover:border-status-error/40 hover:text-status-error disabled:cursor-wait disabled:opacity-60"
                        title="Remove the saved face from this screen, in every scene"
                        disabled=move || busy.get()
                        on:click=remove
                    >
                        <Icon icon=LuTrash2 width="12px" height="12px" />
                        "Remove"
                    </button>
                </div>
            </article>
        </Show>
    }
}

/// A saved face's own settings (a clock's hour format, a gauge's sensor).
/// Edits patch the face saved with the screen.
#[component]
fn KeptFaceControls(kept: KeptFace, device_id: String, effect_id: String) -> impl IntoView {
    let detail = api::daemon_resource({
        let effect_id = effect_id.clone();
        move || {
            let effect_id = effect_id.clone();
            async move { api::fetch_effect_detail(&effect_id).await }
        }
    });
    let defs = Signal::derive(move || {
        detail
            .get()
            .and_then(Result::ok)
            .map(|detail| detail.controls)
            .unwrap_or_default()
    });
    let stored = move || {
        kept.face
            .get()
            .filter(|face| face.live_scope == DisplayFaceScope::Default)
            .map(|face| face_controls(&face))
    };
    let (values, set_values) = signal(stored().unwrap_or_default());

    let patch: ControlPatchFn = Arc::new({
        let device_id = device_id.clone();
        move |_target: String,
              payload: ControlValueMap,
              _version: Option<u64>|
              -> ControlPatchFuture {
            let device_id = device_id.clone();
            Box::pin(async move {
                api::update_display_face_controls(&device_id, payload.into_iter().collect())
                    .await?;
                Ok(api::MutationOutcome::Applied(None))
            })
        }
    });
    let session = use_control_patch_session(ControlPatchConfig {
        target: Signal::stored(Some(format!("saved-face:{device_id}:{effect_id}"))),
        defs,
        set_values,
        initial_version: None,
        debounce_ms: FACE_CONTROLS_DEBOUNCE_MS,
        patch,
        on_error: Callback::new(|error: String| {
            toasts::toast_error(&format!("Face settings failed: {error}"));
        }),
        recover: Callback::new(move |()| {
            if let Some(controls) = stored() {
                set_values.set(controls);
            }
            kept.refresh_face.run(());
        }),
        on_committed: None,
        flush_guard: None,
    });
    Effect::new(move |_| {
        if let Some(controls) = stored() {
            session.reconcile_values.run(controls);
        }
    });
    let on_change = session.on_change;

    view! {
        <div class="space-y-2">
            <span class=label_class(LabelSize::Micro, LabelTone::Default)>"Face settings"</span>
            <Show
                when=move || detail.get().is_some()
                fallback=|| view! {
                    <div class="rounded-lg border border-edge-subtle/50 bg-surface-sunken/40 px-3 py-4 text-center text-[11px] text-fg-tertiary">
                        "Loading settings\u{2026}"
                    </div>
                }
            >
                <Show
                    when=move || !defs.with(Vec::is_empty)
                    fallback=|| view! {
                        <div class="text-[11px] text-fg-tertiary">"This face has no settings."</div>
                    }
                >
                    <ControlPanel
                        controls=defs
                        control_values=values
                        accent_rgb=Signal::derive(|| FACE_ACCENT_RGB.to_owned())
                        on_change=on_change
                    />
                </Show>
            </Show>
        </div>
    }
}

/// What "Copy to every scene" found once it had read the saved scenes.
#[derive(Clone, Debug, PartialEq)]
enum CopyPlan {
    /// Still reading.
    Reading,
    /// The scenes that can take the copy, already edited, each with the
    /// layers the edit made, and what the layer lands on, in words.
    Ready {
        scenes: Vec<(api::SceneDocument, Vec<SceneLayerId>)>,
        screens: String,
    },
    /// Nothing can take it, and why.
    Nowhere(String),
    Writing,
}

/// The screens the layer `layer_id` of the zone `zone_id` is on, as
/// another scene would know them, and what they go by together.
fn copy_screens(
    scene: &api::SceneDocument,
    placed: &[PlacedScreen],
    request: &CopyRequest,
) -> Option<(SceneLayer, Vec<ScreenRef>, String)> {
    let zone = scene
        .zones
        .iter()
        .find(|zone| zone.id.to_string() == request.zone_id)?;
    let layer = zone
        .layers
        .iter()
        .find(|layer| layer.id == request.layer_id)?
        .clone();
    let on: Vec<&PlacedScreen> = placed
        .iter()
        .filter(|screen| match &screen.home {
            Some(StackHome::Stored(stored)) => *stored == request.zone_id,
            Some(StackHome::Owner) => {
                screen.zone_id == request.zone_id
                    && layer
                        .scope
                        .as_ref()
                        .is_some_and(|scope| scope.outputs.contains(&screen.output_id))
            }
            None => false,
        })
        .collect();
    // A stored zone holds how its stack meets what is behind it apart
    // from the layer; the copy takes that with it.
    let mut layer = layer;
    if let Some(target) = zone
        .display_target
        .as_ref()
        .filter(|_| zone.view_of.is_none())
    {
        layer.blend = target.blend_mode;
        layer.opacity = target.opacity;
    }
    let refs: Vec<ScreenRef> = on
        .iter()
        .filter_map(|screen| {
            Some(ScreenRef {
                device_id: screen.device_id.clone()?,
                layout_id: screen.layout_id.clone(),
            })
        })
        .collect();
    (!refs.is_empty()).then(|| (layer, refs, screens_label(&on)))
}

/// Read every saved scene that takes copies and put the layer on its
/// screens in each, without writing anything yet.
async fn plan_copies(
    active: api::SceneDocument,
    placed: Vec<PlacedScreen>,
    request: CopyRequest,
) -> CopyPlan {
    let Some((layer, screens, label)) = copy_screens(&active, &placed, &request) else {
        return CopyPlan::Nowhere("That layer is no longer on a screen.".to_owned());
    };
    let summaries = match api::list_scenes().await {
        Ok(summaries) => summaries,
        Err(error) => return CopyPlan::Nowhere(format!("Could not read your scenes: {error}")),
    };
    let active_id = active.id.to_string();
    let mut scenes = Vec::new();
    for summary in summaries
        .iter()
        .filter(|summary| takes_copies(&summary.id, summary.mutation_mode, &active_id))
    {
        let Ok(mut document) = api::fetch_stored_scene(&summary.id).await else {
            continue;
        };
        let known: HashSet<SceneLayerId> = document
            .zones
            .iter()
            .flat_map(|zone| zone.layers.iter().map(|layer| layer.id))
            .collect();
        if copy_layer_to_scene(&mut document.zones, &layer, &screens) > 0 {
            let added = document
                .zones
                .iter()
                .flat_map(|zone| zone.layers.iter().map(|layer| layer.id))
                .filter(|id| !known.contains(id))
                .collect();
            scenes.push((document, added));
        }
    }
    if scenes.is_empty() {
        return CopyPlan::Nowhere(format!("No other scene that can be edited places {label}."));
    }
    CopyPlan::Ready {
        scenes,
        screens: label,
    }
}

/// "Copy to every scene": one confirmation that names the count, then the
/// writes. Mounted once per Studio page.
#[component]
pub(super) fn CopyEverywhereHost() -> impl IntoView {
    let studio = expect_context::<StudioContext>();
    let screens = studio.screens;
    let plan = RwSignal::new(CopyPlan::Reading);
    // Each request reads the scenes afresh; an answer for a request that
    // has since closed is dropped.
    let asked = StoredValue::new(0_u64);
    Effect::new(move |_| {
        let Some(request) = screens.copy.get() else {
            return;
        };
        let Some(active) = studio.history.current_scene() else {
            screens.copy.set(None);
            return;
        };
        plan.set(CopyPlan::Reading);
        asked.update_value(|asked| *asked += 1);
        let token = asked.get_value();
        let placed = screens.placed.get_untracked();
        spawn_local(async move {
            let found = plan_copies(active, placed, request).await;
            if asked.try_get_value() == Some(token) {
                plan.try_set(found);
            }
        });
    });
    let close = move || {
        asked.update_value(|asked| *asked += 1);
        screens.copy.set(None);
    };
    let name = move || {
        screens
            .copy
            .with(|request| request.as_ref().map(|request| request.name.clone()))
            .unwrap_or_default()
    };
    let confirm = move |_| {
        let CopyPlan::Ready { scenes, .. } = plan.get_untracked() else {
            return;
        };
        let name = name();
        plan.set(CopyPlan::Writing);
        spawn_local(async move {
            let mut written = 0_usize;
            let mut refused = None;
            let total = scenes.len();
            for (document, added) in &scenes {
                match api::save_stored_scene(document, added).await {
                    Ok(()) => written += 1,
                    Err(error) => refused = Some(error.to_string()),
                }
            }
            let scenes = |count: usize| {
                if count == 1 {
                    "1 scene".to_owned()
                } else {
                    format!("{count} scenes")
                }
            };
            match refused {
                None => toasts::toast_success(&format!("{name} copied to {}", scenes(total))),
                Some(why) if written == 0 => {
                    toasts::toast_error(&format!("{name} could not be copied: {why}"));
                }
                Some(why) => toasts::toast_error(&format!(
                    "{name} copied to {} of {total}. The rest were refused: {why}",
                    scenes(written),
                )),
            }
            let _ = screens.copy.try_set(None);
        });
    };

    view! {
        <Show when=move || screens.copy.with(Option::is_some)>
            <Modal
                on_close=Callback::new(move |()| close())
                label="Copy to every scene"
                container_class="fixed inset-0 z-50 flex items-center justify-center px-4 animate-enter-fade"
                backdrop_class="absolute inset-0 bg-black/70"
            >
                <div
                    class="relative w-full max-w-sm rounded-xl border border-edge-subtle bg-surface-overlay p-5"
                    on:click=|event| event.stop_propagation()
                    data-copy-everywhere=""
                >
                    <div class="flex items-start gap-3">
                        <span class="mt-0.5 flex shrink-0 text-accent">
                            <Icon icon=LuCopy width="16px" height="16px" />
                        </span>
                        <div class="min-w-0 flex-1">
                            <h2 class="text-sm font-semibold text-fg-primary">
                                {move || match plan.get() {
                                    CopyPlan::Ready { scenes, screens } => {
                                        copy_prompt(&name(), &screens, scenes.len())
                                    }
                                    CopyPlan::Nowhere(_) => "Nothing to copy to".to_owned(),
                                    CopyPlan::Reading | CopyPlan::Writing => {
                                        format!("Copy {} to every scene", name())
                                    }
                                }}
                            </h2>
                            <p class="mt-1.5 text-xs leading-relaxed text-fg-tertiary">
                                {move || match plan.get() {
                                    CopyPlan::Reading => "Looking through your scenes\u{2026}".to_owned(),
                                    CopyPlan::Writing => "Copying\u{2026}".to_owned(),
                                    CopyPlan::Nowhere(why) => why,
                                    CopyPlan::Ready { .. } => {
                                        "It replaces what those screens show there. Each copy is its own afterwards: a change here will not follow. Locked scenes are left alone.".to_owned()
                                    }
                                }}
                            </p>
                        </div>
                    </div>
                    <div class="mt-5 flex justify-end gap-2">
                        <button
                            type="button"
                            class="rounded-md border border-edge-subtle px-3 py-1.5 text-xs font-medium text-fg-secondary transition-colors btn-press hover:bg-surface-hover/40 hover:text-fg-primary"
                            on:click=move |_| close()
                        >
                            {move || if matches!(plan.get(), CopyPlan::Nowhere(_)) { "Close" } else { "Cancel" }}
                        </button>
                        <Show when=move || matches!(plan.get(), CopyPlan::Ready { .. })>
                            <button
                                type="button"
                                class="rounded-md border border-accent-muted bg-accent/10 px-3 py-1.5 text-xs font-semibold text-accent transition-colors btn-press hover:bg-accent/15"
                                on:click=confirm
                            >
                                {move || match plan.get() {
                                    CopyPlan::Ready { scenes, .. } if scenes.len() == 1 => {
                                        "Copy to 1 scene".to_owned()
                                    }
                                    CopyPlan::Ready { scenes, .. } => {
                                        format!("Copy to {} scenes", scenes.len())
                                    }
                                    _ => String::new(),
                                }}
                            </button>
                        </Show>
                    </div>
                </div>
            </Modal>
        </Show>
    }
}

/// What the output properties bar adds for one selected screen: how the
/// panel is mounted, a fact about the output like its place and size, and
/// the way to its full-screen preview. Renders nothing outside Studio or
/// for any other selection.
#[component]
pub(crate) fn OutputScreenFacts() -> impl IntoView {
    let Some(studio) = use_context::<StudioContext>() else {
        return ().into_any();
    };
    let displays = expect_context::<DisplaysContext>().displays_resource;
    let panel = Memo::new(move |_| {
        let device_id = studio
            .screens
            .selected
            .with(|selected| match selected.as_slice() {
                [one] => one.device_id.clone(),
                _ => None,
            })?;
        displays
            .get()
            .and_then(Result::ok)?
            .into_iter()
            .find(|panel| panel.id == device_id)
            .map(|panel| (panel.id, panel.rotation))
    });
    let device_id =
        Signal::derive(move || panel.with(|panel| panel.as_ref().map(|(id, _)| id.clone())));
    let rotation =
        Signal::derive(move || panel.with(|panel| panel.as_ref().map(|(_, mount)| *mount)));
    view! {
        <Show when=move || panel.with(Option::is_some)>
            <div class="flex shrink-0 items-center gap-2" data-screen-facts="">
                <crate::components::mounting_select::MountingSelect
                    device_id=device_id
                    rotation=rotation
                    class="h-7 border border-edge-subtle px-2 text-[11px] text-fg-secondary"
                />
                {move || device_id.get().map(|id| view! { <super::stage::PreviewLink device_id=id /> })}
            </div>
        </Show>
    }
    .into_any()
}

/// Create the layers that put `draft` on `screens`, one zone after the
/// other, and say how it came out. For a page with no journal to record
/// the add in.
fn add_to_screens(
    scene: api::SceneDocument,
    screens: &[PlacedScreen],
    draft: NewLayerDraft,
    done: Callback<()>,
) {
    let targets: Vec<&PlacedScreen> = screens.iter().collect();
    let plans = plan_screen_adds(&scene.zones, &targets);
    if plans.is_empty() {
        toasts::toast_error("There is no screen here that can take a layer yet");
        return;
    }
    let name = draft.name.clone().unwrap_or_else(|| "Layer".to_owned());
    spawn_local(async move {
        let mut fence = api::Fence::of(&scene);
        let mut landed = Vec::new();
        let mut failed = Vec::new();
        for plan in plans {
            if !failed.is_empty() {
                failed.extend(plan.screens);
                continue;
            }
            let mut request =
                add_layer_request(draft.name.clone(), draft.source.clone(), plan.beneath);
            request.scope = plan.scope;
            match api::create_layer(&plan.zone_id, &request, Some(&fence)).await {
                Ok(api::LayerStackOutcome::Applied(stack)) => {
                    fence = fence.at(stack.revision);
                    landed.extend(plan.screens);
                }
                Ok(api::LayerStackOutcome::Stale { .. }) | Err(_) => failed.extend(plan.screens),
            }
        }
        let words = add_outcome_words(&name, &landed, &failed);
        if failed.is_empty() {
            toasts::toast_success(&words);
        } else {
            toasts::toast_error(&words);
        }
        let _ = done.try_run(());
    });
}

/// One screen's layers in a bottom sheet, for a phone: its own layers
/// with a face's settings open in place, and how the panel is mounted.
#[component]
pub(super) fn ScreenSheet(screen: PlacedScreen, on_close: Callback<()>) -> impl IntoView {
    let zones_ctx = expect_context::<crate::zones::ZonesContext>();
    let displays = expect_context::<DisplaysContext>().displays_resource;
    let active_scene: Signal<Option<api::SceneDocument>> = zones_ctx.active_scene.into();
    let placed = use_placed_screens(active_scene);
    let output_id = screen.output_id.clone();
    let zone_id = screen.zone_id.clone();
    // The screen as the scene has it now: a write may change where its
    // layers live.
    let current = Memo::new(move |_| {
        placed.with(|placed| {
            placed
                .iter()
                .find(|placed| placed.zone_id == zone_id && placed.output_id == output_id)
                .cloned()
        })
    });
    let view = Memo::new(move |_| {
        let screen = current.get()?;
        active_scene.with(|scene| {
            let zones = &scene.as_ref()?.zones;
            let panel = screen_panel(zones, &[&screen])?;
            let mut view = screen_view(zones, &panel, std::slice::from_ref(&screen));
            // A phone has no journal for a copy to go through, and no
            // canvas to choose other screens on.
            for row in &mut view.rows {
                row.copies = false;
                row.targets = false;
            }
            Some(view)
        })
    });
    let device_id =
        Signal::derive(move || current.with(|screen| screen.as_ref()?.device_id.clone()));
    let (face_tick, set_face_tick) = signal(0_u64);
    let (face, kept_row) = use_display_face(device_id, face_tick);
    let refresh = zones_ctx.refresh;
    let kept = KeptFace {
        device_id,
        face,
        row: kept_row.into(),
        refresh_face: Callback::new(move |()| {
            set_face_tick.update(|tick| *tick = tick.wrapping_add(1));
        }),
        refresh_scene: refresh,
    };
    let host = ScreenHost {
        view: view.into(),
        add: Callback::new(move |(draft, _everywhere): (NewLayerDraft, bool)| {
            let (Some(scene), Some(screen)) =
                (active_scene.get_untracked(), current.get_untracked())
            else {
                return;
            };
            add_to_screens(scene, &[screen], draft, refresh);
        }),
        all_screens: Signal::stored(None),
        leave: on_close,
        copy: Callback::new(|_| {}),
        lead: ViewFn::from(move || view! { <KeptFaceCard kept=kept /> }),
        faces: Signal::stored(Vec::new()),
        select: Callback::new(|_| {}),
        add_requests: Signal::stored(0),
        replaced: Callback::new(|_| {}),
    };

    let (owner, set_owner) = signal(Some(screen.zone_id.clone()));
    let layers = LocalResource::new(move || {
        let zone_id = owner.get();
        let stack = active_scene.with(|scene| {
            scene
                .as_ref()
                .zip(zone_id.as_deref())
                .and_then(|(scene, zone_id)| {
                    scene
                        .zones
                        .iter()
                        .find(|zone| zone.id.to_string() == zone_id)
                        .map(|zone| api::LayerStackResponse {
                            items: zone.layers.clone(),
                            revision: scene.revision,
                        })
                })
                .unwrap_or(api::LayerStackResponse {
                    items: Vec::new(),
                    revision: 0,
                })
        });
        async move { Ok(stack) }
    });
    let rotation = Signal::derive(move || {
        let device_id = device_id.get()?;
        displays
            .get()
            .and_then(Result::ok)?
            .into_iter()
            .find(|panel| panel.id == device_id)
            .map(|panel| panel.rotation)
    });
    let name = screen.name.clone();
    let title = name.clone();
    let host = StoredValue::new(host);

    view! {
        <crate::components::bottom_sheet::BottomSheet on_close=on_close title=title>
            <div class="px-4 pb-4" data-screen-sheet="">
                <div class="flex items-center justify-between gap-3 pb-1">
                    <span class=label_class(LabelSize::Micro, LabelTone::Default)>"Mounting"</span>
                    <crate::components::mounting_select::MountingSelect
                        device_id=device_id
                        rotation=rotation
                        class="min-h-11 border border-edge-subtle px-3 text-xs text-fg-secondary"
                    />
                </div>
                <crate::components::layer_panel::LayerPanel
                    active_scene=active_scene
                    selected_zone_id=owner
                    set_selected_zone_id=set_owner
                    surface_label=name.clone()
                    layers_resource=layers
                    on_layers_mutated=refresh
                    screens=host.get_value()
                />
            </div>
        </crate::components::bottom_sheet::BottomSheet>
    }
}
