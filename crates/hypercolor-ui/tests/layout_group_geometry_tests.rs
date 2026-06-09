use hypercolor_types::spatial::{
    EdgeBehavior, LedTopology, NormalizedPosition, Output, SamplingMode, SpatialLayout,
    StripDirection,
};
use hypercolor_ui::layout_geometry;

// ── Compound bounding box ────────────────────────────────────────────────

fn plain_zone(id: &str, device_id: &str, x: f32, y: f32, w: f32, h: f32) -> Output {
    Output {
        id: id.to_owned(),
        name: id.to_owned(),
        device_id: device_id.to_owned(),
        zone_name: None,
        position: NormalizedPosition::new(x, y),
        size: NormalizedPosition::new(w, h),
        rotation: 0.0,
        scale: 1.0,
        display_order: 0,
        orientation: None,
        topology: LedTopology::Strip {
            count: 10,
            direction: StripDirection::LeftToRight,
        },
        led_positions: Vec::new(),
        led_mapping: None,
        sampling_mode: None,
        edge_behavior: None,
        shape: None,
        shape_preset: None,
        attachment: None,
        brightness: None,
    }
}

fn simple_layout(zones: Vec<Output>) -> SpatialLayout {
    SpatialLayout {
        id: "test".to_owned(),
        name: "Test".to_owned(),
        description: None,
        canvas_width: 320,
        canvas_height: 200,
        zones,
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    }
}

#[test]
fn compound_bounding_box_two_zones() {
    // Zone A: center (0.3, 0.4), size (0.2, 0.1) -> spans x [0.2, 0.4], y [0.35, 0.45]
    // Zone B: center (0.7, 0.6), size (0.2, 0.1) -> spans x [0.6, 0.8], y [0.55, 0.65]
    // Combined AABB: x [0.2, 0.8], y [0.35, 0.65]
    // Expected center: (0.5, 0.5), size: (0.6, 0.3)
    let layout = simple_layout(vec![
        plain_zone("a", "dev", 0.3, 0.4, 0.2, 0.1),
        plain_zone("b", "dev", 0.7, 0.6, 0.2, 0.1),
    ]);

    let ids: std::collections::HashSet<String> =
        ["a".to_owned(), "b".to_owned()].into_iter().collect();
    let bounds = layout_geometry::compound_bounding_box(&layout, &ids)
        .expect("should produce bounds for two zones");

    assert!((bounds.center.x - 0.5).abs() < 0.001);
    assert!((bounds.center.y - 0.5).abs() < 0.001);
    assert!((bounds.size.x - 0.6).abs() < 0.001);
    assert!((bounds.size.y - 0.3).abs() < 0.001);
}

#[test]
fn compound_bounding_box_single_zone() {
    let layout = simple_layout(vec![plain_zone("solo", "dev", 0.5, 0.5, 0.2, 0.1)]);

    let ids: std::collections::HashSet<String> = ["solo".to_owned()].into_iter().collect();
    let bounds = layout_geometry::compound_bounding_box(&layout, &ids)
        .expect("should produce bounds for single zone");

    assert!((bounds.center.x - 0.5).abs() < 0.001);
    assert!((bounds.center.y - 0.5).abs() < 0.001);
    assert!((bounds.size.x - 0.2).abs() < 0.001);
    assert!((bounds.size.y - 0.1).abs() < 0.001);
}

#[test]
fn compound_bounding_box_empty_returns_none() {
    let layout = simple_layout(vec![plain_zone("a", "dev", 0.5, 0.5, 0.2, 0.1)]);

    let ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    assert!(layout_geometry::compound_bounding_box(&layout, &ids).is_none());
}

// ── Translate zones ──────────────────────────────────────────────────────

#[test]
fn translate_zones_preserves_relative_positions() {
    let mut layout = simple_layout(vec![
        plain_zone("a", "dev", 0.3, 0.3, 0.1, 0.1),
        plain_zone("b", "dev", 0.5, 0.3, 0.1, 0.1),
    ]);

    let initial_positions = vec![
        ("a".to_owned(), NormalizedPosition::new(0.3, 0.3)),
        ("b".to_owned(), NormalizedPosition::new(0.5, 0.3)),
    ];
    let delta = NormalizedPosition::new(0.1, 0.2);

    layout_geometry::translate_zones(&mut layout, &initial_positions, delta);

    let a = layout.zones.iter().find(|z| z.id == "a").expect("zone a");
    let b = layout.zones.iter().find(|z| z.id == "b").expect("zone b");

    // Relative offset between a and b should be preserved (0.2 horizontal, 0.0 vertical)
    assert!((b.position.x - a.position.x - 0.2).abs() < 0.001);
    assert!((b.position.y - a.position.y).abs() < 0.001);
}

