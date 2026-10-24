//! Which zone the main preview follows, and what acts on it.

use std::collections::HashMap;

use hypercolor_types::scene::ZoneRole;

use hypercolor_ui::apply_target::ApplyTarget;
use hypercolor_ui::preview_zone::{PlayerTarget, player_target, preview_zone_for};
use hypercolor_ui::zones::ZoneEffectState;
use hypercolor_ui::zones::surface::{Surface, SurfaceKind};

fn zone(id: &str, role: ZoneRole, effect_id: &str) -> ZoneEffectState {
    ZoneEffectState {
        zone: Surface {
            id: id.to_owned(),
            name: id.to_owned(),
            kind: SurfaceKind::Light,
            enabled: true,
            role,
            color: None,
            display_device_id: None,
            layer_ids: vec![format!("{id}-layer")],
            top_layer: Some(effect_id.to_owned()),
        },
        effect_id: Some(effect_id.to_owned()),
        effect_name: Some(effect_id.to_owned()),
        effect_category: None,
        control_values: HashMap::new(),
        preset_id: None,
        revision: 1,
        layer_labels: vec![effect_id.to_owned()],
    }
}

#[test]
fn a_single_zone_scene_keeps_the_scene_canvas() {
    // One LED zone: the scene canvas already is that zone.
    assert_eq!(
        preview_zone_for(false, true, Some("roli"), Some("default")),
        None
    );
    assert_eq!(preview_zone_for(false, false, None, Some("default")), None);
}

#[test]
fn a_page_with_a_zone_picker_shows_the_focused_zone() {
    assert_eq!(
        preview_zone_for(true, true, Some("roli"), Some("default")),
        Some("roli".to_owned())
    );
}

#[test]
fn a_zone_picker_with_nothing_focused_shows_the_default_zone() {
    assert_eq!(
        preview_zone_for(true, true, None, Some("default")),
        Some("default".to_owned())
    );
}

#[test]
fn a_page_without_a_zone_picker_always_shows_the_default_zone() {
    // The sidebar thumbnail has no picker, so a zone focused elsewhere
    // never leaks into it.
    assert_eq!(
        preview_zone_for(true, false, Some("roli"), Some("default")),
        Some("default".to_owned())
    );
}

#[test]
fn a_multi_zone_scene_without_a_default_zone_falls_back_to_the_scene_canvas() {
    assert_eq!(preview_zone_for(true, false, None, None), None);
}

#[test]
fn a_player_beside_the_default_zone_steps_the_default_zone() {
    // ROLI is focused on another page, but the thumbnail shows the
    // Default zone, so its player steps and applies there. It steps from
    // the primary effect, which an apply updates at once, not from the
    // zone list, which lags until the scene refetch lands.
    let shown = zone("default", ZoneRole::Primary, "touch-ripple");
    assert_eq!(
        player_target(
            Some(&shown),
            Some("digital-rain"),
            &ApplyTarget::Zone("roli".to_owned()),
        ),
        PlayerTarget {
            current_effect: Some("digital-rain".to_owned()),
            apply_target: ApplyTarget::Primary,
        }
    );
}

#[test]
fn a_player_beside_a_focused_zone_steps_from_that_zone_s_effect() {
    let shown = zone("roli", ZoneRole::Custom, "bubble-garden");
    assert_eq!(
        player_target(
            Some(&shown),
            Some("digital-rain"),
            &ApplyTarget::Zone("roli".to_owned()),
        ),
        PlayerTarget {
            current_effect: Some("bubble-garden".to_owned()),
            apply_target: ApplyTarget::Zone("roli".to_owned()),
        }
    );
}

#[test]
fn a_player_beside_the_scene_canvas_keeps_the_quick_apply_target() {
    // A single-zone scene previews its scene canvas, which already is
    // its one zone: nothing about the player changes.
    assert_eq!(
        player_target(None, Some("digital-rain"), &ApplyTarget::AllZones),
        PlayerTarget {
            current_effect: Some("digital-rain".to_owned()),
            apply_target: ApplyTarget::AllZones,
        }
    );
}
