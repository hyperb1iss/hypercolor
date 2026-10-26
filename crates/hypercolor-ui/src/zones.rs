//! Global zone + scene state.
//!
//! One shared `/scene` resource and one `/scenes` list resource,
//! refreshed from WebSocket scene events, exposed app-wide as
//! [`ZonesContext`] (what zones exist, which one is focused) and
//! [`ScenesContext`] (what scenes exist, switching between them). Studio,
//! the dashboard, the sidebar, and the effects page all read the same
//! state — no page-local `fetch_active_scene` snapshots that go stale
//! when a scene changes from another surface, another client, or the CLI.

pub mod surface;

use hypercolor_types::layer::LayerSource;
use hypercolor_types::scene::ZoneRole;
use leptos::prelude::*;

use crate::api;
use crate::app::WsContext;
use crate::toasts;
use crate::ws::SceneEventHint;
use surface::{Surface, SurfaceKind, led_zone_count, surfaces_from_zones};

/// Zone-level view of the active scene, provided at the app root.
#[derive(Clone, Copy)]
pub struct ZonesContext {
    /// The active scene, shared by every consumer. `None` while loading
    /// or when only the ephemeral default is running with no zones yet.
    pub active_scene: Memo<Option<api::SceneDocument>>,
    /// All zones of the active scene in scene order (LED zones and
    /// display Screens), as the UI presents them.
    pub zones: Memo<Vec<Surface>>,
    /// LED-role zones only — what effect application targets.
    pub led_zones: Memo<Vec<Surface>>,
    /// Whether the scene composes more than one LED zone. The trigger
    /// for per-zone affordances (now-playing rows, zone tabs, badges).
    pub multi_zone: Memo<bool>,
    /// Whether the active scene has been read for this connection, so
    /// `multi_zone` and the zone lists describe it rather than an empty
    /// page still loading. See [`scene_read_settles`].
    pub scene_resolved: Memo<bool>,
    /// The one record of which LED zone effect applies, presets, the
    /// controls, and the preview target. `None`, or an id that has left
    /// the scene, means the primary zone. Written by the zone tabs and by
    /// Studio's selection; read through [`ZonesContext::target_zone`].
    pub focused_zone: RwSignal<Option<String>>,
    /// Refetch the shared active-scene resource.
    pub refresh: Callback<()>,
    /// The zone patch Studio records as an undo step, while Studio is
    /// open. Elsewhere it is `None`, and a zone patch writes directly.
    pub studio_zone_patch: StoredValue<Option<Callback<(String, api::zones::PatchZoneRequest)>>>,
}

impl ZonesContext {
    /// Look up a zone by id.
    pub fn zone(&self, id: &str) -> Option<Surface> {
        self.zones
            .with(|zones| zones.iter().find(|zone| zone.id == id).cloned())
    }

    /// The LED zone writes currently target. See [`surface::target_zone`].
    pub fn target_zone(&self) -> Option<Surface> {
        let focused = self.focused_zone.get();
        self.zones
            .with(|zones| surface::target_zone(zones, focused.as_deref()).cloned())
    }

    /// [`ZonesContext::target_zone`] without tracking, for event handlers.
    pub fn target_zone_untracked(&self) -> Option<Surface> {
        let focused = self.focused_zone.get_untracked();
        self.zones
            .with_untracked(|zones| surface::target_zone(zones, focused.as_deref()).cloned())
    }

    /// The primary LED zone of the active scene.
    pub fn primary_zone(&self) -> Option<Surface> {
        self.zones
            .with(|zones| surface::primary_led_zone(zones).cloned())
    }

    /// Untracked focused-zone id, validated against the current scene.
    /// `None` when unset, stale, or pointing at a Screen.
    pub fn focused_zone_id_untracked(&self) -> Option<String> {
        let id = self.focused_zone.get_untracked()?;
        self.zones.with_untracked(|zones| {
            zones
                .iter()
                .any(|zone| zone.id == id && zone.kind == SurfaceKind::Light)
                .then_some(id)
        })
    }

    /// Immutable effect-layer identity and scene revision at the write target.
    pub fn effect_target_untracked(
        &self,
        effect_id: &str,
        zone_id: Option<&str>,
    ) -> Option<(api::EffectLayerTarget, u64)> {
        self.active_scene
            .with_untracked(|scene| effect_target_in_scene(scene.as_ref()?, effect_id, zone_id))
    }
}