#[test]
fn translate_zones_clamps_to_canvas() {
    let mut layout = simple_layout(vec![plain_zone("a", "dev", 0.5, 0.5, 0.1, 0.1)]);

    let initial_positions = vec![("a".to_owned(), NormalizedPosition::new(0.5, 0.5))];
    // Large delta that would push past [0, 1]
    let delta = NormalizedPosition::new(5.0, 5.0);

    layout_geometry::translate_zones(&mut layout, &initial_positions, delta);

    let a = &layout.zones[0];
    assert!(a.position.x <= 1.0);
    assert!(a.position.y <= 1.0);
    assert!(a.position.x >= 0.0);
    assert!(a.position.y >= 0.0);
}

// ── Group centroid ──────────────────────────────────────────────────────

#[test]
fn group_centroid_averages_zone_positions() {
    let layout = simple_layout(vec![
        plain_zone("a", "dev", 0.2, 0.3, 0.1, 0.1),
        plain_zone("b", "dev", 0.6, 0.3, 0.1, 0.1),
        plain_zone("c", "dev", 0.4, 0.7, 0.1, 0.1),
    ]);

    let ids: std::collections::HashSet<String> =
        ["a", "b", "c"].iter().map(|s| s.to_string()).collect();
    let centroid = layout_geometry::group_centroid(&layout, &ids)
        .expect("should compute centroid for 3 zones");

    assert!((centroid.x - 0.4).abs() < 0.001);
    assert!((centroid.y - (0.3 + 0.3 + 0.7) / 3.0).abs() < 0.001);
}

#[test]
fn group_centroid_empty_returns_none() {
    let layout = simple_layout(vec![plain_zone("a", "dev", 0.5, 0.5, 0.1, 0.1)]);
    let ids = std::collections::HashSet::new();
    assert!(layout_geometry::group_centroid(&layout, &ids).is_none());
}

// ── Group translate ─────────────────────────────────────────────────────

#[test]
fn translate_group_moves_centroid_preserving_relative_positions() {
    let mut layout = simple_layout(vec![
        plain_zone("a", "dev", 0.3, 0.4, 0.1, 0.1),
        plain_zone("b", "dev", 0.5, 0.4, 0.1, 0.1),
    ]);

    let ids: std::collections::HashSet<String> = ["a", "b"].iter().map(|s| s.to_string()).collect();

    // Centroid is (0.4, 0.4). Move it to (0.6, 0.6).
    layout_geometry::translate_group(&mut layout, &ids, NormalizedPosition::new(0.6, 0.6));

    let a = layout.zones.iter().find(|z| z.id == "a").expect("a");
    let b = layout.zones.iter().find(|z| z.id == "b").expect("b");

    // Relative offset should be preserved: b is 0.2 to the right of a
    assert!((b.position.x - a.position.x - 0.2).abs() < 0.001);
    assert!((b.position.y - a.position.y).abs() < 0.001);

    // New centroid should be at (0.6, 0.6)
    assert!(((a.position.x + b.position.x) / 2.0 - 0.6).abs() < 0.001);
    assert!(((a.position.y + b.position.y) / 2.0 - 0.6).abs() < 0.001);
}

// ── Group rotate ────────────────────────────────────────────────────────

