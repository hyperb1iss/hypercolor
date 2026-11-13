//! Screens as outputs: where a screen's layers live, and how they reach
//! the panel-resolution target the engine renders them in.
//!
//! A screen is an output a zone places. Its stack is the zone's layers
//! scoped only to screens, in stack order. The render thread still
//! composes a screen at panel size in a display zone of its own, so the
//! scene manager lowers each screen's stack into one before publishing:
//! the authored zone goes out without its screen layers, and one derived
//! display zone goes out per screen that has something to show.
//!
//! A stored display zone is the older spelling of the same stack, and the
//! only one for a screen no zone places, which has no output to scope a
//! layer to. It passes through lowering as it is, and
//! [`convert_legacy_display_zones`] rewrites it into screen layers when the
//! screen is placed and the result renders the same frames.

use std::collections::{BTreeMap, HashSet};

use hypercolor_types::device::DeviceId;
use hypercolor_types::layer::{BlendMode, LayerScope, LayerSource, SceneLayer, SceneLayerId};
use hypercolor_types::scene::{DisplayFaceTarget, Scene, SceneId, Zone, ZoneId, ZoneRole};
use hypercolor_types::spatial::{EdgeBehavior, Output, SamplingMode, SpatialLayout};
use uuid::Uuid;

use crate::spatial::is_display_zone;

/// One screen this rig knows: the physical device behind a layout device
/// id, and its panel.
#[derive(Debug, Clone, PartialEq)]
pub struct ScreenSurface {
    /// The physical device that owns the panel.
    pub device_id: DeviceId,
    /// Whether the device can take frames now. A screen that is known but
    /// away keeps its stack, and is listed only while it has one.
    pub connected: bool,
    /// The device's name, which names the screen.
    pub name: String,
    /// Panel width in pixels.
    pub width: u32,
    /// Panel height in pixels.
    pub height: u32,
}

/// Every screen the rig knows, keyed by the layout device id its output
/// carries. One physical device may answer to several layout ids.
pub type ScreenSurfaces = BTreeMap<String, ScreenSurface>;

/// The id of the display zone a screen's stack is lowered into, and of the
/// view that stands for the screen on the wire.
///
/// It depends only on the scene and the device, so it survives every
/// refresh, a restart, and a move of the screen between zones, and a moved
/// screen's face keeps its renderer.
#[must_use]
pub fn screen_zone_id(scene_id: SceneId, device_id: DeviceId) -> ZoneId {
    ZoneId(Uuid::new_v5(&scene_id.0, device_id.as_uuid().as_bytes()))
}

/// The panel-resolution canvas one screen's stack composes in.
#[must_use]
pub fn screen_face_layout(
    device_id: DeviceId,
    device_name: &str,
    width: u32,
    height: u32,
) -> SpatialLayout {
    SpatialLayout {
        id: format!("display-face:{device_id}"),
        name: format!("{device_name} Display Face"),
        description: Some(format!("Native-resolution face canvas for {device_name}")),
        canvas_width: width,
        canvas_height: height,
        zones: Vec::new(),
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        version: 1,
    }
}

/// Clamp a face's composition to what the renderer can honor: a face that
/// does not blend covers its backdrop outright, so a partial opacity would
/// describe a composite that never happens.
#[must_use]
pub fn normalize_face_target(target: DisplayFaceTarget) -> DisplayFaceTarget {
    let mut target = target.normalized();
    if !target.clone().blends_with_effect() {
        target.opacity = 1.0;
    }
    target
}

/// Whether a layer is a screen layer of its zone: its scope has at least
/// one live output, and every live output in it is a screen.
#[must_use]
pub fn is_screen_layer(zone: &Zone, layer: &SceneLayer) -> bool {
    let Some(scope) = &layer.scope else {
        return false;
    };
    let mut live = zone.scoped_outputs(scope).peekable();
    live.peek().is_some() && live.all(is_display_zone)
}