fn effect_target_in_scene(
    scene: &api::SceneDocument,
    effect_id: &str,
    zone_id: Option<&str>,
) -> Option<(api::EffectLayerTarget, u64)> {
    let zone = match zone_id {
        Some(zone_id) => scene
            .zones
            .iter()
            .find(|zone| zone.id.to_string() == zone_id),
        None => scene
            .zones
            .iter()
            .find(|zone| zone.role == ZoneRole::Primary)
            .or_else(|| {
                scene
                    .zones
                    .iter()
                    .find(|zone| zone.role != ZoneRole::Display)
            }),
    }?;
    let layer = zone.layers.iter().rev().find(|layer| {
        let LayerSource::Effect {
            effect_id: current, ..
        } = &layer.source
        else {
            return false;
        };
        current.to_string() == effect_id
    })?;
    Some((
        api::EffectLayerTarget {
            effect_id: effect_id.to_owned(),
            zone_id: zone.id.to_string(),
            layer_id: layer.id.to_string(),
        },
        scene.revision,
    ))
}

/// One LED zone's active-effect state — the per-zone answer to "what is
/// playing here". Derived from the shared scene plus the effects index;
/// every now-playing surface reads these instead of the singular
/// primary-zone snapshot.
#[derive(Debug, Clone, PartialEq)]
pub struct ZoneEffectState {
    /// Zone identity and presentation (name, color, enabled, top layer).
    pub zone: Surface,
    /// The zone's directly-assigned effect, if any. A zone driven by an
    /// authored layer stack reports its top layer via `zone.top_layer`.
    pub effect_id: Option<String>,
    /// Effect display name, resolved from the effects index; falls back
    /// to the zone's top-layer caption when the index doesn't know it.
    pub effect_name: Option<String>,
    pub effect_category: Option<String>,
    pub control_values: std::collections::HashMap<String, hypercolor_types::control::ControlValue>,
    pub preset_id: Option<String>,
    /// Scene revision observed with the control values.
    pub revision: u64,
    /// Labels of the zone's enabled layers, bottom to top.
    pub layer_labels: Vec<String>,
}

impl ZoneEffectState {
    /// What the zone is showing, in words: the resolved effect name,
    /// else the top layer caption, else nothing.
    #[must_use]
    pub fn display_label(&self) -> Option<String> {
        self.effect_name
            .clone()
            .or_else(|| self.zone.top_layer.clone())
    }

    /// What the zone shows, in one short line: its top layer plus how
    /// many layers sit under it ("Touch Ripple +1").
    #[must_use]
    pub fn stack_label(&self) -> Option<String> {
        let top = self
            .layer_labels
            .last()
            .cloned()
            .or_else(|| self.display_label())?;
        let below = self.layer_labels.len().saturating_sub(1);
        Some(if below == 0 {
            top
        } else {
            format!("{top} +{below}")
        })
    }

    /// Every enabled layer, top first ("Touch Ripple over Bubble Garden").
    #[must_use]
    pub fn stack_description(&self) -> Option<String> {
        if self.layer_labels.is_empty() {
            return self.display_label();
        }
        Some(
            self.layer_labels
                .iter()
                .rev()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(" over "),
        )
    }

    /// The effect that owns this zone's picture by itself: its only
    /// layer, when that layer is an enabled effect and the zone is live.
    /// A stacked zone composes several sources, and a paused zone or
    /// layer shows none, so no one effect owns those frames.
    #[must_use]
    pub fn sole_effect_id(&self) -> Option<&str> {
        if !self.zone.enabled || self.zone.layer_ids.len() != 1 || self.layer_labels.len() != 1 {
            return None;
        }
        self.effect_id.as_deref()
    }

    /// Whether the zone is actively rendering something.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.zone.enabled && (self.effect_id.is_some() || self.zone.top_layer.is_some())
    }
}

/// Scene-library view, provided at the app root.
#[derive(Clone, Copy)]
pub struct ScenesContext {
    /// Every saved scene (the daemon omits the ephemeral default).
    pub scenes: Memo<Vec<api::SceneSummary>>,
    /// The shared active scene — same memo as [`ZonesContext::active_scene`].
    pub active: Memo<Option<api::SceneDocument>>,
    /// Scene id mid-activation. Switchers disable and spin on this row;
    /// the displayed value flips only when the daemon confirms.
    pub switching: ReadSignal<Option<String>>,
    /// Refetch both the list and the active scene.
    pub refresh: Callback<()>,
    /// Activate a saved scene by id. No optimistic flip: activation
    /// rewrites zones wholesale and can fail validation daemon-side.
    pub activate: Callback<String>,
    /// Return to the ephemeral default scene.
    pub deactivate: Callback<()>,
}

/// Whether one read of the active scene tells the page what scene this
/// connection serves.
///
/// A document always does, even one read on an earlier connection: a
/// refetch keeps the last document until the next one lands. A failed
/// read counts only when it was made on the current, admitted connection.
/// One made before the socket opened (generation 0, or an earlier socket:
/// the daemon still binding at app boot) says nothing about the scene it
/// is about to serve.
#[must_use]
pub const fn scene_read_settles(
    read_generation: u64,
    read_ok: bool,
    connection_generation: u64,
) -> bool {
    read_ok || (connection_generation > 0 && read_generation == connection_generation)
}