#[test]
fn rotate_group_90_degrees_orbits_and_rotates_zones() {
    let mut layout = simple_layout(vec![
        plain_zone("a", "dev", 0.4, 0.5, 0.1, 0.1),
        plain_zone("b", "dev", 0.6, 0.5, 0.1, 0.1),
    ]);

    let ids: std::collections::HashSet<String> = ["a", "b"].iter().map(|s| s.to_string()).collect();

    // Centroid is (0.5, 0.5). Rotate 90 degrees.
    let delta = std::f32::consts::FRAC_PI_2;
    layout_geometry::rotate_group(&mut layout, &ids, delta);

    let a = layout.zones.iter().find(|z| z.id == "a").expect("a");
    let b = layout.zones.iter().find(|z| z.id == "b").expect("b");

    // Zone A was at (-0.1, 0) offset from centroid.
    // After 90 CCW rotation: (0, -0.1) offset -> position (0.5, 0.4)
    assert!((a.position.x - 0.5).abs() < 0.01);
    assert!((a.position.y - 0.4).abs() < 0.01);

    // Zone B was at (0.1, 0) offset -> after 90: (0, 0.1) -> position (0.5, 0.6)
    assert!((b.position.x - 0.5).abs() < 0.01);
    assert!((b.position.y - 0.6).abs() < 0.01);

    // Both zones' individual rotation should include the 90-degree offset
    assert!((a.rotation - delta).abs() < 0.01);
    assert!((b.rotation - delta).abs() < 0.01);
}

#[test]
fn rotate_group_preserves_centroid() {
    let mut layout = simple_layout(vec![
        plain_zone("a", "dev", 0.3, 0.4, 0.1, 0.1),
        plain_zone("b", "dev", 0.5, 0.4, 0.1, 0.1),
        plain_zone("c", "dev", 0.4, 0.6, 0.1, 0.1),
    ]);

    let ids: std::collections::HashSet<String> =
        ["a", "b", "c"].iter().map(|s| s.to_string()).collect();

    let centroid_before = layout_geometry::group_centroid(&layout, &ids).expect("centroid");
    layout_geometry::rotate_group(&mut layout, &ids, 0.7); // ~40 degrees
    let centroid_after = layout_geometry::group_centroid(&layout, &ids).expect("centroid");

    assert!((centroid_before.x - centroid_after.x).abs() < 0.01);
    assert!((centroid_before.y - centroid_after.y).abs() < 0.01);
}

#[test]
fn rotate_group_zero_delta_returns_false() {
    let mut layout = simple_layout(vec![
        plain_zone("a", "dev", 0.3, 0.4, 0.1, 0.1),
        plain_zone("b", "dev", 0.5, 0.4, 0.1, 0.1),
    ]);
    let ids: std::collections::HashSet<String> = ["a", "b"].iter().map(|s| s.to_string()).collect();

    assert!(!layout_geometry::rotate_group(&mut layout, &ids, 0.0));
}

// ── Group scale ─────────────────────────────────────────────────────────

#[test]
fn scale_group_doubles_spread_and_zone_scales() {
    let mut layout = simple_layout(vec![
        plain_zone("a", "dev", 0.4, 0.5, 0.1, 0.1),
        plain_zone("b", "dev", 0.6, 0.5, 0.1, 0.1),
    ]);

    let ids: std::collections::HashSet<String> = ["a", "b"].iter().map(|s| s.to_string()).collect();

    // Centroid is (0.5, 0.5). Scale 2x.
    layout_geometry::scale_group(&mut layout, &ids, 2.0);

    let a = layout.zones.iter().find(|z| z.id == "a").expect("a");
    let b = layout.zones.iter().find(|z| z.id == "b").expect("b");

    // Zone A was at -0.1 offset from centroid, now should be -0.2 -> position 0.3
    assert!((a.position.x - 0.3).abs() < 0.01);
    // Zone B was at +0.1 offset, now +0.2 -> position 0.7
    assert!((b.position.x - 0.7).abs() < 0.01);

    // Individual scales should double
    assert!((a.scale - 2.0).abs() < 0.01);
    assert!((b.scale - 2.0).abs() < 0.01);
}

#[test]
fn scale_group_preserves_centroid() {
    let mut layout = simple_layout(vec![
        plain_zone("a", "dev", 0.3, 0.4, 0.1, 0.1),
        plain_zone("b", "dev", 0.5, 0.4, 0.1, 0.1),
        plain_zone("c", "dev", 0.4, 0.6, 0.1, 0.1),
    ]);

    let ids: std::collections::HashSet<String> =
        ["a", "b", "c"].iter().map(|s| s.to_string()).collect();

    let centroid_before = layout_geometry::group_centroid(&layout, &ids).expect("centroid");
    layout_geometry::scale_group(&mut layout, &ids, 1.5);
    let centroid_after = layout_geometry::group_centroid(&layout, &ids).expect("centroid");

    assert!((centroid_before.x - centroid_after.x).abs() < 0.01);
    assert!((centroid_before.y - centroid_after.y).abs() < 0.01);
}