fn surface_of(surfaces: &ScreenSurfaces, device_id: DeviceId) -> Option<&ScreenSurface> {
    surfaces
        .values()
        .find(|surface| surface.device_id == device_id)
}

/// The distinct known screens, in device order.
pub(super) fn known_screens(surfaces: &ScreenSurfaces) -> Vec<&ScreenSurface> {
    let mut seen = HashSet::new();
    let mut screens = surfaces
        .values()
        .filter(|surface| seen.insert(surface.device_id))
        .collect::<Vec<_>>();
    screens.sort_by_key(|surface| surface.device_id.as_uuid());
    screens
}

/// The zone and output that place a screen in a scene.
#[must_use]
pub fn screen_placement<'a>(
    scene: &'a Scene,
    surfaces: &ScreenSurfaces,
    device_id: DeviceId,
) -> Option<(&'a Zone, &'a Output)> {
    scene
        .zones
        .iter()
        .filter(|zone| zone.display_target.is_none())
        .find_map(|zone| {
            zone.layout
                .zones
                .iter()
                .find(|output| output_is_screen_of(surfaces, output, device_id))
                .map(|output| (zone, output))
        })
}

fn output_is_screen_of(surfaces: &ScreenSurfaces, output: &Output, device_id: DeviceId) -> bool {
    is_display_zone(output)
        && surfaces
            .get(&output.device_id)
            .is_some_and(|surface| surface.device_id == device_id)
}

/// Where a screen's stack lives in a scene.
#[derive(Debug, Clone, Copy)]
pub enum ScreenStack<'a> {
    /// A stored display zone that has not been converted. While it exists
    /// it is the screen's whole stack.
    Legacy(&'a Zone),
    /// The screen is an output of `zone`, and its stack is that zone's
    /// screen layers that name it. The stack may be empty.
    Placed { zone: &'a Zone, output: &'a Output },
    /// No zone places the screen and no display zone is stored for it.
    Unplaced,
}

/// Resolve where a screen's stack lives.
#[must_use]
pub fn screen_stack<'a>(
    scene: &'a Scene,
    surfaces: &ScreenSurfaces,
    device_id: DeviceId,
) -> ScreenStack<'a> {
    if let Some(zone) = scene.display_zone_for(device_id) {
        return ScreenStack::Legacy(zone);
    }
    match screen_placement(scene, surfaces, device_id) {
        Some((zone, output)) => ScreenStack::Placed { zone, output },
        None => ScreenStack::Unplaced,
    }
}

impl<'a> ScreenStack<'a> {
    /// The screen's layers, bottom to top, as authored.
    #[must_use]
    pub fn layers(self) -> Vec<&'a SceneLayer> {
        match self {
            Self::Legacy(zone) => zone.layers.iter().collect(),
            Self::Placed { zone, output } => zone
                .layers
                .iter()
                .filter(|layer| is_screen_layer(zone, layer))
                .filter(|layer| {
                    layer
                        .scope
                        .as_ref()
                        .is_some_and(|scope| scope.contains(&output.id))
                })
                .collect(),
            Self::Unplaced => Vec::new(),
        }
    }

    /// Whether the zone that holds the stack is enabled.
    #[must_use]
    pub fn zone_enabled(self) -> bool {
        match self {
            Self::Legacy(zone) | Self::Placed { zone, .. } => zone.enabled,
            Self::Unplaced => false,
        }
    }

    /// Whether the scene holds a face for the screen: a layer with an
    /// effect source, enabled or not, in a zone enabled or not.
    #[must_use]
    pub fn assigned(self) -> bool {
        self.layers()
            .iter()
            .any(|layer| matches!(layer.source, LayerSource::Effect { .. }))
    }

    /// Whether the scene's stack draws on the screen: at least one enabled
    /// layer in an enabled zone.
    #[must_use]
    pub fn rendering(self) -> bool {
        self.zone_enabled() && self.layers().iter().any(|layer| layer.enabled)
    }

    /// How the stack meets its backdrop: the stored target of a legacy
    /// zone, or the blend and opacity of the bottom enabled layer. An
    /// empty stack reads as a fresh target.
    #[must_use]
    pub fn target(self, device_id: DeviceId) -> DisplayFaceTarget {
        if let Self::Legacy(zone) = self
            && let Some(target) = &zone.display_target
        {
            return target.clone();
        }
        let layers = self.layers();
        let bottom = layers
            .iter()
            .find(|layer| layer.enabled)
            .or_else(|| layers.first());
        bottom.map_or_else(
            || DisplayFaceTarget::new(device_id),
            |layer| {
                normalize_face_target(DisplayFaceTarget {
                    device_id,
                    blend_mode: layer.blend,
                    opacity: layer.opacity,
                })
            },
        )
    }
}

