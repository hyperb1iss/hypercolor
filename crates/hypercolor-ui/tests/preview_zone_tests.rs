//! Which zone the main preview follows, and what acts on it.

use std::collections::HashMap;

use hypercolor_types::scene::ZoneRole;

use hypercolor_ui::preview_zone::{
    PlayerTarget, player_target, preview_frame_owner, preview_zone_for, settled_preview_zone,
    shown_zone_state,
};
use hypercolor_ui::zones::surface::{Surface, SurfaceKind};
use hypercolor_ui::zones::{ZoneEffectState, scene_read_settles};

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
        player_target(Some(&shown), Some("digital-rain"), Some("roli")),
        PlayerTarget {
            current_effect: Some("digital-rain".to_owned()),
            apply_zone: None,
        }
    );
}

#[test]
fn the_shown_zone_s_state_comes_from_the_current_scene() {
    let zones = [
        zone("default", ZoneRole::Primary, "digital-rain"),
        zone("roli", ZoneRole::Custom, "bubble-garden"),
    ];
    let stale = zone("roli", ZoneRole::Custom, "touch-ripple");
    assert_eq!(
        shown_zone_state(Some("roli"), &zones, Some(&stale)),
        Some(zones[1].clone())
    );
    assert_eq!(shown_zone_state(None, &zones, Some(&stale)), None);
}

#[test]
fn a_failed_refetch_keeps_the_shown_zone_s_last_state() {
    // The preview keeps the Default zone through a failed refetch, so the
    // player beside it must keep targeting the Default zone too rather
    // than falling back to the target zone.
    let last = zone("default", ZoneRole::Primary, "digital-rain");
    assert_eq!(
        shown_zone_state(Some("default"), &[], Some(&last)),
        Some(last.clone())
    );
}

#[test]
fn a_shown_zone_missing_from_a_loaded_scene_has_no_state() {
    let zones = [zone("roli", ZoneRole::Custom, "bubble-garden")];
    let last = zone("default", ZoneRole::Primary, "digital-rain");
    assert_eq!(shown_zone_state(Some("default"), &zones, Some(&last)), None);
}

#[test]
fn a_player_beside_a_focused_zone_steps_from_that_zone_s_effect() {
    let shown = zone("roli", ZoneRole::Custom, "bubble-garden");
    assert_eq!(
        player_target(Some(&shown), Some("digital-rain"), Some("roli")),
        PlayerTarget {
            current_effect: Some("bubble-garden".to_owned()),
            apply_zone: Some("roli".to_owned()),
        }
    );
}

#[test]
fn a_player_beside_the_scene_canvas_applies_to_the_target_zone() {
    // A single-zone scene previews its scene canvas, which already is
    // its one zone, so the player applies wherever an apply would.
    assert_eq!(
        player_target(None, Some("digital-rain"), None),
        PlayerTarget {
            current_effect: Some("digital-rain".to_owned()),
            apply_zone: None,
        }
    );
    assert_eq!(
        player_target(None, Some("digital-rain"), Some("roli")).apply_zone,
        Some("roli".to_owned())
    );
}

#[test]
fn a_scene_document_settles_the_preview_target_whenever_it_was_read() {
    // A refetch after a reconnect keeps the last document until the
    // next one lands.
    assert!(scene_read_settles(1, true, 1));
    assert!(scene_read_settles(1, true, 2));
}

#[test]
fn a_failed_scene_read_settles_the_preview_target_only_on_its_own_connection() {
    // At app boot the first read can fail while the daemon is still
    // binding, before any socket was admitted; the socket that opens next
    // serves a scene that read never saw, so the preview keeps holding for
    // that connection's read.
    assert!(!scene_read_settles(0, false, 0));
    assert!(!scene_read_settles(0, false, 1));
    assert!(scene_read_settles(1, false, 1));
}

#[test]
fn a_single_zone_scene_canvas_belongs_to_the_primary_effect() {
    assert_eq!(
        preview_frame_owner(None, true, Some("digital-rain".to_owned()), &[]),
        Some("digital-rain".to_owned())
    );
}

#[test]
fn a_multi_zone_scene_canvas_belongs_to_no_effect() {
    // A daemon that cannot select zones streams the scene canvas, a
    // patchwork of every zone, which must not become anyone's card art.
    // A scene that never loaded may be one, so its canvas is no one's.
    let zones = [
        zone("default", ZoneRole::Primary, "digital-rain"),
        zone("roli", ZoneRole::Custom, "bubble-garden"),
    ];
    assert_eq!(
        preview_frame_owner(None, false, Some("digital-rain".to_owned()), &zones),
        None
    );
    assert_eq!(
        preview_frame_owner(None, false, Some("digital-rain".to_owned()), &[]),
        None
    );
}

#[test]
fn a_shown_zone_s_frames_belong_to_its_sole_effect() {
    let zones = [
        zone("default", ZoneRole::Primary, "digital-rain"),
        zone("roli", ZoneRole::Custom, "bubble-garden"),
    ];
    assert_eq!(
        preview_frame_owner(Some("roli"), false, Some("digital-rain".to_owned()), &zones),
        Some("bubble-garden".to_owned())
    );
}

#[test]
fn a_loaded_scene_document_moves_the_preview_to_the_policy_zone() {
    assert_eq!(
        settled_preview_zone(true, Some("default".to_owned()), None),
        Some("default".to_owned())
    );
    assert_eq!(settled_preview_zone(true, None, Some("default")), None);
}

#[test]
fn a_failed_scene_read_keeps_the_zone_on_screen() {
    // A refetch that fails mid-session, or while the socket is down,
    // must not drop a multi-zone preview onto the scene canvas.
    assert_eq!(
        settled_preview_zone(false, None, Some("default")),
        Some("default".to_owned())
    );
}

#[test]
fn a_first_scene_read_that_fails_keeps_the_scene_canvas() {
    assert_eq!(settled_preview_zone(false, None, None), None);
}
