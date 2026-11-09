use hypercolor_types::device::{SegmentLayoutHint, SegmentPlacement};
use hypercolor_types::spatial::NormalizedPosition;

fn panel(x: f32, y: f32, width: f32, height: f32) -> SegmentPlacement {
    SegmentPlacement {
        position: NormalizedPosition::new(x, y),
        size: NormalizedPosition::new(width, height),
        rotation: 0.0,
    }
}

fn close(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() < 0.00001,
        "{actual} != {expected}"
    );
}

#[test]
fn mixed_panels_keep_contact_and_physical_scale_on_wide_canvas() {
    let placements = [
        Some(panel(-50.0, 0.0, 100.0, 100.0)),
        Some(panel(25.0, 0.0, 50.0, 50.0)),
    ];
    let fitted = SegmentPlacement::fit_all(
        &placements,
        NormalizedPosition::new(0.5, 0.5),
        NormalizedPosition::new(0.8, 0.8),
        (1920, 1080),
    );
    let large = fitted[0].expect("large panel");
    let small = fitted[1].expect("small panel");
    close(large.size.x * (1920.0 / 1080.0), large.size.y);
    close(large.size.x, small.size.x * 2.0);
    close(
        large.position.x + large.size.x * 0.5,
        small.position.x - small.size.x * 0.5,
    );
    close(large.position.y, small.position.y);
}

#[test]
fn rotated_box_fits_within_the_requested_bounds() {
    let mut placement = panel(100.0, -200.0, 200.0, 100.0);
    placement.rotation = std::f32::consts::FRAC_PI_2;
    let fitted = SegmentPlacement::fit_all(
        &[Some(placement)],
        NormalizedPosition::new(0.3, 0.4),
        NormalizedPosition::new(0.2, 0.6),
        (800, 800),
    );
    let fitted = fitted[0].expect("rotated panel");
    close(fitted.position.x, 0.3);
    close(fitted.position.y, 0.4);
    close(fitted.size.y, 0.2);
    close(fitted.size.x, 0.4);
    close(fitted.rotation, std::f32::consts::FRAC_PI_2);
}

#[test]
fn missing_and_invalid_geometry_do_not_poison_valid_siblings() {
    let fitted = SegmentPlacement::fit_all(
        &[
            None,
            Some(panel(f32::NAN, 0.0, 10.0, 10.0)),
            Some(panel(0.0, 0.0, 0.0, 10.0)),
            Some(panel(0.0, 0.0, 10.0, 10.0)),
        ],
        NormalizedPosition::new(0.5, 0.5),
        NormalizedPosition::new(0.2, 0.2),
        (0, 0),
    );
    assert!(fitted[..3].iter().all(Option::is_none));
    let valid = fitted[3].expect("valid sibling");
    close(valid.size.x, 0.2);
    close(valid.size.y, 0.2);
    assert!(
        SegmentPlacement::fit_all(
            &[],
            NormalizedPosition::new(0.5, 0.5),
            NormalizedPosition::new(0.2, 0.2),
            (640, 480)
        )
        .is_empty()
    );
}

#[test]
fn placement_survives_serialization_and_omission_preserves_old_hints() {
    let hint = SegmentLayoutHint {
        placement: Some(panel(-12.0, 25.0, 67.0, 58.0)),
        ..Default::default()
    };
    let encoded = serde_json::to_value(&hint).expect("serialize hint");
    let decoded: SegmentLayoutHint = serde_json::from_value(encoded).expect("deserialize hint");
    assert_eq!(hint, decoded);
    let empty: SegmentLayoutHint = serde_json::from_str("{}").expect("empty hint");
    assert_eq!(empty.placement, None);
}
