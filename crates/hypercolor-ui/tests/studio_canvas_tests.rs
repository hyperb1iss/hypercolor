//! Contract tests for the Studio canvas: which box a press means, and
//! when a box shows its name.

use hypercolor_types::spatial::{LedTopology, NormalizedPosition, Output, Winding};
use hypercolor_ui::components::layout_canvas::labels::{
    LabelBox, LabelMode, LabelRoom, Visibility, callout_offset, detail_line, group_caption,
    label_extent, label_mode, label_rooms, union_bounds, visibility_class,
};
use hypercolor_ui::components::layout_canvas::picking::{HitBox, hit_stack, pick_hit};

fn hit(id: &str, center: (f32, f32), half: (f32, f32), order: usize) -> HitBox {
    HitBox {
        id: id.to_owned(),
        center,
        half,
        rotation: 0.0,
        circular: false,
        polygon: None,
        order,
    }
}

fn ring_output(id: &str) -> Output {
    Output {
        id: id.to_owned(),
        name: id.to_owned(),
        device_id: "fan".to_owned(),
        zone_name: None,
        position: NormalizedPosition::new(0.5, 0.5),
        size: NormalizedPosition::new(0.3, 0.4),
        rotation: 0.0,
        scale: 1.0,
        display_order: 0,
        orientation: None,
        topology: LedTopology::Ring {
            count: 12,
            start_angle: 0.0,
            direction: Winding::Clockwise,
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

// ── Hit testing ─────────────────────────────────────────────────────────

#[test]
fn the_smallest_box_under_the_pointer_wins_even_beneath_a_lifted_one() {
    let boxes = [
        // A selected strip lifted above everything else.
        hit("strip", (0.5, 0.5), (0.4, 0.05), 10_000),
        hit("fan", (0.5, 0.5), (0.03, 0.03), 12),
        hit("case", (0.5, 0.5), (0.45, 0.45), 10),
    ];
    assert_eq!(hit_stack(&boxes, (0.5, 0.5)), ["fan", "strip", "case"]);
    assert_eq!(hit_stack(&boxes, (0.8, 0.5)), ["strip", "case"]);
    assert!(hit_stack(&boxes, (0.99, 0.99)).is_empty());
}

#[test]
fn equal_boxes_resolve_to_the_one_painted_on_top() {
    let boxes = [
        hit("under", (0.5, 0.5), (0.1, 0.1), 10),
        hit("over", (0.5, 0.5), (0.1, 0.1), 11),
    ];
    assert_eq!(hit_stack(&boxes, (0.5, 0.5)), ["over", "under"]);
}

#[test]
fn a_rotated_box_answers_in_its_own_frame() {
    let mut upright = hit("strip", (0.5, 0.5), (0.3, 0.02), 10);
    upright.rotation = std::f32::consts::FRAC_PI_2;
    assert!(upright.contains((0.5, 0.75)), "along the turned strip");
    assert!(
        !upright.contains((0.75, 0.5)),
        "where it lay before turning"
    );
}

#[test]
fn a_ring_answers_inside_its_circle_only() {
    let aspect = 640.0 / 480.0;
    let ring = HitBox::from_output(&ring_output("fan"), aspect, 10);
    assert!(ring.circular);
    // A ring draws square in pixels, sized by its width.
    assert!((ring.half.0 - ring.half.1).abs() < 1e-6);
    let center = ring.center;
    assert!(ring.contains(center));
    let corner = (center.0 + ring.half.0 * 0.9, center.1 + ring.half.1 * 0.9);
    assert!(!ring.contains(corner));
}

#[test]
fn a_plain_press_takes_the_smallest_box() {
    let stack = ["fan".to_owned(), "strip".to_owned(), "case".to_owned()];
    let picked = pick_hit(&stack, Some("strip"), false, str::to_owned);
    assert_eq!(picked.as_deref(), Some("fan"));
    assert_eq!(pick_hit(&[], None, false, str::to_owned), None);
}

#[test]
fn alt_press_cycles_down_the_stack_and_wraps() {
    let stack = ["fan".to_owned(), "strip".to_owned(), "case".to_owned()];
    let next = |current| pick_hit(&stack, current, true, str::to_owned);
    assert_eq!(next(None).as_deref(), Some("fan"));
    assert_eq!(next(Some("fan")).as_deref(), Some("strip"));
    assert_eq!(next(Some("strip")).as_deref(), Some("case"));
    assert_eq!(next(Some("case")).as_deref(), Some("fan"));
    assert_eq!(
        next(Some("elsewhere")).as_deref(),
        Some("fan"),
        "a box outside the stack restarts at the smallest"
    );
}

#[test]
fn alt_press_skips_boxes_that_select_the_same_device() {
    let stack = [
        "pads".to_owned(),
        "buttons".to_owned(),
        "strip".to_owned(),
        "white".to_owned(),
    ];
    let device = |id: &str| match id {
        "strip" => "case",
        _ => "push",
    };
    let next = |current| pick_hit(&stack, current, true, device);
    assert_eq!(next(Some("pads")).as_deref(), Some("strip"));
    assert_eq!(next(Some("buttons")).as_deref(), Some("strip"));
    assert_eq!(next(Some("strip")).as_deref(), Some("pads"));
}

// ── Labels ──────────────────────────────────────────────────────────────

fn label(id: &str, center: (f32, f32), size: (f32, f32), chars: usize, z: usize) -> LabelBox {
    LabelBox {
        id: id.to_owned(),
        center,
        size,
        rotation: 0.0,
        name_chars: chars,
        z,
    }
}

#[test]
fn a_box_holds_its_name_when_the_name_and_led_count_fit() {
    assert!(label_extent(12, 160.0, 60.0).is_some(), "one line");
    assert!(
        label_extent(24, 100.0, 60.0).is_some(),
        "wraps to two lines"
    );
    assert!(
        label_extent(40, 100.0, 80.0).is_none(),
        "three lines is too many"
    );
    assert!(
        label_extent(12, 300.0, 18.0).is_none(),
        "a strip is too short"
    );
    assert!(
        label_extent(12, 4.0, 80.0).is_none(),
        "a sliver has no room"
    );
}

#[test]
fn a_box_painted_above_covers_the_name_beneath_it() {
    let rooms = label_rooms(&[
        label("case", (200.0, 150.0), (300.0, 200.0), 10, 10),
        label("fan", (200.0, 150.0), (30.0, 30.0), 10, 12),
        label("desk", (500.0, 150.0), (160.0, 80.0), 10, 11),
    ]);
    assert_eq!(rooms["case"], LabelRoom::Covered);
    assert_eq!(rooms["fan"], LabelRoom::TooSmall);
    assert_eq!(rooms["desk"], LabelRoom::Clear);
}

#[test]
fn a_box_painted_below_never_covers_the_name_above_it() {
    let rooms = label_rooms(&[
        label("lifted", (200.0, 150.0), (200.0, 100.0), 8, 10_000),
        label("under", (200.0, 150.0), (400.0, 300.0), 8, 10),
    ]);
    assert_eq!(rooms["lifted"], LabelRoom::Clear);
    assert_eq!(rooms["under"], LabelRoom::Covered);
}

#[test]
fn names_show_at_rest_on_hover_or_on_selection() {
    let shown = Visibility::Shown;
    let hover = Visibility::OnHover;
    let hidden = Visibility::Hidden;
    let mode = |inside, callout| LabelMode { inside, callout };
    assert_eq!(
        label_mode(LabelRoom::Clear, false, false),
        mode(shown, hidden)
    );
    assert_eq!(
        label_mode(LabelRoom::Covered, false, false),
        mode(hover, hidden)
    );
    assert_eq!(
        label_mode(LabelRoom::Covered, true, false),
        mode(shown, hidden)
    );
    assert_eq!(
        label_mode(LabelRoom::TooSmall, false, false),
        mode(hidden, hover)
    );
    assert_eq!(
        label_mode(LabelRoom::TooSmall, true, false),
        mode(hidden, hover)
    );
    assert_eq!(
        label_mode(LabelRoom::TooSmall, true, true),
        mode(hidden, shown)
    );
}

#[test]
fn visibility_maps_to_hover_aware_classes() {
    assert_eq!(visibility_class(Visibility::Shown), "opacity-100");
    assert_eq!(
        visibility_class(Visibility::OnHover),
        "opacity-0 group-hover:opacity-100"
    );
    assert_eq!(visibility_class(Visibility::Hidden), "opacity-0");
}

#[test]
fn a_display_box_names_its_resolution_and_a_light_its_led_count() {
    assert_eq!(detail_line(160_000, Some((400, 400))), "400 \u{d7} 400");
    assert_eq!(detail_line(153_600, Some((960, 160))), "960 \u{d7} 160");
    assert_eq!(detail_line(26, None), "26 LEDs");
    assert_eq!(detail_line(1, None), "1 LED");
}

/// A 640 by 480 canvas, in pixels.
const CANVAS: (f32, f32) = (640.0, 480.0);

fn close(actual: (f32, f32), expected: (f32, f32)) -> bool {
    (actual.0 - expected.0).abs() < 0.01 && (actual.1 - expected.1).abs() < 0.01
}

#[test]
fn a_callout_sits_centered_over_the_bounds_it_names() {
    // Half the bounds, a 5 px gap, and half the callout's own 18 px.
    let offset = callout_offset((320.0, 240.0), (20.0, 12.0), 18, CANVAS);
    assert!(close(offset, (0.0, -26.0)), "{offset:?}");
}

#[test]
fn a_callout_the_canvas_clips_above_sits_below() {
    let offset = callout_offset((320.0, 14.0), (20.0, 12.0), 18, CANVAS);
    assert!(close(offset, (0.0, 26.0)), "{offset:?}");
}

#[test]
fn a_callout_with_no_room_either_way_stays_inside_the_top_edge() {
    // Bounds as tall as the canvas: the callout keeps its margin from the
    // top edge instead of leaving the canvas.
    let offset = callout_offset((320.0, 240.0), (40.0, 240.0), 18, CANVAS);
    assert!(close(offset, (0.0, 13.0 - 240.0)), "{offset:?}");
}

#[test]
fn a_callout_slides_sideways_to_keep_its_width_on_the_canvas() {
    // 20 characters draw 124 px wide, so the center stays 66 px in.
    let left = callout_offset((10.0, 240.0), (8.0, 8.0), 20, CANVAS);
    assert!(close(left, (56.0, -22.0)), "{left:?}");
    let right = callout_offset((630.0, 240.0), (8.0, 8.0), 20, CANVAS);
    assert!(close(right, (-56.0, -22.0)), "{right:?}");
    let inside = callout_offset((300.0, 240.0), (8.0, 8.0), 20, CANVAS);
    assert!(close(inside, (0.0, -22.0)), "{inside:?}");
}

#[test]
fn a_callout_wider_than_the_canvas_centers_on_it() {
    let offset = callout_offset((100.0, 240.0), (8.0, 8.0), 200, CANVAS);
    assert!(close(offset, (220.0, -22.0)), "{offset:?}");
}

#[test]
fn a_group_is_bounded_by_the_boxes_at_its_edges() {
    let bounds = union_bounds([
        ((100.0, 100.0), (10.0, 20.0)),
        ((200.0, 160.0), (30.0, 10.0)),
        ((150.0, 120.0), (5.0, 5.0)),
    ]);
    let (center, reach) = bounds.expect("three boxes have bounds");
    assert!(close(center, (160.0, 125.0)), "{center:?}");
    assert!(close(reach, (70.0, 45.0)), "{reach:?}");
    assert_eq!(union_bounds([]), None);
}

#[test]
fn a_group_caption_names_its_device_and_counts_its_outputs() {
    assert_eq!(
        group_caption(Some("Canvas 449E"), 21),
        "Canvas 449E \u{b7} 21 outputs"
    );
    assert_eq!(group_caption(None, 7), "7 outputs");
}

#[test]
fn polygon_panels_do_not_capture_the_empty_corners_of_their_boxes() {
    use hypercolor_types::spatial::ZoneShape;
    let mut panel = ring_output("triangle");
    panel.topology = LedTopology::Point;
    panel.shape = Some(ZoneShape::Custom {
        vertices: vec![
            NormalizedPosition::new(0.5, 0.0),
            NormalizedPosition::new(1.0, 1.0),
            NormalizedPosition::new(0.0, 1.0),
        ],
    });
    let hit = HitBox::from_output(&panel, 4.0 / 3.0, 1);
    assert!(
        !hit.circular,
        "a panel with one addressable color is not a circle"
    );
    assert!(hit.contains(hit.center));
    assert!(!hit.contains((
        hit.center.0 - hit.half.0 * 0.9,
        hit.center.1 - hit.half.1 * 0.9
    )));
    assert!(
        hit.contains((hit.center.0, hit.center.1 - hit.half.1)),
        "the tip belongs to the panel"
    );
    panel.rotation = std::f32::consts::FRAC_PI_2;
    let turned = HitBox::from_output(&panel, 4.0 / 3.0, 1);
    assert!(turned.contains((turned.center.0 + turned.half.1, turned.center.1)));
    assert!(!turned.contains((
        turned.center.0 + turned.half.1 * 0.9,
        turned.center.1 - turned.half.0 * 0.9
    )));
}

#[test]
fn polygon_boundaries_support_concave_shapes_and_reject_invalid_vertices() {
    use hypercolor_types::spatial::ZoneShape;
    use hypercolor_ui::components::layout_canvas::polygons;
    let corners = vec![
        NormalizedPosition::new(0.0, 0.0),
        NormalizedPosition::new(1.0, 0.0),
        NormalizedPosition::new(1.0, 0.3),
        NormalizedPosition::new(0.3, 0.3),
        NormalizedPosition::new(0.3, 1.0),
        NormalizedPosition::new(0.0, 1.0),
    ];
    assert!(polygons::contains(
        &corners,
        NormalizedPosition::new(0.1, 0.8)
    ));
    assert!(!polygons::contains(
        &corners,
        NormalizedPosition::new(0.8, 0.8)
    ));
    assert!(polygons::svg_points(Some(&ZoneShape::Custom { vertices: corners })).is_some());
    let invalid = ZoneShape::Custom {
        vertices: vec![NormalizedPosition::new(f32::NAN, 0.0); 3],
    };
    assert!(polygons::svg_points(Some(&invalid)).is_none());
}
