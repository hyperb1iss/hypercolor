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
//! - **Surface identity** — `active_scene` + `selected_group_id` name the
//!   `(scene id, group id)` pair every mutation is addressed to. The panel
//!   never displays the ids.
//! - **`layers_version`** — read from `layers_resource`; threaded as the
//!   `If-Match` precondition on every mutation. A stale write is reported
//!   and the stack refetched, never silently lost.
//! - **Add-layer picker** — Add-layer opens [`picker::AddLayerPicker`],
//!   covering effects/faces plus media assets.
//! - **One mutation callback** — `on_layers_mutated: Callback<()>` fires
//!   after every applied or rejected mutation; the host refetches the
//!   stack (and active scene) in response. There is exactly one.
//! - **Transform / adjust sub-panels** — each [`row::LayerRow`] carries the
//!   collapsed transform + color-adjust disclosure.
//!
//! Content selection is owned internally (its own asset resource), so the
//! component is decoupled from any host page's selection state.

mod controls;
mod picker;
mod row;
pub mod source;

use std::collections::HashMap;

use hypercolor_types::layer::{LayerAdjust, LayerTransform, SceneLayer};
use leptos::prelude::*;
use leptos_icons::Icon;

use crate::api;
use crate::app::WsContext;
use crate::components::silk_select::SilkSelect;
use crate::icons::*;
use crate::toasts;
use crate::ws::messages::layer_health_key;

use picker::{AddLayerPicker, NewLayerDraft};
use row::LayerRow;
use source::{
    AddLayerScope, available_add_layer_scopes, default_blend_for_added_layer,
    resolve_add_layer_targets,
};

