//! The layer-stack editor — one reusable component for every surface.
//!
//! # Contract (Spec 65 §10)
//!
//! `LayerPanel` edits the layer stack of exactly one zone. Studio,
//! `/assets`, and any future surface mount the same component, so the layer
//! manager cannot drift — there is exactly one implementation.
//!
//! The mount surface pins:
//!
//! - **Surface identity:** `active_scene` + `selected_zone_id` name the
//!   `(scene id, zone id)` pair every mutation is addressed to. The panel
//!   never displays the ids.
//! - **Scene fence:** the stack's revision from `layers_resource`, in the
//!   daemon run of the active scene; threaded as the `If-Match`
//!   precondition on every mutation. A stale write is reported and the
//!   stack refetched, never silently lost.
//! - **Add-layer picker** — Add-layer opens [`picker::AddLayerPicker`],
//!   covering effects/faces plus media assets.
//! - **One mutation callback** — `on_layers_mutated: Callback<()>` fires
//!   after every applied or rejected mutation; the host refetches the
//!   stack (and active scene) in response. There is exactly one.
//! - **Transform / adjust sub-panels** — each [`row::LayerRow`] carries the
//!   collapsed transform + color-adjust disclosure.
//! - **Targets:** a host that can say which outputs the zone holds, and
//!   show them, passes a [`LayerTargetHost`]. Rows then say what each
//!   layer applies to, and a new layer takes the host's target. Without
//!   one the panel shows no target at all.
//! - **Screens:** a layer limited to screens is a screen's own: it draws
//!   on the panel, above everything the zone's canvas puts behind it. The
//!   zone's list leaves such layers out and names each on one line that
//!   leads to its screen. A host that shows screens passes a
//!   [`ScreenHost`]; while screens are all it has selected, the panel
//!   lists their own layers in place of the zone's, then what is behind.
//!
//! Content selection is owned internally (its own asset resource), so the
//! component is decoupled from any host page's selection state.

mod controls;
mod picker;
pub mod row;
pub mod scope;
pub mod source;
pub mod target_checklist;

use std::collections::{HashMap, HashSet};

use hypercolor_types::event::LayerHealth;
use hypercolor_types::layer::{SceneLayer, SceneLayerId};
use hypercolor_types::scene::ZoneRole;
use leptos::prelude::*;
use leptos_icons::Icon;

use crate::api;
use crate::app::{CapabilitiesContext, WsContext};
use crate::components::section_label::{LabelSize, LabelTone, label_class};
use crate::components::silk_select::SilkSelect;
use crate::icons::*;
use crate::toasts;
use crate::ws::messages::layer_health_key;
use crate::zones::surface::zone_display_name;

use picker::AddLayerPicker;
pub use picker::NewLayerDraft;
use row::LayerRow;
pub use row::{Backdrop, DisplayComposition};
use scope::{
    NoCreateTarget, Target, TargetOutput, WHOLE_ZONE, create_scope, draft_words, is_screen_layer,
    is_screen_scope, offers_targets, reorder_within,
};
use source::{add_layer_request, effect_picker_mode};

/// Effect schemas survive layer replacements; a reconnect invalidates their epoch.
pub type LayerEffectCache = StoredValue<HashMap<String, (u64, api::effects::EffectDetailResponse)>>;

/// Whose target a target editor edits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetSubject {
    /// The layer the panel adds next.
    NewLayer,
    /// A layer in the stack, with the name its row shows.
    Layer { id: SceneLayerId, name: String },
}

/// What a host that shows a zone's outputs lends the panel, so layers can
/// say which of them they apply to.
#[derive(Clone, Copy)]
pub struct LayerTargetHost {
    /// The zone's outputs, as targets name them.
    pub outputs: Signal<Vec<TargetOutput>>,
    /// The outputs whose layers stand in front: the host's selection, or
    /// none while it puts nothing in front.
    pub emphasis: Signal<HashSet<String>>,
    /// The output under the pointer on the host's canvas.
    pub pointer: Signal<Option<String>>,
    /// Light these outputs on the host's canvas; none to stop.
    pub hover: Callback<HashSet<String>>,
    /// What the next added layer applies to.
    pub add_target: Signal<Target>,
    /// Open the target editor on a subject, anchored to the element that
    /// asked.
    pub edit: Callback<(TargetSubject, web_sys::Element)>,
    /// Moves each time the host asks for the Add layer picker.
    pub add_requests: Signal<u64>,
    /// A layer was added, applying to this target.
    pub added: Callback<Target>,
    /// A replace minted a new id: `(zone id, old, new)`.
    pub replaced: Callback<(String, SceneLayerId, SceneLayerId)>,
    /// The host has nowhere to choose outputs: rows say what a layer
    /// applies to and offer no way to change it.
    pub fixed: bool,
}

/// How a row's writes report back.
#[derive(Clone, Copy)]
pub struct LayerWrites {
    /// Fires after every applied or rejected mutation.
    pub mutated: Callback<()>,
    /// The zone's stack as last read: a replace keeps its order, which is
    /// how the layer it made is told from the rest.
    pub stack: Signal<Vec<SceneLayer>>,
    /// Hears the id a replace minted: `(zone id, old, new)`.
    pub replaced: Callback<(String, SceneLayerId, SceneLayerId)>,
    /// The daemon's capabilities, which decide whether a replace names the
    /// layer's scope. A mount with none sends what every daemon takes.
    pub capabilities: Option<CapabilitiesContext>,
}

