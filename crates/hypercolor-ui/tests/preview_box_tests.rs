//! Public-API coverage for the shared preview sizing rule.

use hypercolor_ui::components::preview_box::{
    CabinetChrome, PreviewFit, contained_width, hug_cabinet, preview_box_style,
    preview_source_label, preview_source_size,
};

/// The sources the owner reported or the plan names: the default canvas,
/// 16:9 at two scales, a near-square LED zone, and a 6:1 strip.
const SOURCES: [(u32, u32); 6] = [
    (320, 200),
    (640, 360),
    (640, 480),
    (1920, 1080),
    (640, 630),
    (960, 160),
];

const CHROME: CabinetChrome = CabinetChrome {
    width: 2.0,
    height: 49.0,
};

fn aspect((width, height): (u32, u32)) -> f64 {
    f64::from(width) / f64::from(height)
}

#[test]
fn decoded_source_wins_over_expected() {
    assert_eq!(
        preview_source_size(Some((640, 630)), (640, 480)),
        (640, 630)
    );
}

#[test]
fn expected_source_stands_in_before_the_first_frame() {
    assert_eq!(preview_source_size(None, (1920, 1080)), (1920, 1080));
    assert_eq!(preview_source_size(Some((0, 480)), (640, 480)), (640, 480));
    assert_eq!(preview_source_size(None, (0, 0)), (1, 1));
}

#[test]
fn source_label_reads_width_by_height() {
    assert_eq!(preview_source_label((640, 630)), "640x630");
}

#[test]
fn contain_box_never_takes_the_host_shape() {
    let style = preview_box_style(PreviewFit::Contain, "640 / 630", "100%");
    assert!(style.contains("aspect-ratio: 640 / 630;"));
    assert!(style.contains("width: min(100cqw, calc(100cqh * (640 / 630)));"));
    assert!(
        !style.contains("max-height"),
        "max-height leaves width alone and stretches"
    );
}

#[test]
fn fill_width_box_follows_the_source() {
    let style = preview_box_style(PreviewFit::FillWidth, "960 / 160", "100%");
    assert_eq!(
        style,
        "aspect-ratio: 960 / 160; width: 100%; max-width: 100%;"
    );
}

#[test]
fn contained_width_keeps_every_source_inside_the_host() {
    for host in [
        (720.0, 444.0),
        (1100.0, 444.0),
        (1920.0, 1080.0),
        (390.0, 219.0),
    ] {
        for source in SOURCES {
            let width = contained_width(source, host.0, host.1);
            let height = width / aspect(source);
            assert!(width <= host.0 + 1e-9, "{source:?} in {host:?} is too wide");
            assert!(
                height <= host.1 + 1e-9,
                "{source:?} in {host:?} is too tall"
            );
            assert!(
                (width - host.0).abs() < 1e-9 || (height - host.1).abs() < 1e-9,
                "{source:?} in {host:?} should touch one side"
            );
        }
    }
}

#[test]
fn cabinet_hugs_tall_sources_instead_of_pillarboxing() {
    let (width, height) = hug_cabinet((640, 630), 720.0, 496.0, CHROME, 360.0);
    let picture = (width - CHROME.width, height - CHROME.height);
    assert!(
        width < 720.0,
        "extra width goes back to the favorites panel"
    );
    assert!((picture.0 / picture.1 - aspect((640, 630))).abs() < 1e-9);
    assert!((height - 496.0).abs() < 1e-9);
}

#[test]
fn cabinet_shortens_for_wide_sources_instead_of_letterboxing() {
    let (width, height) = hug_cabinet((1920, 1080), 720.0, 496.0, CHROME, 360.0);
    let picture = (width - CHROME.width, height - CHROME.height);
    assert!((width - 720.0).abs() < 1e-9);
    assert!(height < 496.0);
    assert!((picture.0 / picture.1 - aspect((1920, 1080))).abs() < 1e-9);
}

#[test]
fn cabinet_keeps_the_source_aspect_at_every_requested_width() {
    for source in SOURCES {
        for requested in [360.0, 540.0, 720.0, 1100.0, 1600.0] {
            let (width, height) = hug_cabinet(source, requested, 496.0, CHROME, 360.0);
            assert!(width <= requested + 1e-9);
            assert!(height <= 496.0 + 1e-9);
            let picture = (width - CHROME.width, height - CHROME.height);
            let shown = picture.0 / picture.1;
            // A source too tall for the minimum width pillarboxes inside the
            // cabinet; every other source fills it exactly.
            if width > 360.0 + 1e-9 || (height - 496.0).abs() > 1e-9 {
                assert!(
                    (shown - aspect(source)).abs() < 1e-6,
                    "{source:?} at {requested} shows {shown}"
                );
            }
        }
    }
}
