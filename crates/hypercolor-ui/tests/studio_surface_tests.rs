//! Contract tests for the Studio §8 surface model.

use std::collections::HashMap;

use hypercolor_types::device::DeviceId;
use hypercolor_types::effect::EffectId;
use hypercolor_types::layer::{SceneLayer, SceneLayerId};
use hypercolor_types::scene::{DisplayFaceTarget, ZoneId, ZoneRole};
use uuid::Uuid;

use hypercolor_ui::api::ZoneResource;
use hypercolor_ui::pages::studio::surface::{
    Surface, SurfaceKind, led_zone_count, now_playing, selected_screen_device_id,
    surfaces_from_zones,
};

fn zone_resource(
    name: &str,
    role: ZoneRole,
    display_target: Option<DisplayFaceTarget>,
) -> ZoneResource {
    ZoneResource {
        id: ZoneId::new(),
        name: name.to_owned(),
        description: None,
        brightness: 1.0,
        enabled: true,
        color: None,
        display_target,
        role,
        members: Vec::new(),
        layout: None,
        layers: Vec::new(),
        view_of: None,
    }
}

#[test]
fn a_named_primary_zone_shows_its_authored_name() {
    let surfaces = surfaces_from_zones(&[zone_resource("Zone A", ZoneRole::Primary, None)]);

    assert_eq!(surfaces.len(), 1);
    let surface = &surfaces[0];
    assert_eq!(surface.name, "Zone A");
    assert_eq!(surface.kind, SurfaceKind::Light);
    assert_eq!(surface.display_device_id, None);
}

#[test]
fn multiple_led_zones_keep_their_authored_names() {
    let surfaces = surfaces_from_zones(&[
        zone_resource("Desk Zone", ZoneRole::Primary, None),
        zone_resource("Shelf Zone", ZoneRole::Custom, None),
    ]);

    // Every LED zone keeps its authored name, in scene order.
    let names: Vec<&str> = surfaces.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["Desk Zone", "Shelf Zone"]);
    assert!(surfaces.iter().all(|s| s.kind == SurfaceKind::Light));
}

#[test]
fn display_zone_becomes_a_screen_carrying_its_device_id() {
    let device_id = DeviceId::new();
    let target = DisplayFaceTarget::new(device_id);
    let surfaces = surfaces_from_zones(&[zone_resource(
        "Corsair LCD",
        ZoneRole::Display,
        Some(target),
    )]);

    assert_eq!(surfaces.len(), 1);
    let surface = &surfaces[0];
    assert_eq!(surface.kind, SurfaceKind::Screen);
    assert_eq!(surface.name, "Corsair LCD");
    assert_eq!(surface.display_device_id, Some(device_id.to_string()));
}

#[test]
fn display_zone_without_a_target_has_no_preview_device() {
    let surfaces = surfaces_from_zones(&[zone_resource("Pending Face", ZoneRole::Display, None)]);

    let surface = &surfaces[0];
    assert_eq!(surface.kind, SurfaceKind::Screen);
    assert_eq!(surface.display_device_id, None);
}

#[test]
fn a_surface_carries_backing_zone_live_layer_ids() {
    let mut zone = zone_resource("Zone A", ZoneRole::Primary, None);
    let first = SceneLayer::from_effect(
        SceneLayerId::new(),
        EffectId::new(Uuid::nil()),
        HashMap::new(),
        HashMap::new(),
        None,
    );
    let second = SceneLayer::from_effect(
        SceneLayerId::new(),
        EffectId::new(Uuid::nil()),
        HashMap::new(),
        HashMap::new(),
        None,
    );
    let expected = vec![first.id.to_string(), second.id.to_string()];
    zone.layers = vec![first, second];

    // The surface mirrors the zone's live layer ids in stack order, the
    // set the degraded check filters streamed health against.
    let surfaces = surfaces_from_zones(&[zone]);
    assert_eq!(surfaces[0].layer_ids, expected);
}

