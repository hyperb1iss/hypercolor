//! The bridged preview surface's notes and the page's preview demand.

use hypercolor_types::scene::{SceneId, ZoneId};
use hypercolor_ui::components::remote_video_preview::preview_note;
use hypercolor_ui::remote_bridge::{PreviewDemand, PreviewState, PreviewZone};
use hypercolor_ui::remote_preview::{
    PreviewDemandRegistry, SurfaceDemand, device_pixel_extent, preview_zone_target,
};

#[test]
fn each_state_shows_its_note_until_video_plays() {
    assert_eq!(preview_note(PreviewState::Live, true), None);
    assert_eq!(
        preview_note(PreviewState::Live, false),
        Some("Connecting live preview")
    );
    assert_eq!(
        preview_note(PreviewState::Connecting, false),
        Some("Connecting live preview")
    );
    assert_eq!(
        preview_note(PreviewState::Connecting, true),
        Some("Connecting live preview"),
        "a held last frame is a still"
    );
    assert_eq!(
        preview_note(PreviewState::Unsupported, false),
        Some("Live preview needs an update")
    );
}

#[test]
fn surfaces_report_their_longest_edge_in_device_pixels() {
    assert_eq!(device_pixel_extent(320.0, 240.0, 1.0), 320);
    assert_eq!(device_pixel_extent(320.0, 240.0, 2.0), 640);
    assert_eq!(device_pixel_extent(240.0, 320.0, 1.5), 480);
    assert_eq!(device_pixel_extent(100.2, 50.0, 1.0), 101);
    assert_eq!(device_pixel_extent(320.0, 240.0, 0.0), 320);
    assert_eq!(device_pixel_extent(320.0, 240.0, f64::NAN), 320);
    assert_eq!(device_pixel_extent(0.0, 0.0, 2.0), 0);
    assert_eq!(device_pixel_extent(f64::NAN, -1.0, 2.0), 0);
    assert_eq!(device_pixel_extent(1.0e9, 1.0, 3.0), 16_384);
}

#[test]
fn the_page_demand_follows_its_largest_visible_surface() {
    let mut registry = PreviewDemandRegistry::default();
    assert_eq!(registry.demand(true), PreviewDemand::default());

    let cabinet = registry.register();
    let sidebar = registry.register();
    assert_ne!(cabinet, sidebar);
    assert_eq!(registry.demand(true), PreviewDemand::default());

    let on_screen = |longest_edge_px| SurfaceDemand {
        visible: true,
        longest_edge_px,
    };
    registry.update(sidebar, on_screen(480));
    assert_eq!(
        registry.demand(true),
        PreviewDemand {
            enabled: true,
            max_width: 480,
            zone: None,
        }
    );
    registry.update(cabinet, on_screen(1_920));
    assert_eq!(registry.demand(true).max_width, 1_920);

    registry.update(
        cabinet,
        SurfaceDemand {
            visible: false,
            longest_edge_px: 1_920,
        },
    );
    assert_eq!(
        registry.demand(true),
        PreviewDemand {
            enabled: true,
            max_width: 480,
            zone: None,
        },
        "an off-screen surface asks for nothing"
    );

    assert_eq!(
        registry.demand(false),
        PreviewDemand {
            enabled: false,
            max_width: 480,
            zone: None,
        },
        "a hidden page turns the preview off and keeps its size"
    );

    registry.update(sidebar, on_screen(0));
    assert!(
        !registry.demand(true).enabled,
        "a surface with no extent yet asks for nothing"
    );

    registry.release(sidebar);
    registry.release(cabinet);
    registry.update(sidebar, on_screen(640));
    assert_eq!(registry.demand(true), PreviewDemand::default());
}

#[test]
fn the_zone_target_is_the_followed_zone_of_the_active_scene() {
    let scene = SceneId::new();
    let zone = ZoneId::new();
    assert_eq!(
        preview_zone_target(Some(scene), Some(&zone.to_string())),
        Some(PreviewZone {
            scene_id: scene,
            zone_id: zone
        })
    );
    assert_eq!(
        preview_zone_target(Some(scene), None),
        None,
        "a preview that follows no zone shows the scene canvas"
    );
    assert_eq!(
        preview_zone_target(None, Some(&zone.to_string())),
        None,
        "a zone of an unknown scene names nothing"
    );
    assert_eq!(
        preview_zone_target(Some(scene), Some("desk")),
        None,
        "an id that is no zone names nothing"
    );
}

#[test]
fn the_page_demand_carries_the_followed_zone() {
    let mut registry = PreviewDemandRegistry::default();
    let surface = registry.register();
    registry.update(
        surface,
        SurfaceDemand {
            visible: true,
            longest_edge_px: 640,
        },
    );
    let scene_canvas = registry.demand(true);
    assert_eq!(scene_canvas.zone, None);

    let zone = PreviewZone {
        scene_id: SceneId::new(),
        zone_id: ZoneId::new(),
    };
    registry.set_zone(Some(zone));
    let following = registry.demand(true);
    assert_eq!(
        following,
        PreviewDemand {
            enabled: true,
            max_width: 640,
            zone: Some(zone),
        }
    );
    assert_ne!(
        following, scene_canvas,
        "a new zone is a new demand, so the bridge hears it"
    );
    assert_eq!(
        registry.demand(false).zone,
        Some(zone),
        "a hidden page keeps the zone for its return"
    );

    registry.set_zone(None);
    assert_eq!(registry.demand(true), scene_canvas);
}
