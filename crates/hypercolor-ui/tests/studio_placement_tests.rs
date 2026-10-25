//! Contract tests for where outputs land on a zone canvas: rig seeding,
//! the first-arrival fit, and the Fit transform.

use hypercolor_types::api::scene::SceneDocument;
use hypercolor_types::spatial::{
    EdgeBehavior, LedTopology, NormalizedPosition, Output, OutputComponent, SamplingMode,
    SpatialLayout, StripDirection,
};
use hypercolor_ui::api::DeviceSummary;
use hypercolor_ui::pages::studio::placement::{
    Arrival, FIT_MARGIN, Fallback, Footprint, assignment_hints, fit_outputs, move_edits,
    place_arrivals, rig_footprint, rig_layouts,
};

const CANVAS: (u32, u32) = (640, 480);
const ASPECT: f32 = 640.0 / 480.0;
const EPSILON: f32 = 1e-4;

fn output(id: &str, device: &str, segment: &str, x: f32, y: f32, w: f32, h: f32) -> Output {
    Output {
        id: id.to_owned(),
        name: id.to_owned(),
        device_id: device.to_owned(),
        zone_name: Some(segment.to_owned()),
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

fn component(mut output: Output, slot: &str, instance: u32) -> Output {
    output.attachment = Some(OutputComponent {
        template_id: "fan".to_owned(),
        slot_id: slot.to_owned(),
        instance,
        led_start: None,
        led_count: None,
        led_mapping: None,
    });
    output
}

fn layout(outputs: Vec<Output>) -> SpatialLayout {
    SpatialLayout {
        id: "rig".to_owned(),
        name: "Rig".to_owned(),
        description: None,
        canvas_width: 640,
        canvas_height: 480,
        zones: outputs,
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        version: 1,
    }
}

fn arrival(id: Option<&str>, device: &str, segment: &str, fallback: Fallback) -> Arrival {
    Arrival {
        output_id: id.map(str::to_owned),
        device_id: device.to_owned(),
        segment: Some(segment.to_owned()),
        fallback,
    }
}

fn assert_close(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() < EPSILON,
        "expected {expected}, got {actual}"
    );
}

fn assert_position(footprint: Footprint, x: f32, y: f32) {
    assert_close(footprint.position.x, x);
    assert_close(footprint.position.y, y);
}

fn landed(landings: &[Option<Footprint>], index: usize) -> Footprint {
    landings[index].expect("arrival is placed")
}

// ── Rig seeding ─────────────────────────────────────────────────────────

#[test]
fn arrival_lands_at_its_rig_position_beside_members_at_theirs() {
    let rig = vec![vec![
        output("fan", "case", "Fan", 0.2, 0.3, 0.1, 0.1),
        output("strip", "desk", "Main", 0.7, 0.6, 0.3, 0.05),
    ]];
    let members = [output("fan", "case", "Fan", 0.2, 0.3, 0.1, 0.1)];
    let landings = place_arrivals(
        &rig,
        &members,
        &[arrival(None, "desk", "Main", Fallback::None)],
        CANVAS,
    );
    let strip = landed(&landings, 0);
    assert_position(strip, 0.7, 0.6);
    assert_close(strip.size.x, 0.3);
    assert_close(strip.size.y, 0.05);
}

#[test]
fn arrival_wears_the_scale_and_offset_its_zone_members_wear() {
    let rig = vec![vec![
        output("a", "case", "A", 0.4, 0.4, 0.1, 0.1),
        output("b", "case", "B", 0.6, 0.6, 0.1, 0.1),
        output("c", "desk", "Main", 0.5, 0.5, 0.1, 0.1),
    ]];
    // The zone doubled the members' spread around the canvas center.
    let members = [
        output("a", "case", "A", 0.3, 0.3, 0.2, 0.2),
        output("b", "case", "B", 0.7, 0.7, 0.2, 0.2),
    ];
    let landings = place_arrivals(
        &rig,
        &members,
        &[arrival(Some("c"), "desk", "Main", Fallback::None)],
        CANVAS,
    );
    let c = landed(&landings, 0);
    assert_position(c, 0.5, 0.5);
    assert_close(c.size.x, 0.2);
    assert_close(c.size.y, 0.2);
}

#[test]
fn a_device_the_rig_does_not_know_is_left_to_the_daemon() {
    let rig = vec![vec![output("fan", "case", "Fan", 0.2, 0.3, 0.1, 0.1)]];
    let members = [output("fan", "case", "Fan", 0.2, 0.3, 0.1, 0.1)];
    let default = Footprint::of(&output("x", "new", "Main", 0.5, 0.5, 0.2, 0.1));
    let seeded = Footprint::of(&output("x", "new", "Pads", 0.3, 0.7, 0.2, 0.2));
    let landings = place_arrivals(
        &rig,
        &members,
        &[
            arrival(None, "new", "Main", Fallback::None),
            arrival(None, "new", "Main", Fallback::Default(default)),
            arrival(None, "new", "Pads", Fallback::Placed(seeded)),
        ],
        CANVAS,
    );
    assert_eq!(landings[0], None);
    assert_eq!(
        landings[1], None,
        "defaults only seed a zone's first arrival"
    );
    assert_eq!(
        landings[2],
        Some(seeded),
        "a known placement lands as it is"
    );
}

#[test]
fn every_output_of_a_device_is_seeded_from_its_own_segment() {
    let rig = vec![vec![
        output("top", "nollie", "Channel 1", 0.3, 0.2, 0.4, 0.05),
        output("side", "nollie", "Channel 2", 0.8, 0.5, 0.05, 0.4),
    ]];
    let members = [output("other", "desk", "Main", 0.5, 0.9, 0.2, 0.05)];
    let landings = place_arrivals(
        &rig,
        &members,
        &[
            arrival(None, "nollie", "Channel 1", Fallback::None),
            arrival(None, "nollie", "Channel 2", Fallback::None),
        ],
        CANVAS,
    );
    assert_position(landed(&landings, 0), 0.3, 0.2);
    assert_position(landed(&landings, 1), 0.8, 0.5);
}

#[test]
fn a_segment_split_into_components_spans_every_instance() {
    let rig = vec![vec![
        output("raw", "nollie", "Channel 2", 0.1, 0.1, 0.05, 0.05),
        component(
            output("fan-0", "nollie", "channel-2", 0.2, 0.5, 0.1, 0.1),
            "channel-2",
            0,
        ),
        component(
            output("fan-1", "nollie", "channel-2", 0.6, 0.5, 0.1, 0.1),
            "channel-2",
            1,
        ),
    ]];
    let span = rig_footprint(&rig, None, "nollie", Some("Channel 2"), ASPECT).expect("found");
    assert_position(span, 0.4, 0.5);
    assert_close(span.size.x, 0.5);
    assert_close(span.size.y, 0.1);
}

#[test]
fn rig_lookup_prefers_the_output_id_over_its_segment() {
    let rig = vec![vec![
        output("first", "case", "Lighting", 0.1, 0.1, 0.1, 0.1),
        output("second", "case", "Lighting", 0.9, 0.9, 0.1, 0.1),
    ]];
    let by_id = rig_footprint(&rig, Some("second"), "case", Some("Lighting"), ASPECT);
    assert_position(by_id.expect("found"), 0.9, 0.9);
    let by_segment = rig_footprint(&rig, Some("unknown"), "case", Some("Lighting"), ASPECT);
    assert_position(by_segment.expect("found"), 0.1, 0.1);
}

#[test]
fn an_arrival_carried_off_the_canvas_is_pulled_back_on() {
    let rig = vec![vec![
        output("a", "case", "A", 0.5, 0.5, 0.1, 0.1),
        output("far", "desk", "Main", 0.95, 0.5, 0.1, 0.1),
    ]];
    // The zone blew its member up four times around the center.
    let members = [output("a", "case", "A", 0.5, 0.5, 0.4, 0.4)];
    let landings = place_arrivals(
        &rig,
        &members,
        &[arrival(Some("far"), "desk", "Main", Fallback::None)],
        CANVAS,
    );
    let bounds = landed(&landings, 0).bounds(ASPECT);
    assert!(bounds.min_x >= -EPSILON && bounds.max_x <= ASPECT + EPSILON);
    assert!(bounds.min_y >= -EPSILON && bounds.max_y <= 1.0 + EPSILON);
}

#[test]
fn a_single_zone_scene_reads_its_primary_zone_before_the_active_layout() {
    let single: SceneDocument = scene(&[("primary", &[("fan", "case", "Fan", 0.2, 0.2)])], &[]);
    let rig = layout(vec![output("fan", "case", "Fan", 0.9, 0.9, 0.1, 0.1)]);
    let sources = rig_layouts(&single, Some(&rig));
    assert_eq!(sources.len(), 2);
    assert_close(sources[0][0].position.x, 0.2);

    let multi = scene(
        &[
            ("primary", &[("fan", "case", "Fan", 0.2, 0.2)]),
            ("custom", &[]),
        ],
        &[],
    );
    let sources = rig_layouts(&multi, Some(&rig));
    assert_eq!(sources.len(), 1);
    assert_close(sources[0][0].position.x, 0.9);
}

// ── First arrivals and Fit ──────────────────────────────────────────────

#[test]
fn first_arrivals_fill_the_canvas_inside_the_margin() {
    let rig = vec![vec![
        output("grid", "roli", "Grid", 0.15, 0.15, 0.1, 0.1),
        output("keys", "keys", "Keys", 0.25, 0.15, 0.1, 0.05),
    ]];
    let spread = |grid: Footprint, keys: Footprint| {
        let (grid, keys) = (grid.bounds(ASPECT), keys.bounds(ASPECT));
        (keys.max_x - grid.min_x) / (grid.max_y - grid.min_y)
    };
    let before = spread(Footprint::of(&rig[0][0]), Footprint::of(&rig[0][1]));
    let landings = place_arrivals(
        &rig,
        &[],
        &[
            arrival(None, "roli", "Grid", Fallback::None),
            arrival(None, "keys", "Keys", Fallback::None),
        ],
        CANVAS,
    );
    let grid = landed(&landings, 0).bounds(ASPECT);
    let keys = landed(&landings, 1).bounds(ASPECT);
    let width = keys.max_x - grid.min_x;
    let height = grid.max_y - grid.min_y;
    let usable = 1.0 - 2.0 * FIT_MARGIN;
    // The group touches the margin on its tighter axis and is centered.
    assert!((width - ASPECT * usable).abs() < EPSILON || (height - usable).abs() < EPSILON);
    assert_close((grid.min_x + keys.max_x) * 0.5, ASPECT * 0.5);
    assert_close((grid.min_y + grid.max_y) * 0.5, 0.5);
    // The arrangement survives the scale.
    assert_close(spread(landed(&landings, 0), landed(&landings, 1)), before);
}

#[test]
fn a_first_arrival_the_rig_does_not_know_fits_its_default() {
    let default = Footprint::of(&output("x", "new", "Main", 0.5, 0.5, 0.3, 0.05));
    let landings = place_arrivals(
        &[],
        &[],
        &[arrival(None, "new", "Main", Fallback::Default(default))],
        CANVAS,
    );
    let fitted = landed(&landings, 0);
    assert_position(fitted, 0.5, 0.5);
    assert_close(fitted.size.x, 1.0 - 2.0 * FIT_MARGIN);
}

#[test]
fn fit_scales_uniformly_and_centers_the_group() {
    let mut outputs = vec![
        output("a", "case", "A", 0.1, 0.1, 0.1, 0.1),
        output("b", "case", "B", 0.3, 0.2, 0.1, 0.1),
    ];
    let before_ratio = {
        let a = Footprint::of(&outputs[0]).bounds(ASPECT);
        let b = Footprint::of(&outputs[1]).bounds(ASPECT);
        (b.max_x - a.min_x) / (b.max_y - a.min_y)
    };
    assert!(fit_outputs(&mut outputs, CANVAS));
    let a = Footprint::of(&outputs[0]).bounds(ASPECT);
    let b = Footprint::of(&outputs[1]).bounds(ASPECT);
    let (width, height) = (b.max_x - a.min_x, b.max_y - a.min_y);
    assert_close(width / height, before_ratio);
    assert_close((a.min_x + b.max_x) * 0.5, ASPECT * 0.5);
    assert_close((a.min_y + b.max_y) * 0.5, 0.5);
    let usable = 1.0 - 2.0 * FIT_MARGIN;
    assert!(width <= ASPECT * usable + EPSILON && height <= usable + EPSILON);
    assert!((width - ASPECT * usable).abs() < EPSILON || (height - usable).abs() < EPSILON);
    // Both boxes grew by the same factor.
    assert_close(outputs[0].size.x / outputs[0].size.y, 1.0);
    assert_close(outputs[1].size.x, outputs[0].size.x);
}

#[test]
fn fit_fills_the_canvas_with_a_single_member() {
    let mut outputs = vec![output("keyboard", "kb", "Main", 0.2, 0.2, 0.34, 0.13)];
    assert!(fit_outputs(&mut outputs, CANVAS));
    let fitted = Footprint::of(&outputs[0]);
    assert_position(fitted, 0.5, 0.5);
    assert_close(fitted.size.x, 1.0 - 2.0 * FIT_MARGIN);
    assert_close(fitted.size.x / fitted.size.y, 0.34 / 0.13);
}

#[test]
fn fit_measures_rotated_boxes_by_their_drawn_extent() {
    let mut strip = output("strip", "case", "Main", 0.5, 0.5, 0.3, 0.02);
    strip.rotation = std::f32::consts::FRAC_PI_2;
    let mut outputs = vec![strip];
    assert!(fit_outputs(&mut outputs, CANVAS));
    let bounds = Footprint::of(&outputs[0]).bounds(ASPECT);
    // Turned upright, the strip's length runs down the canvas.
    assert_close(bounds.height(), 1.0 - 2.0 * FIT_MARGIN);
    assert_close(outputs[0].rotation, std::f32::consts::FRAC_PI_2);
}

#[test]
fn fit_on_an_empty_zone_reports_nothing_to_fit() {
    assert!(!fit_outputs(&mut [], CANVAS));
}

// ── Assignment and move adapters ────────────────────────────────────────

type MemberSpec<'a> = (&'a str, &'a str, &'a str, f32, f32);