/// The zone a screen is seen as on the wire: one `Display` zone per
/// connected screen, whether or not it has a stack, and per screen that is
/// away while it still has one.
#[derive(Debug, Clone, PartialEq)]
pub struct ScreenView {
    /// The physical device.
    pub device_id: DeviceId,
    /// The view, shaped like a stored display zone.
    pub zone: Zone,
    /// The zone that places the screen and the screen's output in it.
    /// Absent for a legacy zone and for a screen no zone places.
    pub owner: Option<(ZoneId, String)>,
}

/// Build the view of one screen.
#[must_use]
pub fn screen_view(
    scene: &Scene,
    surfaces: &ScreenSurfaces,
    device_id: DeviceId,
) -> Option<ScreenView> {
    let surface = surface_of(surfaces, device_id)?;
    let stack = screen_stack(scene, surfaces, device_id);
    let layout = screen_face_layout(device_id, &surface.name, surface.width, surface.height);
    if let ScreenStack::Legacy(zone) = stack {
        let mut zone = zone.clone();
        zone.layout = layout;
        return Some(ScreenView {
            device_id,
            zone,
            owner: None,
        });
    }
    let (owner, versions) = match stack {
        ScreenStack::Placed { zone, output } => (
            Some((zone.id, output.id.clone())),
            (zone.controls_version, zone.layers_version),
        ),
        _ => (None, (0, 0)),
    };
    let layers = stack
        .layers()
        .into_iter()
        .cloned()
        .map(|mut layer| {
            layer.scope = None;
            layer
        })
        .collect();
    Some(ScreenView {
        device_id,
        zone: Zone {
            id: screen_zone_id(scene.id, device_id),
            name: surface.name.clone(),
            description: None,
            layers,
            layout,
            brightness: 1.0,
            enabled: true,
            color: None,
            display_target: Some(stack.target(device_id)),
            role: ZoneRole::Display,
            controls_version: versions.0,
            layers_version: versions.1,
        },
        owner,
    })
}

/// The views a scene lists: every connected screen that has no stored
/// display zone, and every screen that is away but still has a stack.
#[must_use]
pub fn screen_views(scene: &Scene, surfaces: &ScreenSurfaces) -> Vec<ScreenView> {
    known_screens(surfaces)
        .into_iter()
        .filter(|surface| scene.display_zone_for(surface.device_id).is_none())
        .filter_map(|surface| {
            screen_view(scene, surfaces, surface.device_id)
                .filter(|view| surface.connected || !view.zone.layers.is_empty())
        })
        .collect()
}