/// Layer-stack editor for one zone. See the module docs for the
/// mount contract.
#[component]
pub fn LayerPanel(
    #[prop(into)] active_scene: Signal<Option<api::ActiveSceneResponse>>,
    selected_group_id: ReadSignal<Option<String>>,
    set_selected_group_id: WriteSignal<Option<String>>,
    /// Surface name supplied by a host that owns surface selection
    /// elsewhere (the Studio zone tree). When present the panel shows it
    /// in the header and drops its own redundant group selector.
    #[prop(optional, into)]
    surface_label: MaybeProp<String>,
    layers_resource: LocalResource<Result<api::LayerStackResponse, String>>,
    on_layers_mutated: Callback<()>,
) -> impl IntoView {
    // Content selection is owned here, not driven by the host page — the
    // asset list backs both media-name resolution and the picker's Media tab.
    let assets_resource = LocalResource::new(|| async { api::list_assets().await });
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
            .map(|asset| (asset.id, asset.name))
            .collect::<HashMap<String, String>>()
    });
    // Effect ids on a layer are UUIDs; resolve them to registry names so
    // a layer row reads "Effect Aurora", never "Effect <uuid>".
    let effects_resource = LocalResource::new(api::fetch_effects);
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
    let layers_version = Signal::derive(move || {
        layers_resource
            .get()
            .and_then(Result::ok)
            .map(|stack| stack.layers_version)
    });

    // Per-layer runtime health streams in over the WebSocket, independent
    // of the layer stack itself; an absent context means no health yet.
    let ws = use_context::<WsContext>();
    let layer_health =
        Signal::derive(move || ws.map(|ws| ws.layer_health.get()).unwrap_or_default());

    let (show_picker, set_show_picker) = signal(false);

    let group_options = Signal::derive(move || {
        active_scene
            .get()
            .map(|scene| {
                scene
                    .groups
                    .into_iter()
                    .map(|group| (group.id.to_string(), group.name))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    });

    // The Add-layer target scopes worth offering for the current scene.
    let scopes = Signal::derive(move || {
        active_scene
            .get()
            .map(|scene| available_add_layer_scopes(&scene.groups))
            .unwrap_or_default()
    });
    let selected_group_role = Signal::derive(move || {
        let selected = selected_group_id.get()?;
        active_scene
            .get()?
            .groups
            .into_iter()
            .find(|group| group.id.to_string() == selected)
            .map(|group| group.role)
    });

    let add_layer = Callback::new(move |(draft, scope): (NewLayerDraft, AddLayerScope)| {
        set_show_picker.set(false);
        let Some(scene) = active_scene.get_untracked() else {
            toasts::toast_error("No active scene is available");
            return;
        };
        let Some(group_id) = selected_group_id.get_untracked() else {
            toasts::toast_error("No surface is selected");
            return;
        };
        let targets = resolve_add_layer_targets(scope, &scene.groups, &group_id);
        if targets.is_empty() {
            toasts::toast_error("No target surfaces for that scope");
            return;
        }
        let expected_version = layers_version.get_untracked();
        let existing_layer_count = layers_resource
            .get_untracked()
            .and_then(Result::ok)
            .map_or(0, |stack| stack.items.len());
        let blend = default_blend_for_added_layer(&draft.source, existing_layer_count);
        let request = api::CreateLayerRequest {
            name: draft.name,
            source: draft.source,
            blend,
            opacity: 1.0,
            transform: LayerTransform::default(),
            adjust: LayerAdjust::default(),
            bindings: Vec::new(),
            enabled: true,
        };
        leptos::task::spawn_local(async move {
            let mut applied = 0_usize;
            let mut failed = 0_usize;
            for target in &targets {
                // `If-Match` guards the surface on screen; bulk targets are
                // not being watched, so they add unconditionally.
                let version = if *target == group_id {
                    expected_version
                } else {
                    None
                };
                match api::create_layer(&scene.id, target, &request, version).await {
                    Ok(api::LayerStackOutcome::Applied(_)) => applied += 1,
                    Ok(api::LayerStackOutcome::Stale { .. }) | Err(_) => failed += 1,
                }
            }
            on_layers_mutated.run(());
            if failed == 0 {
                toasts::toast_success(&match applied {
                    1 => "Layer added".to_owned(),
                    count => format!("Layer added to {count} surfaces"),
                });
            } else if applied == 0 {
                toasts::toast_error("Layer add failed");
            } else {
                toasts::toast_error(&format!(
                    "Layer added to {applied} surfaces, {failed} failed"
                ));
            }
        });
    });

    view! {
        <section class="mt-4 rounded-xl border border-edge-subtle/70 bg-surface-overlay/50">
            <div class="flex items-center justify-between gap-3 border-b border-edge-subtle/60 px-4 py-3">
                <div>
                    <div class="text-sm font-semibold text-fg-primary">"Layer Stack"</div>
                    <div class="text-[11px] text-fg-tertiary">
                        {move || {
                            surface_label
                                .get()
                                .unwrap_or_else(|| "Active scene zone".to_owned())
                        }}
                    </div>
                </div>
                <Icon icon=LuLayers width="16px" height="16px" style="color: rgba(225, 53, 255, 0.72)" />
            </div>
            <div class="space-y-4 px-4 py-4">
                <Show when=move || surface_label.get().is_none()>
                    <SilkSelect
                        value=Signal::derive(move || selected_group_id.get().unwrap_or_default())
                        options=group_options
                        on_change=Callback::new(move |id: String| {
                            set_selected_group_id.set((!id.is_empty()).then_some(id));
                        })
                        placeholder="Select group"
                        class="border border-edge-subtle bg-surface-sunken/55 px-3 py-2 text-xs text-fg-primary"
                        label_class="font-medium"
                    />
                </Show>

                <button
                    type="button"
                    class="inline-flex w-full items-center justify-center gap-1.5 rounded-lg border border-accent-muted/30 bg-accent/10 px-3 py-2 text-xs font-semibold text-accent transition-colors hover:bg-accent/15 btn-press disabled:cursor-not-allowed disabled:opacity-45"
                    disabled=move || selected_group_id.get().is_none()
                    on:click=move |_| {
                        // The asset list is decoupled from any host page's
                        // refresh tick, so refresh it on demand — otherwise a
                        // file uploaded since mount is missing from the picker.
                        assets_resource.refetch();
                        set_show_picker.set(true);
                    }
                >
                    <Icon icon=LuPlus width="13px" height="13px" />
                    "Add layer"
                </button>

                <Suspense fallback=move || view! { <LayerLoadingSkeleton /> }>
                    {move || match layers_resource.get() {
                        None => view! { <LayerLoadingSkeleton /> }.into_any(),
                        Some(Err(error)) => view! {
                            <div class="rounded-lg border border-status-error/30 bg-status-error/10 px-3 py-3 text-xs text-status-error">
                                {error}
                            </div>
                        }.into_any(),
                        Some(Ok(stack)) if stack.items.is_empty() => view! {
                            <div class="rounded-lg border border-edge-subtle bg-surface-sunken/45 px-3 py-8 text-center text-xs text-fg-tertiary">
                                "No layers in this group"
                            </div>
                        }.into_any(),
                        Some(Ok(stack)) => {
                            let scene_id = active_scene.get().map(|scene| scene.id).unwrap_or_default();
                            let group_id = selected_group_id.get().unwrap_or_default();
                            let names = media_names.get();
                            let effect_name_map = effect_names.get();
                            let version = stack.layers_version;
                            let total = stack.items.len();
                            let mut rows = stack
                                .items
                                .iter()
                                .cloned()
                                .enumerate()
                                .collect::<Vec<_>>();
                            rows.reverse();
                            // The Top/Bottom stack markers orient a real
                            // stack; with a single layer there is no
                            // ordering to convey, so they stay hidden.
                            let show_stack_markers = total > 1;
                            view! {
                                <div class="space-y-2">
                                    {show_stack_markers.then(|| view! {
                                        <div class="text-[10px] font-mono uppercase tracking-wide text-fg-tertiary/65">
                                            "Top"
                                        </div>
                                    })}
                                    {rows.into_iter().map(|(stack_index, layer)| {
                                        let row_health_key = layer_health_key(
                                            &scene_id,
                                            &group_id,
                                            &layer.id.to_string(),
                                        );
                                        let row_health = Signal::derive(move || {
                                            layer_health.with(|map| map.get(&row_health_key).cloned())
                                        });
                                        view! {
                                            <LayerRow
                                                scene_id=scene_id.clone()
                                                group_id=group_id.clone()
                                                layer=layer
                                                stack_index=stack_index
                                                total_layers=total
                                                stack=stack.items.clone()
                                                layers_version=version
                                                media_names=names.clone()
                                                effect_names=effect_name_map.clone()
                                                health=row_health
                                                on_layers_mutated=on_layers_mutated
                                            />
                                        }
                                    }).collect_view()}
                                    {show_stack_markers.then(|| view! {
                                        <div class="text-[10px] font-mono uppercase tracking-wide text-fg-tertiary/65">
                                            "Bottom"
                                        </div>
                                    })}
                                </div>
                            }.into_any()
                        }
                    }}
                </Suspense>
            </div>

            <Show when=move || show_picker.get()>
                <AddLayerPicker
                    assets=assets
                    scopes=scopes
                    selected_surface_role=selected_group_role
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
fn update_layer(
    scene_id: String,
    group_id: String,
    layer: SceneLayer,
    layers_version: u64,
    on_layers_mutated: Callback<()>,
) {
    let layer_id = layer.id.to_string();
    let request = api::UpdateLayerRequest::from(&layer);
    leptos::task::spawn_local(async move {
        match api::update_layer(
            &scene_id,
            &group_id,
            &layer_id,
            &request,
            Some(layers_version),
        )
        .await
        {
            Ok(api::LayerStackOutcome::Applied(_)) => on_layers_mutated.run(()),
            Ok(api::LayerStackOutcome::Stale { .. }) => {
                on_layers_mutated.run(());
                toasts::toast_error("Layer stack changed elsewhere — reloaded");
            }
            Err(error) => toasts::toast_error(&format!("Layer update failed: {error}")),
        }
    });
}

/// Remove a layer, guarded by the `If-Match` precondition.
fn delete_layer(
    scene_id: String,
    group_id: String,
    layer_id: String,
    layers_version: u64,
    on_layers_mutated: Callback<()>,
) {
    leptos::task::spawn_local(async move {
        match api::delete_layer(&scene_id, &group_id, &layer_id, Some(layers_version)).await {
            Ok(api::LayerStackOutcome::Applied(_)) => {
                on_layers_mutated.run(());
                toasts::toast_success("Layer removed");
            }
            Ok(api::LayerStackOutcome::Stale { .. }) => {
                on_layers_mutated.run(());
                toasts::toast_error("Layer stack changed elsewhere — reloaded");
            }
            Err(error) => toasts::toast_error(&format!("Layer delete failed: {error}")),
        }
    });
}

/// Swap a layer with its neighbor, guarded by the `If-Match` precondition.
fn reorder_layer(
    scene_id: String,
    group_id: String,
    stack: Vec<SceneLayer>,
    index: usize,
    delta: isize,
    layers_version: u64,
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
        match api::reorder_layers(&scene_id, &group_id, layer_ids, Some(layers_version)).await {
            Ok(api::LayerStackOutcome::Applied(_)) => on_layers_mutated.run(()),
            Ok(api::LayerStackOutcome::Stale { .. }) => {
                on_layers_mutated.run(());
                toasts::toast_error("Layer stack changed elsewhere — reloaded");
            }
            Err(error) => toasts::toast_error(&format!("Layer reorder failed: {error}")),
        }
    });
}