impl LayerWrites {
    /// Whether a replace sent now names the layer's scope.
    fn scope_field(&self) -> api::ScopeField {
        self.capabilities
            .map_or(api::ScopeField::Omitted, |caps| caps.layer_scope_field())
    }
}

/// One layer of a screen's own stack, as the panel lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct ScreenViewRow {
    /// The zone whose stack holds the layer: the zone that places the
    /// screen, or a stored zone of the screen's own.
    pub zone_id: String,
    pub layer_id: SceneLayerId,
    /// Set for the bottom layer that draws.
    pub backdrop: Option<Backdrop>,
    /// The zones the layer's health is reported under.
    pub health_zones: Vec<String>,
    /// Whether the row says which screens the layer is on, and lets them
    /// be changed.
    pub targets: bool,
    /// Whether the layer can be copied to every scene.
    pub copies: bool,
}

/// A selection of screens, as the panel shows it in place of the zone's
/// list: the screens' own layers, then what the zone puts behind them.
#[derive(Debug, Clone, PartialEq)]
pub struct ScreenView {
    /// "On this screen".
    pub heading: String,
    /// The screens' own layers, top first.
    pub rows: Vec<ScreenViewRow>,
    /// "Behind it, from Main".
    pub behind_heading: String,
    /// The zone's layers that reach the screens from behind, top first.
    pub behind: Vec<SceneLayerId>,
    /// The screens, in words, for the picker: "TL LCD 1".
    pub target: String,
    /// "Add layer to this screen".
    pub add_label: String,
    /// Why nothing can be added, when nothing can.
    pub no_add: Option<String>,
}

/// One face among a zone's screens, for the zone's own list.
#[derive(Debug, Clone, PartialEq)]
pub struct ScreenFaceLine {
    pub layer_id: SceneLayerId,
    /// The screens it is on, in words.
    pub screens: String,
    /// Those screens' outputs, which selecting the line selects.
    pub outputs: Vec<String>,
}

/// What a host that shows screens lends the panel.
#[derive(Clone)]
pub struct ScreenHost {
    /// The screens selected, when screens are all that is selected.
    pub view: Signal<Option<ScreenView>>,
    /// Add a picked layer to the screens shown, or to every screen of the
    /// scene for `true`.
    pub add: Callback<(NewLayerDraft, bool)>,
    /// What adding to every screen would reach, in words, when the scene
    /// has screens beyond the ones shown.
    pub all_screens: Signal<Option<String>>,
    /// Leave the screens for the zone's own list.
    pub leave: Callback<()>,
    /// Copy a layer to every scene: its zone, id, and name.
    pub copy: Callback<(String, SceneLayerId, String)>,
    /// What leads the screens' own layers: a face saved with the screen.
    pub lead: ViewFn,
    /// The zone's faces, for the line each gets in the zone's own list.
    pub faces: Signal<Vec<ScreenFaceLine>>,
    /// Select these outputs on the host's canvas.
    pub select: Callback<Vec<String>>,
    /// Moves each time the host asks for the Add layer picker on the
    /// screens shown.
    pub add_requests: Signal<u64>,
    /// A replace minted a new id: `(zone id, old, new)`.
    pub replaced: Callback<(String, SceneLayerId, SceneLayerId)>,
}

/// A row to mount: the zone whose stack holds its layer, and what keeps
/// it mounted.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct RowKey {
    scene_id: String,
    zone_id: String,
    mount: (String, String),
    /// One of a screen's own layers.
    on_screen: bool,
}

