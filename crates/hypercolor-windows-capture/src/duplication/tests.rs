use super::{
    PointerShape, PointerShapeKind, PointerState, TopologyEntry, TopologyState, average_channel,
    logical_to_scanout, pointer_scanout_geometry, scanout_to_logical,
};
use crate::DisplayRotation;

fn topology_entry(id: &str, origin_x: i32) -> TopologyEntry {
    TopologyEntry {
        id: id.to_owned(),
        origin_x,
        origin_y: 0,
        width: 1920,
        height: 1080,
        primary: origin_x == 0,
        rotation: DisplayRotation::Identity,
    }
}

fn pointer(kind: PointerShapeKind, bytes: Vec<u8>) -> PointerState {
    PointerState {
        visible: true,
        position_x: 0,
        position_y: 0,
        shape: Some(PointerShape {
            kind,
            width: 1,
            height: if kind == PointerShapeKind::Monochrome {
                2
            } else {
                1
            },
            pitch: if kind == PointerShapeKind::Monochrome {
                1
            } else {
                4
            },
            hotspot_x: 0,
            hotspot_y: 0,
            bytes,
        }),
        shape_generation: 1,
    }
}

#[test]
fn color_pointer_alpha_blends_bgra() {
    let pointer = pointer(PointerShapeKind::Color, vec![200, 100, 50, 128]);

    assert_eq!(
        pointer.composite_bgra([20, 40, 60, 255], 0, 0, 1, 1, DisplayRotation::Identity),
        [110, 70, 55, 255]
    );
}

#[test]
fn monochrome_pointer_applies_and_then_xor_masks() {
    let invert = pointer(PointerShapeKind::Monochrome, vec![0x80, 0x80]);
    let black = pointer(PointerShapeKind::Monochrome, vec![0x00, 0x00]);

    assert_eq!(
        invert.composite_bgra(
            [0x11, 0x22, 0x33, 0xFF],
            0,
            0,
            1,
            1,
            DisplayRotation::Identity
        ),
        [0xEE, 0xDD, 0xCC, 0xFF]
    );
    assert_eq!(
        black.composite_bgra(
            [0x11, 0x22, 0x33, 0xFF],
            0,
            0,
            1,
            1,
            DisplayRotation::Identity
        ),
        [0, 0, 0, 0xFF]
    );
}

#[test]
fn masked_color_pointer_selects_copy_or_xor_by_alpha_mask() {
    let copy = pointer(PointerShapeKind::MaskedColor, vec![1, 2, 3, 0]);
    let xor = pointer(PointerShapeKind::MaskedColor, vec![0xFF, 0x0F, 0xF0, 0xFF]);

    assert_eq!(
        copy.composite_bgra(
            [0x10, 0x20, 0x30, 0xFF],
            0,
            0,
            1,
            1,
            DisplayRotation::Identity
        ),
        [1, 2, 3, 0xFF]
    );
    assert_eq!(
        xor.composite_bgra(
            [0x10, 0x20, 0x30, 0xFF],
            0,
            0,
            1,
            1,
            DisplayRotation::Identity
        ),
        [0xEF, 0x2F, 0xC0, 0xFF]
    );
}

#[test]
fn hidden_or_already_composed_pointer_does_not_modify_desktop() {
    let mut pointer = pointer(PointerShapeKind::Color, vec![255, 255, 255, 255]);
    pointer.visible = false;
    let desktop = [1, 2, 3, 255];

    assert_eq!(
        pointer.composite_bgra(desktop, 0, 0, 1, 1, DisplayRotation::Identity),
        desktop
    );
    assert!(
        pointer
            .cursor_info(1, 1, DisplayRotation::Identity)
            .composed
    );
}

