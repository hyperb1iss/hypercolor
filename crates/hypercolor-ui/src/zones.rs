//! Global zone + scene state.
//!
//! One shared `/scenes/active` resource and one `/scenes` list resource,
//! refreshed from WebSocket scene events, exposed app-wide as
//! [`ZonesContext`] (what zones exist, which one is focused) and
//! [`ScenesContext`] (what scenes exist, switching between them). Studio,
//! the dashboard, the sidebar, and the effects page all read the same
//! state — no page-local `fetch_active_scene` snapshots that go stale
//! when a scene changes from another surface, another client, or the CLI.

pub mod surface;

use leptos::prelude::*;

use crate::api;
use crate::toasts;
use crate::ws::SceneEventHint;
use surface::{Surface, SurfaceKind, led_zone_count, surfaces_from_groups};

/// Zone-level view of the active scene, provided at the app root.
#[derive(Clone, Copy)]
pub struct ZonesContext {
    /// The active scene, shared by every consumer. `None` while loading
    /// or when only the ephemeral default is running with no zones yet.
    pub active_scene: Memo<Option<api::ActiveSceneResponse>>,
    /// All zones of the active scene in scene order (LED zones and
    /// display Screens), as the UI presents them.
    pub zones: Memo<Vec<Surface>>,
    /// LED-role zones only — what effect application targets.
    pub led_zones: Memo<Vec<Surface>>,
    /// Whether the scene composes more than one LED zone. The trigger
    /// for per-zone affordances (now-playing rows, zone tabs, badges).
    pub multi_zone: Memo<bool>,
    /// The zone quick-applies and the controls panel target. `None`
    /// means the primary zone. Set by Studio's selection and by explicit
    /// zone pickers; surfaces that consume it must render it visibly.
    pub focused_zone: RwSignal<Option<String>>,
    /// Refetch the shared active-scene resource.
    pub refresh: Callback<()>,
}

impl ZonesContext {
    /// Look up a zone by id.
    pub fn zone(&self, id: &str) -> Option<Surface> {
        self.zones
            .with(|zones| zones.iter().find(|zone| zone.id == id).cloned())
    }

    /// The zone writes currently target: the focused zone when it still
    /// exists in the active scene, otherwise the primary LED zone.
    pub fn target_zone(&self) -> Option<Surface> {
        let focused = self
            .focused_zone
            .get()
            .and_then(|id| self.zone(&id))
            .filter(|zone| zone.kind == SurfaceKind::Light);
        focused.or_else(|| self.primary_zone())
    }

    /// The primary (Default) LED zone of the active scene.
    pub fn primary_zone(&self) -> Option<Surface> {
        self.led_zones.with(|zones| {
            zones
                .iter()
                .find(|zone| zone.role == hypercolor_types::scene::ZoneRole::Primary)
                .or_else(|| zones.first())
                .cloned()
        })
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
    pub control_values: std::collections::HashMap<String, hypercolor_types::effect::ControlValue>,
    pub preset_id: Option<String>,
    /// `If-Match` token for the zone's controls PATCH stream.
    pub controls_version: u64,
    /// `If-Match` token for the zone's layer mutations.
    pub layers_version: u64,
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
    pub active: Memo<Option<api::ActiveSceneResponse>>,
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

impl ScenesContext {
    /// Whether the user has somewhere to switch *to* — the gate for
    /// rendering scene-switcher affordances at all. True with two or
    /// more saved scenes, or with one saved scene while the ephemeral
    /// default is running (Default ↔ the saved scene is still a switch).
    pub fn has_multiple(&self) -> bool {
        let saved = self.scenes.with(Vec::len);
        if saved > 1 {
            return true;
        }
        saved == 1
            && self.active.with(|active| {
                active
                    .as_ref()
                    .is_none_or(|scene| scene.kind == hypercolor_types::scene::SceneKind::Ephemeral)
            })
    }
}

/// Build and provide [`ZonesContext`] + [`ScenesContext`]. Called once
/// from the app root, after the WebSocket manager exists.
pub fn provide_scene_contexts(
    last_scene_event: ReadSignal<Option<SceneEventHint>>,
) -> (ZonesContext, ScenesContext) {
    let active_scene_resource = LocalResource::new(api::fetch_active_scene);
    let scenes_resource = LocalResource::new(api::list_scenes);

    // Memo (not derive) so refetches that return identical state don't
    // wake every zone-aware surface in the app.
    let active_scene =
        Memo::new(move |_| active_scene_resource.get().and_then(Result::ok).flatten());
    let scenes = Memo::new(move |_| {
        scenes_resource
            .get()
            .and_then(Result::ok)
            .unwrap_or_default()
    });

    let zones = Memo::new(move |_| {
        active_scene
            .get()
            .map(|scene| surfaces_from_groups(&scene.groups))
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
            .is_some_and(|scene| led_zone_count(&scene.groups) > 1)
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

    // WS-driven freshness: any scene event refreshes the active scene
    // (except pure control patches — those arrive at slider-drag rate
    // and don't change scene structure); activation and library CRUD
    // also refresh the list.
    Effect::new(move |previous: Option<Option<SceneEventHint>>| {
        let current = last_scene_event.get();
        if previous.as_ref() == Some(&current) {
            return current;
        }
        let Some(hint) = current.as_ref() else {
            return current;
        };

        let controls_only = hint.event_type == "render_group_changed"
            && hint.render_group_change_kind
                == Some(hypercolor_types::event::ZoneChangeKind::ControlsPatched);
        if !controls_only {
            active_scene_resource.refetch();
        }
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
        focused_zone: RwSignal::new(None),
        refresh: refresh_active,
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