/// Layer-stack editor for one zone. See the module docs for the
/// mount contract.
#[component]
pub fn LayerPanel(
    #[prop(into)] active_scene: Signal<Option<api::SceneDocument>>,
    selected_zone_id: ReadSignal<Option<String>>,
    set_selected_zone_id: WriteSignal<Option<String>>,
    /// Surface name supplied by a host that owns surface selection
    /// elsewhere (Studio's zone tabs). When present the panel shows it
    /// in the header and drops its own redundant zone selector.
    #[prop(optional, into)]
    surface_label: MaybeProp<String>,
    layers_resource: LocalResource<api::ApiResult<api::LayerStackResponse>>,
    on_layers_mutated: Callback<()>,
    /// The host's outputs and canvas, when it can show what a layer
    /// applies to.
    #[prop(optional, into)]
    targets: Signal<Option<LayerTargetHost>>,
    /// The host's screens, when it shows them.
    #[prop(optional)]
    screens: Option<ScreenHost>,
) -> impl IntoView {
    let effect_cache = LayerEffectCache::new(HashMap::new());
    // Disclosure preference belongs to a visible stack slot, independently of
    // the fresh layer authority minted by a whole-layer replacement.
    let disclosures = StoredValue::new(HashMap::<(String, String, usize, u8), bool>::new());
    // Content selection is owned here, not driven by the host page — the
    // asset list backs both media-name resolution and the picker's Media tab.
    let assets_resource = api::daemon_resource(|| async { api::list_assets().await });
    let assets = Signal::derive(move || {
        assets_resource
            .get()
            .and_then(Result::ok)
            .map(|response| response.items)
            .unwrap_or_default()
    });
    let media_names = Memo::new(move |_| {
        assets
            .get()
            .into_iter()
            .map(|asset| (asset.id.to_string(), asset.name))
            .collect::<HashMap<String, String>>()
    });
    // Effect ids on a layer are UUIDs; resolve them to registry names so
    // a layer row reads "Effect Aurora", never "Effect <uuid>".
    let effects_resource = api::daemon_resource(api::fetch_effects);
    let effect_names = Memo::new(move |_| {
        effects_resource
            .get()
            .and_then(Result::ok)
            .map(|effects| {
                effects
                    .into_iter()
                    .map(|effect| (effect.id, effect.name))
                    .collect::<HashMap<String, String>>()
            })
            .unwrap_or_default()
    });
    let scene_revision = Signal::derive(move || {
        layers_resource
            .get()
            .and_then(Result::ok)
            .map(|stack| stack.revision)
    });

    let layers = Memo::new(move |_| {
        layers_resource
            .get()
            .and_then(Result::ok)
            .map(|stack| stack.items)
            .unwrap_or_default()
    });
    let scene_id = Memo::new(move |_| {
        active_scene
            .get()
            .map(|scene| scene.id.to_string())
            .unwrap_or_default()
    });
    // The daemon run the stack's revision belongs to. A stack and a scene
    // read in different runs make a fence the daemon refuses, never one it
    // takes wrongly.
    let run = Signal::derive(move || {
        active_scene.with(|scene| {
            scene
                .as_ref()
                .map(|scene| scene.run_epoch.clone())
                .unwrap_or_default()
        })
    });
    let fence = Signal::derive(move || api::Fence {
        revision: scene_revision.get().unwrap_or_default(),
        run: run.get(),
    });

    // The screens selected, when the host shows any: the panel lists their
    // own layers in place of the zone's.
    let screen_view = screens.as_ref().map(|host| host.view);
    let view_now = Memo::new(move |_| screen_view.and_then(|view| view.get()));
    let on_screens = Memo::new(move |_| view_now.with(Option::is_some));
    // The stack of any zone of the scene: the selected zone's as last
    // read, another's (a screen's stored zone) as the scene has it.
    let stack_of = move |zone_id: &str| -> Vec<SceneLayer> {
        if selected_zone_id.with(|selected| selected.as_deref() == Some(zone_id)) {
            return layers.get();
        }
        active_scene.with(|scene| {
            scene
                .as_ref()
                .and_then(|scene| {
                    scene
                        .zones
                        .iter()
                        .find(|zone| zone.id.to_string() == zone_id)
                })
                .map(|zone| zone.layers.clone())
                .unwrap_or_default()
        })
    };
    // Which of the selected zone's layers are a screen's own. They are not
    // part of the zone's canvas, so its list leaves them to their screens.
    let screen_layer_ids = Memo::new(move |_| {
        let selected = selected_zone_id.get();
        let stack = layers.get();
        active_scene.with(|scene| {
            scene
                .as_ref()
                .zip(selected.as_ref())
                .and_then(|(scene, selected)| {
                    scene
                        .zones
                        .iter()
                        .find(|zone| zone.id.to_string() == *selected)
                })
                .map(|zone| {
                    stack
                        .iter()
                        .filter(|layer| is_screen_layer(zone, layer))
                        .map(|layer| layer.id)
                        .collect::<HashSet<_>>()
                })
                .unwrap_or_default()
        })
    });
    // The zone's own list: its canvas layers, bottom first.
    let canvas_layers = Memo::new(move |_| {
        let on_screen = screen_layer_ids.get();
        layers
            .get()
            .into_iter()
            .filter(|layer| !on_screen.contains(&layer.id))
            .collect::<Vec<_>>()
    });
    let row_keys = Memo::new(move |_| {
        let scene = scene_id.get();
        if let Some(view) = view_now.get() {
            return view
                .rows
                .iter()
                .filter_map(|row| {
                    let layer = stack_of(&row.zone_id)
                        .into_iter()
                        .find(|layer| layer.id == row.layer_id)?;
                    Some(RowKey {
                        scene_id: scene.clone(),
                        zone_id: row.zone_id.clone(),
                        mount: layer_mount_key(&layer),
                        on_screen: true,
                    })
                })
                .collect::<Vec<_>>();
        }
        let zone = selected_zone_id.get().unwrap_or_default();
        canvas_layers
            .get()
            .iter()
            .rev()
            .map(|layer| RowKey {
                scene_id: scene.clone(),
                zone_id: zone.clone(),
                mount: layer_mount_key(layer),
                on_screen: false,
            })
            .collect::<Vec<_>>()
    });

    // Per-layer runtime health streams in over the WebSocket, independent
    // of the layer stack itself; an absent context means no health yet.
    let ws = use_context::<WsContext>();
    let layer_health =
        Signal::derive(move || ws.map(|ws| ws.layer_health.get()).unwrap_or_default());

    let (show_picker, set_show_picker) = signal(false);

    let zone_options = Signal::derive(move || {
        active_scene
            .get()
            .map(|scene| {
                scene
                    .zones
                    .into_iter()
                    .filter(|zone| zone.role != ZoneRole::Display)
                    .map(|zone| {
                        let name = zone_display_name(&zone.name, zone.role);
                        (zone.id.to_string(), name)
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    });

    // What the next added layer applies to, with the zone's outputs as
    // they stand. The host's target is taken as it is: a zone that has
    // fallen to one output still holds a target somebody chose.
    let target_host = targets;
    let add_target: Memo<Option<(Target, Vec<TargetOutput>)>> = Memo::new(move |_| {
        let host = target_host.get()?;
        Some((host.add_target.get(), host.outputs.get()))
    });
    // The line under "Add layer" shows where there is something to choose
    // between, and wherever a target is already chosen.
    let add_target_words = Memo::new(move |_| {
        add_target.with(|target| {
            target
                .as_ref()
                .filter(|(target, outputs)| offers_targets(outputs) || !target.is_whole_zone())
                .map(|(target, outputs)| draft_words(target, outputs))
        })
    });
    // The chosen target's outputs have all left the zone.
    let add_target_departed = Memo::new(move |_| {
        add_target.with(|target| {
            target.as_ref().is_some_and(|(target, outputs)| {
                create_scope(target, outputs) == Err(NoCreateTarget::Departed)
            })
        })
    });
    // The target the open picker adds to: the one shown when it opened. A
    // selection that moves on, or outputs that leave, while it is open
    // must not turn the add into one for something else.
    let picker_target = RwSignal::new(None::<Target>);
    // The picker was opened on the screens shown, and adds through the
    // host that shows them.
    let picker_on_screens = RwSignal::new(false);
    let open_picker = move || {
        let on_screens = on_screens.get_untracked();
        let target = (!on_screens).then(|| add_target.get_untracked()).flatten();
        if let Some((target, outputs)) = &target
            && let Err(stopped) = create_scope(target, outputs)
        {
            toasts::toast_error(stopped.message());
            return;
        }
        if on_screens
            && let Some(reason) =
                view_now.with_untracked(|view| view.as_ref().and_then(|view| view.no_add.clone()))
        {
            toasts::toast_error(&reason);
            return;
        }
        // The asset list is decoupled from any host page's refresh tick,
        // so refresh it on demand, or a file uploaded since mount is
        // missing from the picker.
        assets_resource.refetch();
        picker_target.set(target.map(|(target, _)| target));
        picker_on_screens.set(on_screens);
        set_show_picker.set(true);
    };
    // The host's "+ Layer" asks for the picker; the count is only read to
    // notice that it moved.
    Effect::new(move |seen: Option<Option<u64>>| {
        let asked = target_host.get().map(|host| host.add_requests.get());
        if let Some(Some(before)) = seen
            && asked.is_some_and(|now| now != before)
        {
            open_picker();
        }
        asked
    });
    if let Some(asked) = screens.as_ref().map(|host| host.add_requests) {
        Effect::new(move |seen: Option<u64>| {
            let now = asked.get();
            if seen.is_some_and(|before| before != now) {
                open_picker();
            }
            now
        });
    }
    // Outputs that leave while the picker is open take its target with
    // them: the picker closes and says why, rather than go on offering
    // content for nothing. So does a selection of screens that ends.
    Effect::new(move |_| {
        if !show_picker.get() {
            return;
        }
        if picker_on_screens.get() {
            if !on_screens.get() {
                set_show_picker.set(false);
            }
            return;
        }
        let stopped = picker_target.with(|target| {
            let target = target.as_ref()?;
            let outputs = target_host.get()?.outputs.get();
            create_scope(target, &outputs).err()
        });
        if let Some(stopped) = stopped {
            set_show_picker.set(false);
            toasts::toast_error(stopped.message());
        }
    });
    // Faces for a layer that screens alone will play, effects otherwise.
    let picker_mode = Signal::derive(move || {
        if picker_on_screens.get() {
            return effect_picker_mode(true);
        }
        let screens_only = picker_target.with(|target| {
            let scope = target.as_ref().and_then(Target::scope);
            target_host.get().is_some_and(|host| {
                host.outputs
                    .with(|outputs| is_screen_scope(scope.as_ref(), outputs))
            })
        });
        effect_picker_mode(screens_only)
    });

    let screen_add = screens.as_ref().map(|host| host.add);
    let add_layer = Callback::new(move |(draft, everywhere): (NewLayerDraft, bool)| {
        set_show_picker.set(false);
        if picker_on_screens.get_untracked() {
            if let Some(add) = screen_add {
                add.run((draft, everywhere));
            }
            return;
        }
        if active_scene.with_untracked(Option::is_none) {
            toasts::toast_error("No active scene is available");
            return;
        }
        let Some(zone_id) = selected_zone_id.get_untracked() else {
            toasts::toast_error("No zone is selected");
            return;
        };
        let expected_revision = scene_revision.get_untracked();
        let run = run.get_untracked();
        // A new layer blends over the canvas layers under it; a screen's
        // own layers are not among them.
        let mut request = add_layer_request(
            draft.name,
            draft.source,
            canvas_layers.with_untracked(Vec::len),
        );
        let target = picker_target.get_untracked();
        let host = target_host.get_untracked();
        let mut added_to = None;
        let mut added_target = Target::WholeZone;
        if let (Some(target), Some(host)) = (target, host) {
            let outputs = host.outputs.get_untracked();
            // A target whose outputs left since it was chosen stops the
            // add. It never widens to the whole zone.
            match create_scope(&target, &outputs) {
                Ok(None) => {}
                Ok(Some(scope)) => {
                    // The first layer of a screen's stack meets what is
                    // behind the screen, as a face does: over it.
                    if is_screen_scope(Some(&scope), &outputs) {
                        request.blend = Some(hypercolor_types::layer::BlendMode::Alpha);
                    }
                    added_target = Target::of(Some(&scope));
                    added_to = Some(draft_words(&added_target, &outputs));
                    request.scope = Some(scope);
                }
                Err(stopped) => {
                    toasts::toast_error(stopped.message());
                    return;
                }
            }
        }
        leptos::task::spawn_local(async move {
            let fence = expected_revision.map(|revision| api::Fence { revision, run });
            let applied = matches!(
                api::create_layer(&zone_id, &request, fence.as_ref()).await,
                Ok(api::LayerStackOutcome::Applied(_))
            );
            on_layers_mutated.run(());
            if !applied {
                toasts::toast_error("Layer add failed");
                return;
            }
            if let Some(host) = host {
                host.added.run(added_target);
            }
            toasts::toast_success(&match added_to {
                Some(target) => format!("Layer added to {target}"),
                None => "Layer added".to_owned(),
            });
        });
    });

    let capabilities = use_context::<CapabilitiesContext>();
    let screen_replaced = screens.as_ref().map(|host| host.replaced);
    let replaced = Callback::new(move |minted| {
        if let Some(host) = targets.get_untracked() {
            host.replaced.run(minted);
        } else if let Some(replaced) = screen_replaced {
            replaced.run(minted);
        }
    });
    let screens = StoredValue::new_local(screens);
    let screen_faces = Signal::derive(move || {
        screens
            .with_value(|host| host.as_ref().map(|host| host.faces))
            .map(|faces| faces.get())
            .unwrap_or_default()
    });
    let title_of = move |layer: &SceneLayer| {
        let (_, _, kind_word) = row::source_meta(&layer.source);
        row::layer_title(layer, &media_names.get(), &effect_names.get(), kind_word)
    };

    view! {
        <section class="space-y-4 pt-4">
            <Show when=move || surface_label.get().is_none()>
                <SilkSelect
                    value=Signal::derive(move || selected_zone_id.get().unwrap_or_default())
                    options=zone_options
                    on_change=Callback::new(move |id: String| {
                        set_selected_zone_id.set((!id.is_empty()).then_some(id));
                    })
                    aria_label="Zone"
                    placeholder="Select zone"
                    class="border border-edge-subtle bg-surface-sunken/55 px-3 py-2 text-xs text-fg-primary"
                    label_class="font-medium"
                />
            </Show>

            // The zone's own list leads with Add layer; a selection of
            // screens leads with their layers and adds beneath them.
            <Show when=move || !on_screens.get()>
                <div>
                    <button
                        type="button"
                        class="inline-flex w-full items-center justify-center gap-1.5 rounded-lg border border-accent-muted/30 bg-accent/10 px-3 py-2 text-xs font-semibold text-accent transition-colors hover:bg-accent/15 btn-press disabled:cursor-not-allowed disabled:opacity-45"
                        disabled=move || selected_zone_id.get().is_none()
                        on:click=move |_| open_picker()
                    >
                        <Icon icon=LuPlus width="13px" height="13px" />
                        "Add layer"
                    </button>
                    // What the new layer will apply to, and the way to change it.
                    <Show when=move || add_target_words.with(Option::is_some)>
                        <div class="mt-1.5 flex justify-center">
                            <button
                                type="button"
                                class="group inline-flex max-w-full items-center gap-1.5 rounded-md px-2 py-1 text-[11px] text-fg-tertiary transition-colors hover:bg-surface-hover/40 hover:text-fg-secondary"
                                aria-haspopup="dialog"
                                aria-label=move || {
                                    if add_target_departed.get() {
                                        "The new layer's outputs are not in this zone".to_owned()
                                    } else {
                                        format!(
                                            "New layer applies to {}",
                                            add_target_words.get().unwrap_or_default(),
                                        )
                                    }
                                }
                                title=move || {
                                    if add_target_departed.get() {
                                        "The outputs chosen for the new layer have left this zone. Choose what it applies to."
                                    } else {
                                        "Choose what the new layer applies to"
                                    }
                                }
                                on:click=move |ev: web_sys::MouseEvent| {
                                    if let (Some(host), Some(element)) =
                                        (target_host.get_untracked(), crate::pointer_gesture::listener_element(&ev))
                                    {
                                        host.edit.run((TargetSubject::NewLayer, element));
                                    }
                                }
                            >
                                <Show when=move || !add_target_departed.get()>
                                    <span>"to"</span>
                                </Show>
                                <span
                                    class="min-w-0 truncate font-medium"
                                    class=("text-fg-secondary", move || {
                                        add_target_words.with(|words| words.as_deref() == Some(WHOLE_ZONE))
                                    })
                                    class=("text-fg-primary", move || {
                                        !add_target_departed.get()
                                            && add_target_words.with(|words| words.as_deref() != Some(WHOLE_ZONE))
                                    })
                                    class=("text-status-warning", move || add_target_departed.get())
                                >
                                    {move || add_target_words.get().unwrap_or_default()}
                                </span>
                                <Icon icon=LuChevronDown width="11px" height="11px" />
                            </button>
                        </div>
                    </Show>
                </div>
            </Show>

            // The zone's own list leads with its screens' faces, one line
            // each, which lead to the screen: they play above everything
            // listed under them.
            <Show when=move || !on_screens.get() && !screen_faces.with(Vec::is_empty)>
                <div class="space-y-1.5">
                    <span class=label_class(LabelSize::Small, LabelTone::Default)>"On screens"</span>
                    {move || {
                        let stack = layers.get();
                        screen_faces
                            .get()
                            .into_iter()
                            .filter_map(|face| {
                                let layer = stack.iter().find(|layer| layer.id == face.layer_id)?;
                                let title = title_of(layer);
                                let label = format!("{title} on {}", face.screens);
                                let outputs = face.outputs.clone();
                                let select = screens.with_value(|host| host.as_ref().map(|host| host.select));
                                Some(view! {
                                    <button
                                        type="button"
                                        class="group flex w-full items-center gap-2.5 rounded-lg border border-edge-subtle/60 bg-surface-sunken/40 px-3 py-2 text-left transition-colors hover:border-accent-muted"
                                        title="Open this screen's layers"
                                        aria-label=label
                                        data-screen-face=""
                                        on:click=move |_| {
                                            if let Some(select) = select {
                                                select.run(outputs.clone());
                                            }
                                        }
                                    >
                                        <span class="flex shrink-0 text-accent/75">
                                            <Icon icon=LuMonitor width="13px" height="13px" />
                                        </span>
                                        <span class="min-w-0 flex-1 truncate text-xs font-medium text-fg-primary">{title}</span>
                                        <span class="min-w-0 max-w-[45%] shrink truncate text-[10px] text-fg-tertiary">{face.screens}</span>
                                        <span class="flex shrink-0 text-fg-tertiary transition-colors group-hover:text-fg-secondary">
                                            <Icon icon=LuChevronRight width="12px" height="12px" />
                                        </span>
                                    </button>
                                })
                            })
                            .collect_view()
                    }}
                </div>
            </Show>

            <Show when=move || layers_resource.get().is_none()>
                <LayerLoadingSkeleton />
            </Show>
            {move || layers_resource.get().and_then(Result::err).map(|error| view! {
                <div class="rounded-lg border border-status-error/30 bg-status-error/10 px-3 py-3 text-xs text-status-error">{error.to_string()}</div>
            })}
            <Show when=move || {
                !on_screens.get()
                    && layers_resource.get().is_some_and(|result| result.is_ok())
                    && canvas_layers.with(Vec::is_empty)
            }>
                <div class="rounded-lg border border-edge-subtle bg-surface-sunken/45 px-3 py-8 text-center text-xs text-fg-tertiary">
                    "No layers in this zone"
                </div>
            </Show>

            // A selection of screens: what leads their layers.
            <Show when=move || on_screens.get()>
                <div class="flex items-center justify-between gap-2">
                    <span class=label_class(LabelSize::Small, LabelTone::Default) data-screen-heading="">
                        {move || view_now.with(|view| view.as_ref().map(|view| view.heading.clone()))}
                    </span>
                </div>
                {move || screens.with_value(|host| host.as_ref().map(|host| host.lead.run()))}
            </Show>

            <div class="space-y-2">
                <Show when=move || !on_screens.get() && canvas_layers.with(|layers| layers.len() > 1)>
                    <div class="text-[10px] font-mono uppercase tracking-wide text-fg-tertiary/65">"Top"</div>
                </Show>
                <For
                    each=move || row_keys.get()
                    key=|key| key.clone()
                    children=move |RowKey { scene_id, zone_id, mount: (layer_id, _), on_screen }| {
                        let row_zone = StoredValue::new(zone_id.clone());
                        let stack = Signal::derive(move || row_zone.with_value(|zone| stack_of(zone)));
                        let Some(initial) = stack.with_untracked(|layers| {
                            layers.iter().find(|layer| layer.id.to_string() == layer_id).cloned()
                        }) else {
                            return ().into_any();
                        };
                        let lookup_id = layer_id.clone();
                        let current_layer = Memo::new(move |_| {
                            stack.with(|layers| layers.iter().find(|layer| layer.id.to_string() == lookup_id).cloned()).unwrap_or_else(|| initial.clone())
                        });
                        let stack_index = Signal::derive(move || stack.with(|layers| layers.iter().position(|layer| layer.id.to_string() == layer_id).unwrap_or_default()));
                        // A row's two disclosures, each remembered by its slot:
                        // transform and color, and a screen row's blend.
                        let slot = (scene_id.clone(), zone_id.clone());
                        let disclosure = move |which: u8| {
                            let key = (slot.0.clone(), slot.1.clone(), stack_index.get_untracked(), which);
                            let open = disclosures.with_value(|states| states.get(&key).copied().unwrap_or(false));
                            let report = Callback::new(move |open| {
                                let mut key = key.clone();
                                key.2 = stack_index.get_untracked();
                                disclosures.update_value(|states| { states.insert(key, open); });
                            });
                            (open, report)
                        };
                        let (expanded, on_disclosure) = disclosure(0);
                        let (blend_open, on_blend_disclosure) = disclosure(1);
                        // This row among the screens' rows, while screens
                        // are what is shown.
                        let view_row = Memo::new(move |_| {
                            let id = current_layer.with(|layer| layer.id);
                            view_now.with(|view| {
                                view.as_ref().and_then(|view| {
                                    view.rows
                                        .iter()
                                        .find(|row| row.layer_id == id && row_zone.with_value(|zone| row.zone_id == *zone))
                                        .cloned()
                                })
                            })
                        });
                        // A layer on several screens is as well as its
                        // least well instance.
                        let row_scene = scene_id.clone();
                        let row_health = Signal::derive(move || {
                            let layer_id = current_layer.with(|layer| layer.id.to_string());
                            let zones = view_row.with(|row| {
                                row.as_ref().map(|row| row.health_zones.clone())
                            }).unwrap_or_else(|| vec![row_zone.get_value()]);
                            layer_health.with(|map| {
                                zones
                                    .iter()
                                    .filter_map(|zone| map.get(&layer_health_key(&row_scene, zone, &layer_id)).cloned())
                                    .max_by_key(health_rank)
                            })
                        });
                        let row_targets = Signal::derive(move || {
                            targets.get().filter(|_| {
                                !on_screen || view_row.with(|row| row.as_ref().is_some_and(|row| row.targets))
                            })
                        });
                        let group = Signal::derive(move || {
                            if on_screen {
                                view_now.with(|view| {
                                    view.as_ref().map(|view| {
                                        view.rows
                                            .iter()
                                            .rev()
                                            .filter(|row| row_zone.with_value(|zone| row.zone_id == *zone))
                                            .map(|row| row.layer_id)
                                            .collect::<Vec<_>>()
                                    })
                                }).unwrap_or_default()
                            } else {
                                canvas_layers.with(|layers| layers.iter().map(|layer| layer.id).collect())
                            }
                        });
                        let backdrop = Signal::derive(move || {
                            view_row.with(|row| row.as_ref().and_then(|row| row.backdrop.clone()))
                        });
                        let writes = LayerWrites {
                            mutated: on_layers_mutated,
                            stack,
                            replaced,
                            capabilities,
                        };
                        let copies = on_screen
                            && view_row.with_untracked(|row| row.as_ref().is_some_and(|row| row.copies));
                        let on_copy = copies
                            .then(|| screens.with_value(|host| host.as_ref().map(|host| host.copy)))
                            .flatten()
                            .map(|copy| {
                                Callback::new(move |name: String| {
                                    copy.run((
                                        row_zone.get_value(),
                                        current_layer.with_untracked(|layer| layer.id),
                                        name,
                                    ));
                                })
                            });
                        view! {
                            <LayerRow
                                zone_id=zone_id
                                layer=current_layer.into()
                                fence=fence
                                media_names=media_names
                                effect_names=effect_names
                                effect_cache=effect_cache
                                expanded=expanded
                                on_disclosure=on_disclosure
                                health=row_health
                                writes=writes
                                targets=row_targets
                                group=group
                                backdrop=backdrop
                                on_screen=on_screen
                                on_copy=on_copy
                                blend_open=blend_open
                                on_blend_disclosure=Some(on_blend_disclosure)
                            />
                        }
                        .into_any()
                    }
                />
                <Show when=move || !on_screens.get() && canvas_layers.with(|layers| layers.len() > 1)>
                    <div class="text-[10px] font-mono uppercase tracking-wide text-fg-tertiary/65">"Bottom"</div>
                </Show>
            </div>

            // The screens' add, under their layers, and what the zone puts
            // behind them.
            <Show when=move || on_screens.get()>
                <button
                    type="button"
                    class="inline-flex w-full items-center justify-center gap-1.5 rounded-lg border border-accent-muted/30 bg-accent/10 px-3 py-2 text-xs font-semibold text-accent transition-colors hover:bg-accent/15 btn-press disabled:cursor-not-allowed disabled:opacity-45"
                    disabled=move || view_now.with(|view| view.as_ref().is_some_and(|view| view.no_add.is_some()))
                    title=move || view_now.with(|view| view.as_ref().and_then(|view| view.no_add.clone()))
                    data-screen-add=""
                    on:click=move |_| open_picker()
                >
                    <Icon icon=LuPlus width="13px" height="13px" />
                    {move || view_now.with(|view| view.as_ref().map(|view| view.add_label.clone()))}
                </button>
                <div class="space-y-1.5 pt-2">
                    <span class=label_class(LabelSize::Small, LabelTone::Default) data-screen-behind="">
                        {move || view_now.with(|view| view.as_ref().map(|view| view.behind_heading.clone()))}
                    </span>
                    {move || {
                        let behind = view_now
                            .with(|view| view.as_ref().map(|view| view.behind.clone()))
                            .unwrap_or_default();
                        if behind.is_empty() {
                            return view! {
                                <p class="px-1 text-[11px] text-fg-tertiary">
                                    "Nothing. The screen is dark where its layers let it through."
                                </p>
                            }
                            .into_any();
                        }
                        let stack = layers.get();
                        let outputs = target_host.get().map(|host| host.outputs.get());
                        behind
                            .into_iter()
                            .filter_map(|id| stack.iter().find(|layer| layer.id == id).cloned())
                            .map(|layer| {
                                let (icon, _, _) = row::source_meta(&layer.source);
                                let title = title_of(&layer);
                                let reach = match (&layer.scope, &outputs) {
                                    (None, _) => WHOLE_ZONE.to_owned(),
                                    (Some(scope), Some(outputs)) => {
                                        scope::target_words(Some(scope), outputs).label().to_owned()
                                    }
                                    (Some(_), None) => String::new(),
                                };
                                let leave = screens.with_value(|host| host.as_ref().map(|host| host.leave));
                                let label = format!("Show {title} in the zone's layers");
                                view! {
                                    <button
                                        type="button"
                                        class="flex w-full items-center gap-2.5 rounded-lg border border-edge-subtle/50 bg-surface-sunken/30 px-3 py-2 text-left opacity-70 transition-[opacity,border-color] hover:border-edge-subtle hover:opacity-100"
                                        title="Edit it in the zone's layers"
                                        aria-label=label
                                        data-behind-row=""
                                        on:click=move |_| {
                                            if let Some(leave) = leave {
                                                leave.run(());
                                            }
                                        }
                                    >
                                        <span class="flex shrink-0 text-fg-tertiary">
                                            <Icon icon=icon width="13px" height="13px" />
                                        </span>
                                        <span class="min-w-0 flex-1 truncate text-xs font-medium text-fg-secondary">{title}</span>
                                        <span class="shrink-0 text-[10px] text-fg-tertiary">{reach}</span>
                                    </button>
                                }
                            })
                            .collect_view()
                            .into_any()
                    }}
                </div>
            </Show>

            <Show when=move || show_picker.get()>
                <AddLayerPicker
                    assets=assets
                    mode=picker_mode
                    target=Signal::derive(move || {
                        if picker_on_screens.get() {
                            return view_now.with(|view| view.as_ref().map(|view| view.target.clone()));
                        }
                        let target = picker_target.get().filter(|target| !target.is_whole_zone())?;
                        let outputs = target_host.get()?.outputs.get();
                        Some(draft_words(&target, &outputs))
                    })
                    all_screens=Signal::derive(move || {
                        if !picker_on_screens.get() {
                            return None;
                        }
                        screens
                            .with_value(|host| host.as_ref().map(|host| host.all_screens))
                            .and_then(|reach| reach.get())
                    })
                    on_pick=add_layer
                    on_cancel=Callback::new(move |()| set_show_picker.set(false))
                />
            </Show>
        </section>
    }
}

/// How unwell a layer's health is: the higher, the worse.
fn health_rank(health: &LayerHealth) -> u8 {
    match health {
        LayerHealth::Active => 0,
        LayerHealth::Loading => 1,
        LayerHealth::Stalled => 2,
        LayerHealth::AssetMissing => 3,
        LayerHealth::Failed { .. } => 4,
    }
}

#[component]
fn LayerLoadingSkeleton() -> impl IntoView {
    view! {
        <div class="space-y-2">
            {(0..3).map(|_| view! {
                <div class="h-[96px] rounded-xl border border-edge-subtle/50 bg-surface-sunken/40 animate-pulse" />
            }).collect_view()}
        </div>
    }
}

/// Push a single-field layer update, guarded by the `If-Match` precondition.
fn update_layer(zone_id: String, layer: SceneLayer, fence: api::Fence, writes: LayerWrites) {
    let layer_id = layer.id.to_string();
    let request = api::update_request_from_layer(&layer, writes.scope_field());
    let before = writes.stack.get_untracked();
    leptos::task::spawn_local(async move {
        match api::update_layer(&zone_id, &layer_id, &request, Some(&fence)).await {
            Ok(api::LayerStackOutcome::Applied(stack)) => {
                // The replace minted a new id for the layer at the same
                // place in the stack; whoever tracks the layer follows it.
                let minted = before
                    .iter()
                    .position(|candidate| candidate.id == layer.id)
                    .filter(|_| before.len() == stack.items.len())
                    .and_then(|index| stack.items.get(index))
                    .map(|replaced| replaced.id);
                if let Some(minted) = minted {
                    writes.replaced.run((zone_id, layer.id, minted));
                }
                writes.mutated.run(());
            }
            Ok(api::LayerStackOutcome::Stale { .. }) => {
                writes.mutated.run(());
                toasts::toast_error("Layer stack changed elsewhere, so it was reloaded");
            }
            Err(error) => toasts::toast_error(&format!("Layer update failed: {error}")),
        }
    });
}

/// Remove a layer, guarded by the `If-Match` precondition.
fn delete_layer(
    zone_id: String,
    layer_id: String,
    fence: api::Fence,
    on_layers_mutated: Callback<()>,
) {
    leptos::task::spawn_local(async move {
        match api::delete_layer(&zone_id, &layer_id, Some(&fence)).await {
            Ok(api::LayerStackOutcome::Applied(_)) => {
                on_layers_mutated.run(());
                toasts::toast_success("Layer removed");
            }
            Ok(api::LayerStackOutcome::Stale { .. }) => {
                on_layers_mutated.run(());
                toasts::toast_error("Layer stack changed elsewhere, so it was reloaded");
            }
            Err(error) => toasts::toast_error(&format!("Layer delete failed: {error}")),
        }
    });
}

/// Trade a layer's place with its neighbor in `group`, guarded by the
/// `If-Match` precondition. Layers outside the group keep their slots.
fn reorder_layer(
    zone_id: String,
    stack: &[SceneLayer],
    group: &[SceneLayerId],
    layer_id: SceneLayerId,
    delta: isize,
    fence: api::Fence,
    on_layers_mutated: Callback<()>,
) {
    let order: Vec<SceneLayerId> = stack.iter().map(|layer| layer.id).collect();
    let Some(layer_ids) = reorder_within(&order, group, layer_id, delta) else {
        return;
    };
    leptos::task::spawn_local(async move {
        match api::reorder_layers(&zone_id, layer_ids, Some(&fence)).await {
            Ok(api::LayerStackOutcome::Applied(_)) => on_layers_mutated.run(()),
            Ok(api::LayerStackOutcome::Stale { .. }) => {
                on_layers_mutated.run(());
                toasts::toast_error("Layer stack changed elsewhere, so it was reloaded");
            }
            Err(error) => toasts::toast_error(&format!("Layer reorder failed: {error}")),
        }
    });
}

/// Keep editors mounted across value/revision changes, but retire a session
/// when its layer or source identity changes.
#[must_use]
pub fn layer_mount_key(layer: &SceneLayer) -> (String, String) {
    use hypercolor_types::layer::LayerSource;
    let source = match &layer.source {
        LayerSource::Effect { effect_id, .. } => format!("effect:{effect_id}"),
        LayerSource::Media { asset_id, .. } => format!("media:{asset_id}"),
        other => row::source_meta(other).2.to_owned(),
    };
    (layer.id.to_string(), source)
}