#[test]
fn pointer_coordinates_round_trip_for_every_rotation() {
    for rotation in [
        DisplayRotation::Identity,
        DisplayRotation::Clockwise90,
        DisplayRotation::Clockwise180,
        DisplayRotation::Clockwise270,
    ] {
        for (x, y) in [(0, 0), (1, 2), (3, 1)] {
            let logical = scanout_to_logical(x, y, 4, 3, rotation);
            assert_eq!(
                logical_to_scanout(logical.0, logical.1, 4, 3, rotation),
                (i64::from(x), i64::from(y))
            );
        }
    }
}

#[test]
fn rotated_pointer_geometry_stays_in_native_scanout_space() {
    let geometry = pointer_scanout_geometry(2, 1, 3, 2, 1, 1, 8, 6, DisplayRotation::Clockwise90);

    assert_eq!(geometry, (1, 1, 2, 3, 1, 1));
}

#[test]
fn moving_pointer_recomposes_from_clean_desktop_without_residue() {
    let desktop = [10, 20, 30, 255];
    let mut pointer = pointer(PointerShapeKind::Color, vec![200, 100, 50, 255]);
    assert_ne!(
        pointer.composite_bgra(desktop, 0, 0, 2, 1, DisplayRotation::Identity),
        desktop
    );

    pointer.position_x = 1;
    assert_eq!(
        pointer.composite_bgra(desktop, 0, 0, 2, 1, DisplayRotation::Identity),
        desktop
    );
    assert_ne!(
        pointer.composite_bgra(desktop, 1, 0, 2, 1, DisplayRotation::Identity),
        desktop
    );
}

#[test]
fn visible_pointer_composites_at_the_rotated_scanout_pixel() {
    let mut pointer = pointer(PointerShapeKind::Color, vec![200, 100, 50, 255]);
    pointer.position_x = 2;
    pointer.position_y = 1;
    let scanout = logical_to_scanout(2, 1, 8, 6, DisplayRotation::Clockwise90);

    assert_eq!(
        pointer.composite_bgra(
            [10, 20, 30, 255],
            scanout.0 as u32,
            scanout.1 as u32,
            8,
            6,
            DisplayRotation::Clockwise90,
        ),
        [200, 100, 50, 255]
    );
}

#[test]
fn truncated_pointer_shapes_are_rejected_before_composition() {
    let color = PointerShape {
        kind: PointerShapeKind::Color,
        width: 2,
        height: 1,
        pitch: 8,
        hotspot_x: 0,
        hotspot_y: 0,
        bytes: vec![0; 4],
    };
    let monochrome = PointerShape {
        kind: PointerShapeKind::Monochrome,
        width: 9,
        height: 2,
        pitch: 1,
        hotspot_x: 0,
        hotspot_y: 0,
        bytes: vec![0; 2],
    };

    assert!(color.validate().is_err());
    assert!(monochrome.validate().is_err());
}

#[test]
fn topology_generation_changes_only_when_descriptor_content_changes() {
    let mut state = TopologyState::default();
    let mut initial = vec![topology_entry("left", -1920), topology_entry("main", 0)];
    initial.sort_unstable_by(|left, right| left.id.cmp(&right.id));

    assert_eq!(state.observe(initial.clone()), 1);
    assert_eq!(state.observe(initial.clone()), 1);

    let mut reordered = initial.clone();
    reordered.reverse();
    reordered.sort_unstable_by(|left, right| left.id.cmp(&right.id));
    assert_eq!(state.observe(reordered), 1);

    assert_eq!(state.observe(vec![topology_entry("main", 0)]), 2);
}

#[test]
fn channel_average_handles_a_single_full_8k_reduction_box() {
    let samples = 7_680_u64 * 4_320;
    let sum = samples * u64::from(u8::MAX);

    assert!(sum > u64::from(u32::MAX));
    assert_eq!(average_channel(sum, samples), u8::MAX);
}

#[test]
fn channel_average_handles_the_maximum_d3d11_surface() {
    let samples = 16_384_u64 * 16_384;
    let sum = samples * u64::from(u8::MAX);

    assert_eq!(average_channel(sum, samples), u8::MAX);
}

#[test]
fn channel_average_defends_against_an_empty_box() {
    assert_eq!(average_channel(0, 0), 0);
}
