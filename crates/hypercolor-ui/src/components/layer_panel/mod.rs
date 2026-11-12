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

use hypercolor_types::layer::{SceneLayer, SceneLayerId};
use hypercolor_types::scene::ZoneRole;
use leptos::prelude::*;
use leptos_icons::Icon;

use crate::api;
use crate::app::WsContext;
use crate::components::silk_select::SilkSelect;
use crate::icons::*;
use crate::toasts;
use crate::ws::messages::layer_health_key;
use crate::zones::surface::zone_display_name;

use picker::{AddLayerPicker, NewLayerDraft};
use row::LayerRow;
use scope::{Target, TargetOutput, WHOLE_ZONE, draft_words, pickable_count};
use source::{
    AddLayerScope, add_layer_request, available_add_layer_scopes, resolve_add_layer_targets,
};

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
) -> impl IntoView {
    let effect_cache = LayerEffectCache::new(HashMap::new());
    // Disclosure preference belongs to a visible stack slot, independently of
    // the fresh layer authority minted by a whole-layer replacement.
    let disclosures = StoredValue::new(HashMap::<(String, String, usize), bool>::new());
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
    let row_keys = Memo::new(move |_| {
        let scene = scene_id.get();
        let zone = selected_zone_id.get().unwrap_or_default();
        layers
            .get()
            .into_iter()
            .rev()
            .map(|layer| (scene.clone(), zone.clone(), layer_mount_key(&layer)))
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
                    .map(|zone| {
                        let name = zone_display_name(&zone.name, zone.role);
                        (zone.id.to_string(), name)
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    });

    let selected_zone_role = Signal::derive(move || {
        let selected = selected_zone_id.get()?;
        active_scene
            .get()?
            .zones
            .into_iter()
            .find(|zone| zone.id.to_string() == selected)
            .map(|zone| zone.role)
    });
    // The Add-layer target scopes worth offering for the selected surface.
    let scopes = Signal::derive(move || {
        active_scene
            .get()
            .map(|scene| available_add_layer_scopes(&scene.zones, selected_zone_role.get()))
            .unwrap_or_default()
    });

    // A new layer's target only shows where there is something to choose
    // between: an LED zone with at least two outputs.
    let target_host = targets;
    let add_target = Memo::new(move |_| {
        let host = target_host.get()?;
        if selected_zone_role.get() == Some(ZoneRole::Display) {
            return None;
        }
        let outputs = host.outputs.get();
        (pickable_count(&outputs) > 1).then(|| (host.add_target.get(), outputs))
    });
    let add_target_words = Memo::new(move |_| {
        add_target.with(|target| {
            target
                .as_ref()
                .map(|(target, outputs)| draft_words(target, outputs))
        })
    });
    // The host's "+ Layer" asks for the picker; the count is only read to
    // notice that it moved.
    Effect::new(move |seen: Option<Option<u64>>| {
        let asked = target_host.get().map(|host| host.add_requests.get());
        if let Some(Some(before)) = seen
            && asked.is_some_and(|now| now != before)
        {
            assets_resource.refetch();
            set_show_picker.set(true);
        }
        asked
    });

    let add_layer = Callback::new(move |(draft, scope): (NewLayerDraft, AddLayerScope)| {
        set_show_picker.set(false);
        let Some(scene) = active_scene.get_untracked() else {
            toasts::toast_error("No active scene is available");
            return;
        };
        let Some(zone_id) = selected_zone_id.get_untracked() else {
            toasts::toast_error("No surface is selected");
            return;
        };
        let targets = resolve_add_layer_targets(scope, &scene.zones, &zone_id);
        if targets.is_empty() {
            toasts::toast_error("There is nothing to add the layer to");
            return;
        }
        let expected_revision = scene_revision.get_untracked();
        let run = run.get_untracked();
        let existing_layer_count = layers_resource
            .get_untracked()
            .and_then(Result::ok)
            .map_or(0, |stack| stack.items.len());
        let mut request = add_layer_request(draft.name, draft.source, existing_layer_count);
        // Only the open zone's own layer takes the target; a layer sent to
        // every screen is each screen's whole.
        let target = (scope == AddLayerScope::ThisSurface)
            .then(|| add_target.get_untracked())
            .flatten();
        let mut added_to = None;
        let mut added_target = Target::WholeZone;
        if let Some((mut target, outputs)) = target {
            target.drop_missing(&outputs);
            if target.is_complete() && !target.is_whole_zone() {
                added_to = Some(draft_words(&target, &outputs));
                request.scope = target.scope();
                added_target = target;
            }
        }
        let host = target_host.get_untracked();
        leptos::task::spawn_local(async move {
            let mut applied = 0_usize;
            let mut failed = 0_usize;
            let mut fence = expected_revision.map(|revision| api::Fence {
                revision,
                run: run.clone(),
            });
            for (index, target) in targets.iter().enumerate() {
                match api::create_layer(target, &request, fence.as_ref()).await {
                    Ok(api::LayerStackOutcome::Applied(stack)) => {
                        applied += 1;
                        fence = Some(api::Fence {
                            revision: stack.revision,
                            run: run.clone(),
                        });
                    }
                    Ok(api::LayerStackOutcome::Stale { .. }) | Err(_) => {
                        failed += targets.len() - index;
                        break;
                    }
                }
            }
            on_layers_mutated.run(());
            if applied > 0
                && let Some(host) = host
            {
                host.added.run(added_target);
            }
            if failed == 0 {
                toasts::toast_success(&match (applied, added_to) {
                    (1, Some(target)) => format!("Layer added to {target}"),
                    (1, None) => "Layer added".to_owned(),
                    (count, _) => format!("Layer added to {count} screens"),
                });
            } else if applied == 0 {
                toasts::toast_error("Layer add failed");
            } else {
                toasts::toast_error(&format!(
                    "Layer added to {applied} screens, {failed} failed"
                ));
            }
        });
    });

    let writes = LayerWrites {
        mutated: on_layers_mutated,
        stack: layers.into(),
        replaced: Callback::new(move |minted| {
            if let Some(host) = targets.get_untracked() {
                host.replaced.run(minted);
            }
        }),
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

            <div>
                <button
                    type="button"
                    class="inline-flex w-full items-center justify-center gap-1.5 rounded-lg border border-accent-muted/30 bg-accent/10 px-3 py-2 text-xs font-semibold text-accent transition-colors hover:bg-accent/15 btn-press disabled:cursor-not-allowed disabled:opacity-45"
                    disabled=move || selected_zone_id.get().is_none()
                    on:click=move |_| {
                        // The asset list is decoupled from any host page's
                        // refresh tick, so refresh it on demand, or a file
                        // uploaded since mount is missing from the picker.
                        assets_resource.refetch();
                        set_show_picker.set(true);
                    }
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
                                format!(
                                    "New layer applies to {}",
                                    add_target_words.get().unwrap_or_default(),
                                )
                            }
                            title="Choose what the new layer applies to"
                            on:click=move |ev: web_sys::MouseEvent| {
                                if let (Some(host), Some(element)) =
                                    (target_host.get_untracked(), crate::pointer_gesture::listener_element(&ev))
                                {
                                    host.edit.run((TargetSubject::NewLayer, element));
                                }
                            }
                        >
                            <span>"to"</span>
                            <span
                                class="min-w-0 truncate font-medium"
                                class=("text-fg-secondary", move || {
                                    add_target_words.with(|words| words.as_deref() == Some(WHOLE_ZONE))
                                })
                                class=("text-fg-primary", move || {
                                    add_target_words.with(|words| words.as_deref() != Some(WHOLE_ZONE))
                                })
                            >
                                {move || add_target_words.get().unwrap_or_default()}
                            </span>
                            <Icon icon=LuChevronDown width="11px" height="11px" />
                        </button>
                    </div>
                </Show>
            </div>

            <Show when=move || layers_resource.get().is_none()>
                <LayerLoadingSkeleton />
            </Show>
            {move || layers_resource.get().and_then(Result::err).map(|error| view! {
                <div class="rounded-lg border border-status-error/30 bg-status-error/10 px-3 py-3 text-xs text-status-error">{error.to_string()}</div>
            })}
            <Show when=move || layers_resource.get().is_some_and(|result| result.is_ok()) && layers.with(Vec::is_empty)>
                <div class="rounded-lg border border-edge-subtle bg-surface-sunken/45 px-3 py-8 text-center text-xs text-fg-tertiary">
                    {move || if selected_zone_role.get() == Some(ZoneRole::Display) { "No scene layers on this screen" } else { "No layers in this zone" }}
                </div>
            </Show>
            <div class="space-y-2">
                <Show when=move || layers.with(|layers| layers.len() > 1)>
                    <div class="text-[10px] font-mono uppercase tracking-wide text-fg-tertiary/65">"Top"</div>
                </Show>
                <For
                    each=move || row_keys.get()
                    key=|key| key.clone()
                    children=move |(scene_id, zone_id, (layer_id, _))| {
                        let initial = layers.with_untracked(|layers| layers.iter().find(|layer| layer.id.to_string() == layer_id).cloned()).expect("keyed layer is present when mounted");
                        let lookup_id = layer_id.clone();
                        let current_layer = Memo::new(move |_| {
                            layers.with(|layers| layers.iter().find(|layer| layer.id.to_string() == lookup_id).cloned()).unwrap_or_else(|| initial.clone())
                        });
                        let stack_index = Signal::derive(move || layers.with(|layers| layers.iter().position(|layer| layer.id.to_string() == layer_id).unwrap_or_default()));
                        let disclosure_key = (scene_id.clone(), zone_id.clone(), stack_index.get_untracked());
                        let expanded = disclosures.with_value(|states| states.get(&disclosure_key).copied().unwrap_or(false));
                        let on_disclosure = Callback::new(move |open| {
                            let mut key = disclosure_key.clone();
                            key.2 = stack_index.get_untracked();
                            disclosures.update_value(|states| { states.insert(key, open); });
                        });
                        let row_health_key = layer_health_key(&scene_id, &zone_id, &current_layer.get_untracked().id.to_string());
                        let row_health = Signal::derive(move || layer_health.with(|map| map.get(&row_health_key).cloned()));
                        // A screen's stack is its face: no target to show.
                        let row_targets = Signal::derive(move || {
                            targets
                                .get()
                                .filter(|_| selected_zone_role.get() != Some(ZoneRole::Display))
                        });
                        view! {
                            <LayerRow
                                zone_id=zone_id
                                layer=current_layer.into()
                                stack_index=stack_index
                                fence=fence
                                media_names=media_names
                                effect_names=effect_names
                                effect_cache=effect_cache
                                expanded=expanded
                                on_disclosure=on_disclosure
                                health=row_health
                                writes=writes
                                targets=row_targets
                            />
                        }
                    }
                />
                <Show when=move || layers.with(|layers| layers.len() > 1)>
                    <div class="text-[10px] font-mono uppercase tracking-wide text-fg-tertiary/65">"Bottom"</div>
                </Show>
            </div>

            <Show when=move || show_picker.get()>
                <AddLayerPicker
                    assets=assets
                    scopes=scopes
                    selected_surface_role=selected_zone_role
                    target=Signal::derive(move || {
                        add_target_words.get().filter(|words| words != WHOLE_ZONE)
                    })
                    on_pick=add_layer
                    on_cancel=Callback::new(move |()| set_show_picker.set(false))
                />
            </Show>
        </section>
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
    let request = api::update_request_from_layer(&layer);
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

/// Swap a layer with its neighbor, guarded by the `If-Match` precondition.
fn reorder_layer(
    zone_id: String,
    stack: Vec<SceneLayer>,
    index: usize,
    delta: isize,
    fence: api::Fence,
    on_layers_mutated: Callback<()>,
) {
    let Some(target) = index.checked_add_signed(delta) else {
        return;
    };
    if target >= stack.len() {
        return;
    }

    let mut layer_ids = stack.iter().map(|layer| layer.id).collect::<Vec<_>>();
    layer_ids.swap(index, target);
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