#[test]
fn scale_group_identity_returns_false() {
    let mut layout = simple_layout(vec![
        plain_zone("a", "dev", 0.3, 0.4, 0.1, 0.1),
        plain_zone("b", "dev", 0.5, 0.4, 0.1, 0.1),
    ]);
    let ids: std::collections::HashSet<String> = ["a", "b"].iter().map(|s| s.to_string()).collect();

    assert!(!layout_geometry::scale_group(&mut layout, &ids, 1.0));
}

// ── Group align ─────────────────────────────────────────────────────────

#[test]
fn align_group_left_matches_bbox_left_edge() {
    let mut layout = simple_layout(vec![
        plain_zone("a", "dev", 0.20, 0.30, 0.10, 0.10),
        plain_zone("b", "dev", 0.60, 0.30, 0.20, 0.10),
        plain_zone("c", "dev", 0.45, 0.60, 0.10, 0.10),
    ]);
    let ids: std::collections::HashSet<String> =
        ["a", "b", "c"].iter().map(|s| s.to_string()).collect();

    layout_geometry::align_group(
        &mut layout,
        &ids,
        layout_geometry::AlignAxis::X,
        layout_geometry::AlignAnchor::Min,
    );

    let left_edges: Vec<f32> = layout
        .zones
        .iter()
        .filter(|z| ids.contains(&z.id))
        .map(|z| z.position.x - z.size.x * 0.5)
        .collect();
    let first = left_edges[0];
    for edge in &left_edges[1..] {
        assert!(
            (edge - first).abs() < 0.001,
            "left edges should match: {first} vs {edge}"
        );
    }
    // Y coords unchanged
    let a = layout.zones.iter().find(|z| z.id == "a").expect("a");
    assert!((a.position.y - 0.30).abs() < 0.001);
    let c = layout.zones.iter().find(|z| z.id == "c").expect("c");
    assert!((c.position.y - 0.60).abs() < 0.001);
}

#[test]
fn align_group_right_matches_bbox_right_edge() {
    let mut layout = simple_layout(vec![
        plain_zone("a", "dev", 0.20, 0.30, 0.10, 0.10),
        plain_zone("b", "dev", 0.60, 0.30, 0.20, 0.10),
    ]);
    let ids: std::collections::HashSet<String> = ["a", "b"].iter().map(|s| s.to_string()).collect();

    layout_geometry::align_group(
        &mut layout,
        &ids,
        layout_geometry::AlignAxis::X,
        layout_geometry::AlignAnchor::Max,
    );

    // Bbox right edge = 0.60 + 0.10 = 0.70
    let a = layout.zones.iter().find(|z| z.id == "a").expect("a");
    let b = layout.zones.iter().find(|z| z.id == "b").expect("b");
    assert!((a.position.x + a.size.x * 0.5 - 0.70).abs() < 0.001);
    assert!((b.position.x + b.size.x * 0.5 - 0.70).abs() < 0.001);
}

#[test]
fn align_group_center_x_matches_bbox_center() {
    let mut layout = simple_layout(vec![
        plain_zone("a", "dev", 0.20, 0.30, 0.10, 0.10),
        plain_zone("b", "dev", 0.60, 0.30, 0.20, 0.10),
    ]);
    let ids: std::collections::HashSet<String> = ["a", "b"].iter().map(|s| s.to_string()).collect();

    // a spans x [0.15, 0.25], b spans x [0.50, 0.70]. Bbox center x = 0.425.
    layout_geometry::align_group(
        &mut layout,
        &ids,
        layout_geometry::AlignAxis::X,
        layout_geometry::AlignAnchor::Center,
    );

    let a = layout.zones.iter().find(|z| z.id == "a").expect("a");
    let b = layout.zones.iter().find(|z| z.id == "b").expect("b");
    assert!((a.position.x - 0.425).abs() < 0.01);
    assert!((b.position.x - 0.425).abs() < 0.01);
}