/// Lower a scene's zones into the list the render thread consumes.
///
/// Each authored zone goes out without its screen layers, its canvas
/// layers and their scopes intact. Each screen with at least one enabled
/// screen layer, an enabled owning zone and a known surface gets one
/// derived display zone, connected or not, as a stored display zone always
/// reached the render list. A stored display zone passes through, with its
/// canvas taken from the surface table when the panel is known. A zone
/// with no scoped layer is cloned as it is, so a rig with no faces pays
/// nothing for any of this.
#[must_use]
pub(super) fn lower_zones(scene: &Scene, surfaces: &ScreenSurfaces) -> Vec<Zone> {
    let mut zones = Vec::with_capacity(scene.zones.len());
    let mut derived: Vec<Zone> = Vec::new();
    for zone in &scene.zones {
        if let Some(target) = &zone.display_target {
            let mut zone = zone.clone();
            if let Some(surface) = surface_of(surfaces, target.device_id) {
                zone.layout = screen_face_layout(
                    target.device_id,
                    &surface.name,
                    surface.width,
                    surface.height,
                );
            }
            zones.push(zone);
            continue;
        }
        if zone.layers.iter().all(|layer| layer.scope.is_none())
            || !zone.layers.iter().any(|layer| is_screen_layer(zone, layer))
        {
            zones.push(zone.clone());
            continue;
        }
        let mut canvas = zone.clone();
        canvas.layers.retain(|layer| !is_screen_layer(zone, layer));
        zones.push(canvas);
        if !zone.enabled {
            continue;
        }
        for output in zone
            .layout
            .zones
            .iter()
            .filter(|output| is_display_zone(output))
        {
            let Some(surface) = surfaces.get(&output.device_id) else {
                continue;
            };
            let device_id = surface.device_id;
            if scene.display_zone_for(device_id).is_some()
                || derived.iter().any(|zone| {
                    zone.display_target
                        .as_ref()
                        .is_some_and(|target| target.device_id == device_id)
                })
            {
                continue;
            }
            let stack = ScreenStack::Placed { zone, output }.layers();
            if let Some(lowered) = derived_zone(scene.id, zone, surface, &stack) {
                derived.push(lowered);
            }
        }
    }
    zones.extend(derived);
    zones
}

/// Build the display zone one screen's stack renders in.
///
/// Scope is resolved by the caller and cleared here, because the panel
/// layout has no outputs and a scope left on a clone would make it
/// dormant. The bottom enabled layer's blend and opacity become the
/// target, and that clone is set to replace at full opacity so the opacity
/// is applied once. Clones keep their authored ids, so control patches and
/// effect slots address them.
fn derived_zone(
    scene_id: SceneId,
    owner: &Zone,
    surface: &ScreenSurface,
    stack: &[&SceneLayer],
) -> Option<Zone> {
    let bottom = stack.iter().position(|layer| layer.enabled)?;
    let target = normalize_face_target(DisplayFaceTarget {
        device_id: surface.device_id,
        blend_mode: stack[bottom].blend,
        opacity: stack[bottom].opacity,
    });
    let layers = stack
        .iter()
        .enumerate()
        .map(|(index, layer)| {
            let mut layer = (*layer).clone();
            layer.scope = None;
            if index == bottom {
                layer.blend = BlendMode::Replace;
                layer.opacity = 1.0;
            }
            layer
        })
        .collect();
    Some(Zone {
        id: screen_zone_id(scene_id, surface.device_id),
        name: surface.name.clone(),
        description: None,
        layers,
        layout: screen_face_layout(
            surface.device_id,
            &surface.name,
            surface.width,
            surface.height,
        ),
        brightness: 1.0,
        enabled: true,
        color: None,
        display_target: Some(target),
        role: ZoneRole::Display,
        controls_version: owner.controls_version,
        layers_version: owner.layers_version,
    })
}

/// Why a stored display zone was left in its legacy spelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegacyReason {
    /// The scene is snapshot-locked.
    SceneLocked,
    /// The zone's device is not a screen this rig knows.
    DeviceUnknown,
    /// The zone is disabled.
    ZoneDisabled,
    /// The zone's brightness is not 1.
    ZoneBrightness,
    /// The first layer is disabled.
    FirstLayerDisabled,
    /// The first layer's opacity is not 1.
    FirstLayerOpacity,
    /// The stored target is one the renderer would clamp.
    TargetNotNormalized,
    /// The zone that places the screen is disabled, so the face would stop.
    OwnerDisabled,
    /// No zone places the screen, so there is no output to scope its
    /// layers to. Its backdrop comes from wherever the render thread finds
    /// its device, which a box of the scene manager's choosing would move.
    ScreenUnplaced,
    /// The owning zone already uses one of the stack's layer ids.
    LayerIdTaken,
    /// The owning zone already holds screen layers for this screen. They
    /// are covered while the display zone exists, and would join its stack.
    OwnerHoldsScreenLayers,
}