/// Build and provide [`ZonesContext`] + [`ScenesContext`]. Called once
/// from the app root, after the WebSocket manager exists.
pub fn provide_scene_contexts(
    last_scene_event: ReadSignal<Option<SceneEventHint>>,
) -> (ZonesContext, ScenesContext) {
    // Each read remembers the connection it was made on, so a failure
    // from before the socket opened is not taken for this connection's.
    let connection_generation = expect_context::<WsContext>().connection_generation;
    let active_scene_resource = api::daemon_resource(move || {
        let generation = connection_generation.get_untracked();
        async move { (generation, api::fetch_active_scene().await) }
    });
    let scenes_resource = api::daemon_resource(api::list_scenes);

    // Memo (not derive) so refetches that return identical state don't
    // wake every zone-aware surface in the app.
    let active_scene = Memo::new(move |_| {
        active_scene_resource
            .get()
            .and_then(|(_, scene)| scene.ok())
    });
    let scene_resolved = Memo::new(move |_| {
        let generation = connection_generation.get();
        active_scene_resource.with(|read| {
            read.as_ref().is_some_and(|(read_generation, scene)| {
                scene_read_settles(*read_generation, scene.is_ok(), generation)
            })
        })
    });
    let scenes = Memo::new(move |_| {
        scenes_resource
            .get()
            .and_then(Result::ok)
            .unwrap_or_default()
    });

    let zones = Memo::new(move |_| {
        active_scene
            .get()
            .map(|scene| surfaces_from_zones(&scene.zones))
            .unwrap_or_default()
    });
    let led_zones = Memo::new(move |_| {
        zones.with(|zones| {
            zones
                .iter()
                .filter(|zone| zone.kind == SurfaceKind::Light)
                .cloned()
                .collect::<Vec<_>>()
        })
    });
    let multi_zone = Memo::new(move |_| {
        active_scene
            .get()
            .is_some_and(|scene| led_zone_count(&scene.zones) > 1)
    });

    let refresh_active = Callback::new(move |()| active_scene_resource.refetch());
    let refresh_all = Callback::new(move |()| {
        active_scene_resource.refetch();
        scenes_resource.refetch();
    });

    let (switching, set_switching) = signal(None::<String>);
    let activate = Callback::new(move |scene_id: String| {
        if switching.get_untracked().is_some() {
            return;
        }
        set_switching.set(Some(scene_id.clone()));
        leptos::task::spawn_local(async move {
            match api::activate_scene(&scene_id).await {
                Ok(()) => {
                    active_scene_resource.refetch();
                    scenes_resource.refetch();
                }
                Err(error) => toasts::toast_error(&format!("Couldn't switch scene: {error}")),
            }
            set_switching.set(None);
        });
    });
    let deactivate = Callback::new(move |()| {
        if switching.get_untracked().is_some() {
            return;
        }
        set_switching.set(Some(String::new()));
        leptos::task::spawn_local(async move {
            match api::deactivate_scene().await {
                Ok(()) => {
                    active_scene_resource.refetch();
                    scenes_resource.refetch();
                }
                Err(error) => {
                    toasts::toast_error(&format!("Couldn't return to the default scene: {error}"));
                }
            }
            set_switching.set(None);
        });
    });

    // A zone commit includes every control value and the new scene revision.
    // Its per-control notifications share that invalidation, so a compound
    // edit fetches one authoritative snapshot instead of one per control.
    let scene_generation = Memo::new(move |_| {
        last_scene_event.with(|hint| hint.as_ref().map_or(0, |hint| hint.scene_generation))
    });
    Effect::new(move |previous: Option<u64>| {
        let current = scene_generation.get();
        if previous.unwrap_or_default() != current {
            active_scene_resource.refetch();
        }
        current
    });

    // Activation and library CRUD also refresh the saved-scene list.
    Effect::new(move |previous: Option<Option<SceneEventHint>>| {
        let current = last_scene_event.get();
        if previous.as_ref() == Some(&current) {
            return current;
        }
        let Some(hint) = current.as_ref() else {
            return current;
        };

        if matches!(
            hint.event_type.as_str(),
            "active_scene_changed" | "scene_library_changed"
        ) {
            scenes_resource.refetch();
        }

        current
    });

    let zones_ctx = ZonesContext {
        active_scene,
        zones,
        led_zones,
        multi_zone,
        scene_resolved,
        focused_zone: RwSignal::new(None),
        refresh: refresh_active,
        studio_zone_patch: StoredValue::new(None),
    };
    let scenes_ctx = ScenesContext {
        scenes,
        active: active_scene,
        switching,
        refresh: refresh_all,
        activate,
        deactivate,
    };
    provide_context(zones_ctx);
    provide_context(scenes_ctx);
    (zones_ctx, scenes_ctx)
}