#[test]
fn led_and_display_zones_split_into_lights_and_screens() {
    let surfaces = surfaces_from_zones(&[
        zone_resource("Zone A", ZoneRole::Primary, None),
        zone_resource(
            "AIO Screen",
            ZoneRole::Display,
            Some(DisplayFaceTarget::new(DeviceId::new())),
        ),
    ]);

    let lights = surfaces
        .iter()
        .filter(|s| s.kind == SurfaceKind::Light)
        .count();
    let screens = surfaces
        .iter()
        .filter(|s| s.kind == SurfaceKind::Screen)
        .count();
    assert_eq!((lights, screens), (1, 1));
    // The lone LED zone keeps its authored name; the screen is separate.
    assert_eq!(surfaces[0].name, "Zone A");
}

#[test]
fn a_renamed_primary_zone_shows_its_typed_name_when_multi_zone() {
    let surfaces = surfaces_from_zones(&[
        zone_resource("Desk Strips", ZoneRole::Primary, None),
        zone_resource("Case Fans", ZoneRole::Custom, None),
    ]);
    // A multi-zone Primary zone keeps the user's typed name.
    assert_eq!(surfaces[0].name, "Desk Strips");
}

#[test]
fn an_unnamed_primary_zone_reads_as_main() {
    // The daemon seeds the primary zone as "Primary"; until renamed, the
    // rail shows "Main" rather than leaking that internal label.
    let surfaces = surfaces_from_zones(&[
        zone_resource("Primary", ZoneRole::Primary, None),
        zone_resource("Case Fans", ZoneRole::Custom, None),
    ]);
    assert_eq!(surfaces[0].name, "Main");
    // The relabel holds at every scale; a solo unnamed zone reads the same.
    let solo = surfaces_from_zones(&[zone_resource("Primary", ZoneRole::Primary, None)]);
    assert_eq!(solo[0].name, "Main");
}

#[test]
fn a_surface_carries_backing_zone_role_and_accent_color() {
    let mut zone = zone_resource("Case Fans", ZoneRole::Custom, None);
    zone.color = Some("#e135ff".to_owned());
    let surfaces = surfaces_from_zones(&[zone]);
    assert_eq!(surfaces[0].role, ZoneRole::Custom);
    assert_eq!(surfaces[0].color.as_deref(), Some("#e135ff"));
}

#[test]
fn led_zone_count_excludes_display_zones() {
    let zones = [
        zone_resource("Default", ZoneRole::Primary, None),
        zone_resource("Case Fans", ZoneRole::Custom, None),
        zone_resource(
            "AIO Screen",
            ZoneRole::Display,
            Some(DisplayFaceTarget::new(DeviceId::new())),
        ),
    ];
    assert_eq!(led_zone_count(&zones), 2);
}

fn surface(kind: SurfaceKind, top_layer: Option<&str>) -> Surface {
    Surface {
        id: "surface".to_owned(),
        name: "Surface".to_owned(),
        kind,
        enabled: true,
        role: match kind {
            SurfaceKind::Screen => ZoneRole::Display,
            SurfaceKind::Light => ZoneRole::Custom,
        },
        color: None,
        display_device_id: None,
        layer_ids: Vec::new(),
        top_layer: top_layer.map(str::to_owned),
    }
}

#[test]
fn now_playing_prefers_the_scene_top_layer() {
    let screen = surface(SurfaceKind::Screen, Some("Neon Clock"));
    let playing = now_playing(Some(&screen), Some("Sensor Grid"));
    assert_eq!(playing.label, "Neon Clock");
    assert!(!playing.is_default_face);
}

#[test]
fn now_playing_names_the_default_face_on_an_empty_screen() {
    let screen = surface(SurfaceKind::Screen, None);
    let playing = now_playing(Some(&screen), Some("Neon Clock"));
    assert_eq!(playing.label, "Neon Clock");
    assert!(playing.is_default_face);

    let blank = now_playing(Some(&screen), Some("   "));
    assert_eq!(blank.label, "No layers");
    assert!(!blank.is_default_face);
}