/// What converting one scene's stored display zones did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ScreenConversion {
    /// Display zones rewritten into screen layers: the removed zone, and
    /// the zone that now holds its layers.
    pub converted: Vec<(Zone, ZoneId)>,
    /// Layerless display zones that were removed.
    pub removed: Vec<Zone>,
    /// Display zones left as stored, and why.
    pub legacy: Vec<(ZoneId, LegacyReason)>,
}

impl ScreenConversion {
    /// Whether the scene was changed at all.
    #[must_use]
    pub fn changed(&self) -> bool {
        !self.converted.is_empty() || !self.removed.is_empty()
    }
}

/// Rewrite a scene's stored display zones into screen layers.
///
/// A zone converts only when lowering the result rebuilds exactly the
/// zone that was stored, so the screen shows the same frames: the zone is
/// enabled at brightness 1, its first layer is enabled at opacity 1, its
/// target is one the renderer honors as written, a zone places its screen
/// and is enabled, and that zone holds no screen layer for the screen yet.
/// Then its layers move to the top of that zone in order, each scoped to
/// the screen, and the first takes the target's blend and opacity. A
/// layerless zone is removed. Every other zone stays as stored and is
/// reported with its reason. Running it again changes nothing.
pub fn convert_legacy_display_zones(
    scene: &mut Scene,
    surfaces: &ScreenSurfaces,
) -> ScreenConversion {
    let mut report = ScreenConversion::default();
    let legacy_ids = scene
        .zones
        .iter()
        .filter(|zone| zone.display_target.is_some())
        .map(|zone| zone.id)
        .collect::<Vec<_>>();
    if legacy_ids.is_empty() {
        return report;
    }
    if scene.blocks_runtime_mutation() {
        report.legacy = legacy_ids
            .into_iter()
            .map(|id| (id, LegacyReason::SceneLocked))
            .collect();
        return report;
    }
    for zone_id in legacy_ids {
        let Some(index) = scene.zones.iter().position(|zone| zone.id == zone_id) else {
            continue;
        };
        if scene.zones[index].layers.is_empty() {
            report.removed.push(scene.zones.remove(index));
        } else if let Some(reason) = conversion_blocker(scene, index, surfaces) {
            report.legacy.push((zone_id, reason));
        } else {
            convert_zone(scene, index, surfaces, &mut report);
        }
    }
    if report.changed() {
        scene.zones_revision = scene.zones_revision.saturating_add(1);
    }
    report
}

fn conversion_blocker(
    scene: &Scene,
    index: usize,
    surfaces: &ScreenSurfaces,
) -> Option<LegacyReason> {
    let zone = &scene.zones[index];
    let target = zone.display_target.as_ref()?;
    if surface_of(surfaces, target.device_id).is_none() {
        return Some(LegacyReason::DeviceUnknown);
    }
    let Some((owner, output)) = screen_placement(scene, surfaces, target.device_id) else {
        return Some(LegacyReason::ScreenUnplaced);
    };
    let first = zone.layers.first()?;
    if !zone.enabled {
        return Some(LegacyReason::ZoneDisabled);
    }
    if (zone.brightness - 1.0).abs() > f32::EPSILON {
        return Some(LegacyReason::ZoneBrightness);
    }
    if !first.enabled {
        return Some(LegacyReason::FirstLayerDisabled);
    }
    if (first.opacity - 1.0).abs() > f32::EPSILON {
        return Some(LegacyReason::FirstLayerOpacity);
    }
    if normalize_face_target(target.clone()) != *target {
        return Some(LegacyReason::TargetNotNormalized);
    }
    if !owner.enabled {
        return Some(LegacyReason::OwnerDisabled);
    }
    if !(ScreenStack::Placed {
        zone: owner,
        output,
    })
    .layers()
    .is_empty()
    {
        return Some(LegacyReason::OwnerHoldsScreenLayers);
    }
    if zone
        .layers
        .iter()
        .any(|layer| owner.layers.iter().any(|held| held.id == layer.id))
    {
        return Some(LegacyReason::LayerIdTaken);
    }
    None
}