#[test]
fn align_group_top_matches_bbox_top_edge() {
    let mut layout = simple_layout(vec![
        plain_zone("a", "dev", 0.3, 0.25, 0.10, 0.10),
        plain_zone("b", "dev", 0.5, 0.40, 0.10, 0.20),
        plain_zone("c", "dev", 0.7, 0.55, 0.10, 0.10),
    ]);
    let ids: std::collections::HashSet<String> =
        ["a", "b", "c"].iter().map(|s| s.to_string()).collect();

    layout_geometry::align_group(
        &mut layout,
        &ids,
        layout_geometry::AlignAxis::Y,
        layout_geometry::AlignAnchor::Min,
    );

    // Bbox top = 0.25 - 0.05 = 0.20
    let tops: Vec<f32> = layout
        .zones
        .iter()
        .filter(|z| ids.contains(&z.id))
        .map(|z| z.position.y - z.size.y * 0.5)
        .collect();
    for t in &tops {
        assert!((t - 0.20).abs() < 0.001);
    }
}

// ── Group distribute ────────────────────────────────────────────────────

#[test]
fn distribute_group_horizontal_equalizes_gaps() {
    let mut layout = simple_layout(vec![
        plain_zone("a", "dev", 0.10, 0.50, 0.05, 0.10),
        plain_zone("b", "dev", 0.30, 0.50, 0.05, 0.10),
        plain_zone("c", "dev", 0.90, 0.50, 0.05, 0.10),
    ]);
    let ids: std::collections::HashSet<String> =
        ["a", "b", "c"].iter().map(|s| s.to_string()).collect();

    layout_geometry::distribute_group(&mut layout, &ids, layout_geometry::AlignAxis::X);

    // First and last should be unchanged
    let a = layout.zones.iter().find(|z| z.id == "a").expect("a");
    let c = layout.zones.iter().find(|z| z.id == "c").expect("c");
    assert!((a.position.x - 0.10).abs() < 0.01);
    assert!((c.position.x - 0.90).abs() < 0.01);

    // Gap between a.right and b.left should equal gap between b.right and c.left
    let b = layout.zones.iter().find(|z| z.id == "b").expect("b");
    let gap1 = (b.position.x - b.size.x * 0.5) - (a.position.x + a.size.x * 0.5);
    let gap2 = (c.position.x - c.size.x * 0.5) - (b.position.x + b.size.x * 0.5);
    assert!((gap1 - gap2).abs() < 0.01);
}

#[test]
fn distribute_group_under_three_zones_is_noop() {
    let mut layout = simple_layout(vec![
        plain_zone("a", "dev", 0.1, 0.5, 0.05, 0.10),
        plain_zone("b", "dev", 0.9, 0.5, 0.05, 0.10),
    ]);
    let ids: std::collections::HashSet<String> = ["a", "b"].iter().map(|s| s.to_string()).collect();
    assert!(!layout_geometry::distribute_group(
        &mut layout,
        &ids,
        layout_geometry::AlignAxis::X
    ));
}

// ── Group pack ──────────────────────────────────────────────────────────

#[test]
fn pack_group_horizontal_butts_zones_edge_to_edge() {
    let mut layout = simple_layout(vec![
        plain_zone("a", "dev", 0.20, 0.50, 0.10, 0.10),
        plain_zone("b", "dev", 0.50, 0.50, 0.10, 0.10),
        plain_zone("c", "dev", 0.80, 0.50, 0.10, 0.10),
    ]);
    let ids: std::collections::HashSet<String> =
        ["a", "b", "c"].iter().map(|s| s.to_string()).collect();

    layout_geometry::pack_group(&mut layout, &ids, layout_geometry::AlignAxis::X);

    let a = layout.zones.iter().find(|z| z.id == "a").expect("a");
    let b = layout.zones.iter().find(|z| z.id == "b").expect("b");
    let c = layout.zones.iter().find(|z| z.id == "c").expect("c");

    // a anchors the sequence at its original position
    assert!((a.position.x - 0.20).abs() < 0.01);
    // b.left == a.right
    let a_right = a.position.x + a.size.x * 0.5;
    let b_left = b.position.x - b.size.x * 0.5;
    assert!((a_right - b_left).abs() < 0.001);
    // c.left == b.right
    let b_right = b.position.x + b.size.x * 0.5;
    let c_left = c.position.x - c.size.x * 0.5;
    assert!((b_right - c_left).abs() < 0.001);
}