#[test]
fn now_playing_ignores_default_faces_for_lights_and_no_selection() {
    let light = surface(SurfaceKind::Light, None);
    let playing = now_playing(Some(&light), Some("Neon Clock"));
    assert_eq!(playing.label, "No layers");
    assert!(!playing.is_default_face);

    let none = now_playing(None, Some("Neon Clock"));
    assert_eq!(none.label, "No layers");
    assert!(!none.is_default_face);
}

#[test]
fn selected_screen_device_id_resolves_only_bound_display_zones() {
    let device_id = DeviceId::new();
    let zones = vec![
        zone_resource("Default zone", ZoneRole::Primary, None),
        zone_resource(
            "Screen",
            ZoneRole::Display,
            Some(DisplayFaceTarget::new(device_id)),
        ),
        zone_resource("Unbound screen", ZoneRole::Display, None),
    ];

    assert_eq!(
        selected_screen_device_id(&zones, &zones[1].id.to_string()),
        Some(device_id.to_string())
    );
    assert_eq!(
        selected_screen_device_id(&zones, &zones[0].id.to_string()),
        None
    );
    assert_eq!(
        selected_screen_device_id(&zones, &zones[2].id.to_string()),
        None
    );
    assert_eq!(selected_screen_device_id(&zones, "missing"), None);
}

// ── The layer a zone is known by ─────────────────────────────────────────

fn effect_layer(name: &str, scope: Option<&[&str]>) -> hypercolor_types::layer::SceneLayer {
    let mut layer = hypercolor_types::layer::SceneLayer::from_effect(
        hypercolor_types::layer::SceneLayerId::new(),
        hypercolor_types::effect::EffectId::new(uuid::Uuid::new_v4()),
        std::collections::HashMap::new(),
        std::collections::HashMap::new(),
        None,
    );
    layer.name = Some(name.to_owned());
    layer.scope = scope.map(|ids| hypercolor_types::layer::LayerScope {
        outputs: ids.iter().map(|id| (*id).to_owned()).collect(),
    });
    layer
}

#[test]
fn a_zone_is_known_by_its_whole_zone_layer_not_one_on_a_single_device() {
    let mut zone = zone_resource("Main", ZoneRole::Primary, None);
    zone.layers = vec![
        effect_layer("Bubbles", None),
        effect_layer("Ripples", Some(&["keys"])),
    ];

    let surfaces = surfaces_from_zones(std::slice::from_ref(&zone));
    assert_eq!(surfaces[0].top_layer.as_deref(), Some("Bubbles"));

    let bubbles = zone.layers[0].clone();
    let known = hypercolor_ui::api::zone_effect(&zone).expect("the zone plays an effect");
    match &bubbles.source {
        hypercolor_types::layer::LayerSource::Effect { effect_id, .. } => {
            assert_eq!(known.effect_id, *effect_id);
        }
        other => panic!("expected an effect layer, found {other:?}"),
    }
}

#[test]
fn stacked_whole_zone_layers_are_still_known_by_the_top_one() {
    let mut zone = zone_resource("Main", ZoneRole::Primary, None);
    zone.layers = vec![effect_layer("Bubbles", None), effect_layer("Haze", None)];
    let surfaces = surfaces_from_zones(std::slice::from_ref(&zone));
    assert_eq!(surfaces[0].top_layer.as_deref(), Some("Haze"));
}

#[test]
fn a_zone_with_only_limited_layers_is_known_by_the_top_of_them() {
    let mut zone = zone_resource("Main", ZoneRole::Primary, None);
    zone.layers = vec![
        effect_layer("Ripples", Some(&["keys"])),
        effect_layer("Glow", Some(&["mouse"])),
    ];
    let surfaces = surfaces_from_zones(std::slice::from_ref(&zone));
    assert_eq!(surfaces[0].top_layer.as_deref(), Some("Glow"));
    // The caption names what is there to see. The zone's effect is another
    // question: a layer on some outputs is never it, so there is none.
    assert!(hypercolor_ui::api::zone_effect(&zone).is_none());
}