fn convert_zone(
    scene: &mut Scene,
    index: usize,
    surfaces: &ScreenSurfaces,
    report: &mut ScreenConversion,
) {
    let Some(target) = scene.zones[index].display_target.clone() else {
        return;
    };
    let Some((owner_id, output_id)) = screen_placement(scene, surfaces, target.device_id)
        .map(|(owner, output)| (owner.id, output.id.clone()))
    else {
        return;
    };
    let legacy = scene.zones.remove(index);
    let Some(owner) = scene.zones.iter_mut().find(|zone| zone.id == owner_id) else {
        scene.zones.insert(index, legacy);
        return;
    };
    for (position, layer) in legacy.layers.iter().enumerate() {
        let mut layer = layer.clone();
        layer.scope = Some(LayerScope {
            outputs: vec![output_id.clone()],
        });
        if position == 0 {
            layer.blend = target.blend_mode;
            layer.opacity = target.opacity;
        }
        owner.layers.push(layer);
    }
    owner.layers_version = owner.layers_version.saturating_add(1);
    report.converted.push((legacy, owner_id));
}

/// Take one screen out of a shared layer's scope and give it a copy of its
/// own, directly above, so a write about this screen changes no other.
///
/// Returns the copy's index. A layer already scoped to this screen alone
/// is returned as it is.
pub fn split_for_screen(zone: &mut Zone, layer_id: SceneLayerId, output_id: &str) -> Option<usize> {
    let index = zone.layers.iter().position(|layer| layer.id == layer_id)?;
    let scope = zone.layers[index].scope.as_mut()?;
    if !scope.contains(output_id) {
        return None;
    }
    if scope.outputs.len() == 1 {
        return Some(index);
    }
    scope.outputs.retain(|id| id != output_id);
    let mut copy = zone.layers[index].clone();
    copy.id = SceneLayerId::new();
    copy.scope = Some(LayerScope {
        outputs: vec![output_id.to_owned()],
    });
    zone.layers.insert(index + 1, copy);
    Some(index + 1)
}

/// Remove one screen from a zone's screen layers: layers scoped to it
/// alone are deleted, and it leaves the scope of layers it shares.
///
/// Returns whether anything changed.
pub fn clear_screen_layers(zone: &mut Zone, output_id: &str) -> bool {
    let screen_layers = zone
        .layers
        .iter()
        .filter(|layer| is_screen_layer(zone, layer))
        .filter(|layer| {
            layer
                .scope
                .as_ref()
                .is_some_and(|scope| scope.contains(output_id))
        })
        .map(|layer| layer.id)
        .collect::<Vec<_>>();
    for layer_id in &screen_layers {
        remove_screen_from_layer(zone, *layer_id, output_id);
    }
    !screen_layers.is_empty()
}

/// Remove one screen from one layer: the layer is deleted when the screen
/// was its whole scope, and narrowed otherwise.
pub fn remove_screen_from_layer(zone: &mut Zone, layer_id: SceneLayerId, output_id: &str) -> bool {
    let Some(index) = zone.layers.iter().position(|layer| layer.id == layer_id) else {
        return false;
    };
    let Some(scope) = zone.layers[index].scope.as_mut() else {
        return false;
    };
    if !scope.contains(output_id) {
        return false;
    }
    if scope.outputs.len() == 1 {
        zone.layers.remove(index);
    } else {
        scope.outputs.retain(|id| id != output_id);
    }
    true
}