// ── Group mirror ────────────────────────────────────────────────────────

#[test]
fn mirror_group_horizontal_flips_positions_around_centroid() {
    let mut layout = simple_layout(vec![
        plain_zone("a", "dev", 0.30, 0.50, 0.10, 0.10),
        plain_zone("b", "dev", 0.70, 0.50, 0.10, 0.10),
    ]);
    let ids: std::collections::HashSet<String> = ["a", "b"].iter().map(|s| s.to_string()).collect();

    // centroid x = 0.50. Mirror should swap effective X positions.
    layout_geometry::mirror_group(&mut layout, &ids, layout_geometry::AlignAxis::X);

    let a = layout.zones.iter().find(|z| z.id == "a").expect("a");
    let b = layout.zones.iter().find(|z| z.id == "b").expect("b");
    assert!((a.position.x - 0.70).abs() < 0.01);
    assert!((b.position.x - 0.30).abs() < 0.01);
    // Y unchanged
    assert!((a.position.y - 0.50).abs() < 0.01);
    assert!((b.position.y - 0.50).abs() < 0.01);
}

#[test]
fn mirror_group_across_vertical_axis_reflects_rotation_to_pi_minus_theta() {
    // Reflecting across a vertical line through the centroid sends a
    // segment at angle θ to angle π − θ (not −θ). Use 45° so the two
    // formulas give different answers — this test catches a bug where
    // the code naïvely negated rotation for both axes.
    let mut layout = simple_layout(vec![
        plain_zone("a", "dev", 0.30, 0.50, 0.10, 0.10),
        plain_zone("b", "dev", 0.70, 0.50, 0.10, 0.10),
    ]);
    layout.zones[0].rotation = std::f32::consts::FRAC_PI_4;
    let ids: std::collections::HashSet<String> = ["a", "b"].iter().map(|s| s.to_string()).collect();

    layout_geometry::mirror_group(&mut layout, &ids, layout_geometry::AlignAxis::X);

    let a = layout.zones.iter().find(|z| z.id == "a").expect("a");
    let expected =
        (std::f32::consts::PI - std::f32::consts::FRAC_PI_4).rem_euclid(std::f32::consts::TAU);
    assert!(
        (a.rotation - expected).abs() < 0.01,
        "expected {expected}, got {}",
        a.rotation
    );
}

#[test]
fn mirror_group_across_horizontal_axis_negates_rotation() {
    let mut layout = simple_layout(vec![
        plain_zone("a", "dev", 0.50, 0.30, 0.10, 0.10),
        plain_zone("b", "dev", 0.50, 0.70, 0.10, 0.10),
    ]);
    layout.zones[0].rotation = std::f32::consts::FRAC_PI_4;
    let ids: std::collections::HashSet<String> = ["a", "b"].iter().map(|s| s.to_string()).collect();

    layout_geometry::mirror_group(&mut layout, &ids, layout_geometry::AlignAxis::Y);

    let a = layout.zones.iter().find(|z| z.id == "a").expect("a");
    let expected = (-std::f32::consts::FRAC_PI_4).rem_euclid(std::f32::consts::TAU);
    assert!(
        (a.rotation - expected).abs() < 0.01,
        "expected {expected}, got {}",
        a.rotation
    );
}

#[test]
fn mirror_group_single_zone_is_noop() {
    let mut layout = simple_layout(vec![plain_zone("a", "dev", 0.5, 0.5, 0.1, 0.1)]);
    layout.zones[0].rotation = std::f32::consts::FRAC_PI_4;
    let ids: std::collections::HashSet<String> = ["a"].iter().map(|s| s.to_string()).collect();

    assert!(!layout_geometry::mirror_group(
        &mut layout,
        &ids,
        layout_geometry::AlignAxis::X,
    ));
    let a = layout.zones.iter().find(|z| z.id == "a").expect("a");
    // Rotation preserved
    assert!((a.rotation - std::f32::consts::FRAC_PI_4).abs() < 0.01);
}