/// A zone whose base effect also plays, as a second layer, on the keys
/// alone: the same effect twice, the limited copy on top.
fn zone_playing_one_effect_twice() -> (ZoneResource, SceneLayer, SceneLayer) {
    let mut zone = zone_resource("Main", ZoneRole::Primary, None);
    let base = effect_layer("Aurora", None);
    let mut copy = effect_layer("Aurora on the keys", Some(&["keys"]));
    copy.source = base.source.clone();
    zone.layers = vec![base.clone(), copy.clone()];
    (zone, base, copy)
}

fn effect_id_of(layer: &SceneLayer) -> String {
    match &layer.source {
        hypercolor_types::layer::LayerSource::Effect { effect_id, .. } => effect_id.to_string(),
        other => panic!("expected an effect layer, found {other:?}"),
    }
}

fn scene_of(zone: ZoneResource) -> hypercolor_ui::api::SceneDocument {
    let mut scene: hypercolor_ui::api::SceneDocument = serde_json::from_value(serde_json::json!({
        "id": "33333333-3333-4333-8333-333333333333", "name": "Desk", "kind": "named",
        "is_default": false, "revision": 4, "zones": [],
    }))
    .expect("valid scene fixture");
    scene.zones = vec![zone];
    scene
}

#[test]
fn a_write_to_the_zones_effect_addresses_its_whole_zone_layer() {
    // A preset reset, a control patch, or a preset apply to "the zone's
    // effect" must land on the layer the zone is known by, not on a copy
    // of the same effect limited to one device above it.
    let (zone, base, copy) = zone_playing_one_effect_twice();
    let effect_id = effect_id_of(&base);
    let zone_id = zone.id.to_string();
    let scene = scene_of(zone);

    let (target, fence) =
        hypercolor_ui::zones::effect_target_in_scene(&scene, &effect_id, Some(&zone_id))
            .expect("the zone plays the effect");
    assert_eq!(target.layer_id, base.id.to_string());
    assert_ne!(target.layer_id, copy.id.to_string());
    assert_eq!(fence.revision, 4);

    // The primary zone resolves the same way when no zone is named.
    let (target, _) = hypercolor_ui::zones::effect_target_in_scene(&scene, &effect_id, None)
        .expect("the primary zone plays the effect");
    assert_eq!(target.layer_id, base.id.to_string());
}

#[test]
fn the_layer_a_write_answers_with_is_the_whole_zone_one() {
    // An apply or a reset replaces the zone's whole-zone layer, so the
    // layer its answer names is that one, minted again under the copy.
    let (zone, base, _) = zone_playing_one_effect_twice();
    let target = hypercolor_ui::api::effect_target_from_zone(&effect_id_of(&base), &zone)
        .expect("the answer holds the layer");
    assert_eq!(target.layer_id, base.id.to_string());
}

#[test]
fn an_effect_only_limited_layers_play_is_not_the_zones_effect() {
    // A write to "the zone's effect" never lands on a layer limited to
    // some outputs, a keyboard's ripples or a screen's face, however many
    // of them play it.
    let mut zone = zone_resource("Main", ZoneRole::Primary, None);
    let keys = effect_layer("Ripples", Some(&["keys"]));
    let mut mouse = effect_layer("Ripples", Some(&["mouse"]));
    mouse.source = keys.source.clone();
    zone.layers = vec![effect_layer("Base", None), keys.clone(), mouse.clone()];
    let zone_id = zone.id.to_string();
    let scene = scene_of(zone);

    assert!(
        hypercolor_ui::zones::effect_target_in_scene(&scene, &effect_id_of(&keys), Some(&zone_id))
            .is_none()
    );

    // An effect no layer plays resolves to nothing.
    let other = hypercolor_types::effect::EffectId::new(uuid::Uuid::new_v4()).to_string();
    assert!(hypercolor_ui::zones::effect_target_in_scene(&scene, &other, Some(&zone_id)).is_none());
}
