//! The bridged preview surface's notes and the page's preview demand.

use hypercolor_ui::components::remote_video_preview::preview_note;
use hypercolor_ui::remote_bridge::{PreviewDemand, PreviewState};
use hypercolor_ui::remote_preview::{PreviewDemandRegistry, SurfaceDemand, device_pixel_extent};

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
            max_width: 480
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
            max_width: 480
        },
        "an off-screen surface asks for nothing"
    );

    assert_eq!(
        registry.demand(false),
        PreviewDemand {
            enabled: false,
            max_width: 480
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