fn scene(zones: &[(&str, &[MemberSpec<'_>])], displays: &[&str]) -> SceneDocument {
    let ids = [
        "11111111-1111-4111-8111-111111111111",
        "22222222-2222-4222-8222-222222222222",
        "44444444-4444-4444-8444-444444444444",
    ];
    let mut json_zones: Vec<serde_json::Value> = zones
        .iter()
        .zip(ids)
        .map(|((role, members), id)| {
            serde_json::json!({
                "id": id, "name": role, "role": role, "enabled": true, "brightness": 1,
                "members": members.iter().map(|(member, device, segment, _, _)| serde_json::json!({
                    "id": member, "device_id": device, "segment": segment, "name": member,
                })).collect::<Vec<_>>(),
                "layout": { "placements": members.iter().map(|(member, _, _, x, y)| serde_json::json!({
                    "member": member, "position": { "x": x, "y": y }, "size": { "x": 0.1, "y": 0.1 },
                    "topology": { "type": "strip", "count": 10, "direction": "left_to_right" },
                })).collect::<Vec<_>>() },
                "layers": [],
            })
        })
        .collect();
    for (index, name) in displays.iter().enumerate() {
        json_zones.push(serde_json::json!({
            "id": format!("55555555-5555-4555-8555-55555555555{index}"), "name": name,
            "role": "display", "enabled": true, "brightness": 1, "members": [], "layers": [],
        }));
    }
    serde_json::from_value(serde_json::json!({
        "id": "33333333-3333-4333-8333-333333333333", "name": "Fixture", "kind": "named",
        "is_default": false, "revision": 7, "zones": json_zones,
    }))
    .expect("valid scene fixture")
}

fn device(id: &str, segments: serde_json::Value) -> DeviceSummary {
    serde_json::from_value(serde_json::json!({
        "id": id, "layout_device_id": id, "name": "Fixture device",
        "origin": { "driver_id": "fixture", "backend_id": "usb", "transport": "usb", "protocol_id": "fixture/test" },
        "presentation": { "label": "Fixture" }, "status": "connected", "brightness": 100,
        "total_leds": 40, "segments": segments,
    }))
    .expect("valid device fixture")
}

#[test]
fn assignment_hints_seed_each_light_segment_from_the_rig() {
    let scene = scene(
        &[
            ("primary", &[("fan", "case", "Fan", 0.2, 0.3)]),
            ("custom", &[]),
        ],
        &[],
    );
    let rig = layout(vec![
        output("fan", "case", "Fan", 0.2, 0.3, 0.1, 0.1),
        output("top", "strip", "Top", 0.4, 0.1, 0.3, 0.04),
        output("bottom", "strip", "Bottom", 0.4, 0.9, 0.3, 0.04),
    ]);
    let strip = device(
        "strip",
        serde_json::json!([
            { "id": "top", "name": "Top", "led_count": 20, "topology": "strip" },
            { "id": "bottom", "name": "Bottom", "led_count": 20, "topology": "strip" },
            { "id": "lcd", "name": "Screen", "led_count": 1, "topology": "display",
              "topology_hint": { "type": "display", "width": 480, "height": 480, "circular": true } },
        ]),
    );
    let zone_id = scene.zones[0].id.to_string();
    let hints = assignment_hints(&scene, Some(&rig), &strip, &zone_id, CANVAS);
    assert_eq!(hints.len(), 2, "the screen segment mints no LED output");
    assert_eq!(hints[0].segment.as_deref(), Some("Top"));
    assert_close(hints[0].position.y, 0.1);
    assert_eq!(hints[1].segment.as_deref(), Some("Bottom"));
    assert_close(hints[1].position.y, 0.9);
}

#[test]
fn assignment_hints_leave_an_unknown_device_in_a_populated_zone_to_the_daemon() {
    let scene = scene(
        &[
            ("primary", &[("fan", "case", "Fan", 0.2, 0.3)]),
            ("custom", &[]),
        ],
        &[],
    );
    let lamp = device(
        "lamp",
        serde_json::json!([{ "id": "main", "name": "Main", "led_count": 30, "topology": "strip" }]),
    );
    let zone_id = scene.zones[0].id.to_string();
    assert!(assignment_hints(&scene, None, &lamp, &zone_id, CANVAS).is_empty());
}

#[test]
fn assignment_hints_fit_a_zone_s_first_device() {
    let scene = scene(
        &[
            ("primary", &[("fan", "case", "Fan", 0.2, 0.3)]),
            ("custom", &[]),
        ],
        &[],
    );
    let lamp = device(
        "lamp",
        serde_json::json!([{ "id": "main", "name": "Main", "led_count": 30, "topology": "strip" }]),
    );
    let zone_id = scene.zones[1].id.to_string();
    let hints = assignment_hints(&scene, None, &lamp, &zone_id, CANVAS);
    assert_eq!(hints.len(), 1);
    assert_close(hints[0].position.x, 0.5);
    assert_close(hints[0].position.y, 0.5);
    let footprint = Footprint {
        position: hints[0].position,
        size: hints[0].size,
        rotation: hints[0].rotation,
        scale: hints[0].scale,
        orientation: hints[0].orientation,
    };
    let bounds = footprint.bounds(ASPECT);
    let usable = 1.0 - 2.0 * FIT_MARGIN;
    assert!(
        (bounds.width() - ASPECT * usable).abs() < EPSILON
            || (bounds.height() - usable).abs() < EPSILON
    );
}

#[test]
fn moving_into_an_empty_zone_fits_the_moved_outputs_and_keeps_their_source() {
    let scene = scene(
        &[
            (
                "primary",
                &[
                    ("fan", "case", "Fan", 0.2, 0.3),
                    ("kb", "keyboard", "Main", 0.6, 0.8),
                ],
            ),
            ("custom", &[]),
        ],
        &[],
    );
    let target = scene.zones[1].id.to_string();
    let edits = move_edits(&scene, None, &target, &["kb".to_owned()], CANVAS);
    assert_eq!(edits.len(), 1);
    let before = edits[0].before.as_ref().expect("before state");
    assert_eq!(before.zone_id, scene.zones[0].id);
    assert_eq!(before.index, 1);
    assert_close(before.output.position.x, 0.6);
    let after = edits[0].after.as_ref().expect("after state");
    assert_eq!(after.zone_id, scene.zones[1].id);
    assert_eq!(after.index, 0);
    assert_close(after.output.position.x, 0.5);
    assert_close(after.output.position.y, 0.5);
    assert!(after.output.size.x > before.output.size.x);
    assert_eq!(after.output.device_id, before.output.device_id);
    assert_eq!(after.output.zone_name, before.output.zone_name);
}

#[test]
fn moving_into_a_populated_zone_lands_at_the_rig_position() {
    let scene = scene(
        &[
            ("primary", &[("kb", "keyboard", "Main", 0.6, 0.8)]),
            ("custom", &[("fan", "case", "Fan", 0.2, 0.3)]),
        ],
        &["Screen"],
    );
    let rig = layout(vec![
        output("fan", "case", "Fan", 0.2, 0.3, 0.1, 0.1),
        output("kb", "keyboard", "Main", 0.4, 0.7, 0.1, 0.1),
    ]);
    let target = scene.zones[1].id.to_string();
    let edits = move_edits(&scene, Some(&rig), &target, &["kb".to_owned()], CANVAS);
    let after = edits[0].after.as_ref().expect("after state");
    assert_close(after.output.position.x, 0.4);
    assert_close(after.output.position.y, 0.7);
    assert_eq!(after.index, 1);
}
