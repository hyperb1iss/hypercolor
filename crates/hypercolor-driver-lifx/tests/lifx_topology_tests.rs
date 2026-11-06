//! Zone topology to segments and LED positions (Spec 88 §6.4 and §7).

use std::net::{Ipv4Addr, SocketAddr};

use hypercolor_driver_lifx::products::{MatrixQuirk, lookup_product};
use hypercolor_driver_lifx::protocol::{FirmwareVersion, Serial};
use hypercolor_driver_lifx::scanner::max_fps_for;
use hypercolor_driver_lifx::topology::{
    LifxDeviceProfile, LifxTopology, MatrixSlot, Orientation, TileGeometry, segments_for,
};
use hypercolor_types::device::DeviceTopologyHint;
use hypercolor_types::spatial::LedTopology;

fn tile(width: u8, height: u8, user: (f32, f32), orientation: Orientation) -> TileGeometry {
    TileGeometry {
        width,
        height,
        user_x: user.0,
        user_y: user.1,
        orientation,
        supported_frame_buffers: 1,
    }
}

fn matrix(tiles: Vec<TileGeometry>) -> LifxTopology {
    LifxTopology::Matrix {
        tiles,
        quirk: MatrixQuirk::None,
        uplight: None,
    }
}

fn custom_positions(topology: &LifxTopology) -> Vec<(f32, f32)> {
    let segments = segments_for(topology);
    let hint = segments[0].layout_hint.as_ref().expect("layout hint");
    let Some(LedTopology::Custom { positions }) = &hint.topology else {
        panic!("expected custom positions");
    };
    positions.iter().map(|point| (point.x, point.y)).collect()
}

#[test]
fn orientation_follows_the_documented_accelerometer_rules() {
    assert_eq!(
        Orientation::from_accel(-1, -1, -1),
        Orientation::RightSideUp
    );
    assert_eq!(
        Orientation::from_accel(0, -100, 0),
        Orientation::RightSideUp
    );
    assert_eq!(Orientation::from_accel(0, 100, 0), Orientation::UpsideDown);
    assert_eq!(
        Orientation::from_accel(100, 0, 0),
        Orientation::RotatedRight
    );
    assert_eq!(
        Orientation::from_accel(-100, 10, 0),
        Orientation::RotatedLeft
    );
    assert_eq!(Orientation::from_accel(0, 0, 100), Orientation::FaceDown);
    assert_eq!(Orientation::from_accel(0, 10, -100), Orientation::FaceUp);
}

/// The documented image-to-device remap for an 8 x 8 tile.
fn documented_device_zone(orientation: Orientation, x: u32, y: u32) -> (u32, u32) {
    match orientation {
        Orientation::UpsideDown => (7 - x, 7 - y),
        Orientation::RotatedLeft => (7 - y, x),
        Orientation::RotatedRight => (y, 7 - x),
        _ => (x, y),
    }
}

#[test]
fn upright_position_inverts_the_documented_remap() {
    for orientation in [
        Orientation::RightSideUp,
        Orientation::UpsideDown,
        Orientation::RotatedLeft,
        Orientation::RotatedRight,
        Orientation::FaceUp,
        Orientation::FaceDown,
    ] {
        for y in 0..8 {
            for x in 0..8 {
                let (dx, dy) = documented_device_zone(orientation, x, y);
                assert_eq!(
                    orientation.upright_position(dx, dy, 8),
                    (x, y),
                    "{orientation:?} ({x}, {y})"
                );
            }
        }
    }
}

#[test]
fn single_zone_and_strip_segments() {
    let bulb = segments_for(&LifxTopology::SingleZone);
    assert_eq!(bulb.len(), 1);
    assert_eq!(bulb[0].led_count, 1);
    assert_eq!(bulb[0].topology, DeviceTopologyHint::Point);

    let strip = segments_for(&LifxTopology::Multizone {
        zones: 80,
        extended: true,
    });
    assert_eq!(strip.len(), 1);
    assert_eq!(strip[0].led_count, 80);
    assert_eq!(strip[0].topology, DeviceTopologyHint::Strip);
}

#[test]
fn upright_single_tile_is_a_plain_matrix() {
    let topology = matrix(vec![tile(5, 11, (0.0, 0.0), Orientation::RightSideUp)]);
    let segments = segments_for(&topology);
    assert_eq!(segments.len(), 1);
    assert_eq!(segments[0].led_count, 55);
    assert_eq!(
        segments[0].topology,
        DeviceTopologyHint::Matrix { rows: 11, cols: 5 }
    );
    assert!(segments[0].layout_hint.is_none());
    let layout = topology.matrix_layout().expect("matrix layout");
    assert_eq!(layout.slots[54], MatrixSlot { tile: 0, zone: 54 });
}

#[test]
fn chain_positions_follow_user_coordinates() {
    let topology = matrix(vec![
        tile(8, 8, (0.0, 0.0), Orientation::RightSideUp),
        tile(8, 8, (1.0, 0.0), Orientation::RightSideUp),
    ]);
    let segments = segments_for(&topology);
    assert_eq!(segments.len(), 1);
    assert_eq!(segments[0].name, "Tiles");
    assert_eq!(segments[0].led_count, 128);
    assert_eq!(segments[0].topology, DeviceTopologyHint::Custom);
    let size = segments[0]
        .layout_hint
        .as_ref()
        .and_then(|hint| hint.size)
        .expect("chain size hint");
    assert!((size.x / size.y - 2.0).abs() < 0.01, "2:1 chain aspect");

    let positions = custom_positions(&topology);
    assert_eq!(positions.len(), 128);
    let first = positions[0];
    let second_tile_first = positions[64];
    assert!((first.0 - 0.5 / 16.0).abs() < 1e-4, "{first:?}");
    assert!((first.1 - 0.5 / 8.0).abs() < 1e-4, "{first:?}");
    assert!((second_tile_first.0 - 8.5 / 16.0).abs() < 1e-4);
    assert!(
        positions
            .iter()
            .all(|(x, y)| (0.0..=1.0).contains(x) && (0.0..=1.0).contains(y))
    );
}

#[test]
fn positive_user_y_is_above_on_the_canvas() {
    let topology = matrix(vec![
        tile(8, 8, (0.0, 0.0), Orientation::RightSideUp),
        tile(8, 8, (0.0, 1.0), Orientation::RightSideUp),
    ]);
    let positions = custom_positions(&topology);
    assert!(
        positions[64].1 < positions[0].1,
        "tile at user_y 1 sits higher"
    );
}

#[test]
fn unarranged_chains_lay_out_left_to_right() {
    let topology = matrix(vec![
        tile(8, 8, (0.0, 0.0), Orientation::RightSideUp),
        tile(8, 8, (0.0, 0.0), Orientation::RightSideUp),
        tile(8, 8, (0.0, 0.0), Orientation::RightSideUp),
    ]);
    let positions = custom_positions(&topology);
    assert!(positions[0].0 < positions[64].0);
    assert!(positions[64].0 < positions[128].0);
    assert!((positions[0].1 - positions[128].1).abs() < 1e-4);
}

#[test]
fn rotated_tiles_place_device_zones_where_they_physically_are() {
    let topology = matrix(vec![tile(8, 8, (0.0, 0.0), Orientation::RotatedRight)]);
    let positions = custom_positions(&topology);
    // Rotated right, device zone (0, 0) shows the upright pixel (7, 0).
    let zone0 = positions[0];
    assert!((zone0.0 - 7.5 / 8.0).abs() < 1e-4 && (zone0.1 - 0.5 / 8.0).abs() < 1e-4);
}

#[test]
fn candle_maps_a_top_zone_and_a_five_by_five_wrap() {
    let topology = LifxTopology::Matrix {
        tiles: vec![tile(5, 6, (0.0, 0.0), Orientation::RightSideUp)],
        quirk: MatrixQuirk::CandleTopZone,
        uplight: None,
    };
    let layout = topology.matrix_layout().expect("matrix layout");
    assert_eq!(layout.slots.len(), 26);
    assert_eq!(layout.slots[0].zone, 0);
    assert_eq!(layout.slots[1].zone, 5);
    assert_eq!(layout.slots[25].zone, 29);
    assert_eq!(layout.segments.len(), 1);
    assert_eq!(layout.segments[0].name, "Candle");
    let positions = custom_positions(&topology);
    assert_eq!(positions[0], (0.5, 0.0));
    assert_eq!(positions[1], (0.0, 0.2));
}

#[test]
fn ceiling_uplight_gets_its_own_segment() {
    for (pid, width, height) in [(176_u32, 8_u8, 8_u8), (201, 16, 8)] {
        let product = lookup_product(pid).expect("ceiling product");
        let topology = LifxTopology::Matrix {
            tiles: vec![tile(width, height, (0.0, 0.0), Orientation::RightSideUp)],
            quirk: product.quirk,
            uplight: product.uplight,
        };
        let layout = topology.matrix_layout().expect("matrix layout");
        let zones = u16::from(width) * u16::from(height);
        assert_eq!(layout.slots.len(), usize::from(zones));
        assert_eq!(layout.segments.len(), 2);
        assert_eq!(layout.segments[0].name, "Downlight");
        assert_eq!(layout.segments[0].led_count, u32::from(zones) - 1);
        assert_eq!(layout.segments[1].name, "Uplight");
        assert_eq!(layout.segments[1].topology, DeviceTopologyHint::Point);
        assert!(
            layout.segments[1]
                .layout_hint
                .as_ref()
                .is_some_and(|hint| hint.co_located)
        );
        let (x, y) = product.uplight.expect("uplight cell");
        let uplight_zone = u16::from(y) * u16::from(width) + u16::from(x);
        assert_eq!(
            layout.slots.last().map(|slot| slot.zone),
            Some(uplight_zone)
        );
        assert!(
            !layout.slots[..layout.slots.len() - 1]
                .iter()
                .any(|slot| slot.zone == uplight_zone)
        );
    }
}

fn profile(topology: LifxTopology, product_id: u32, label: &str) -> LifxDeviceProfile {
    LifxDeviceProfile {
        serial: Serial([0xd0, 0x73, 0xd5, 0x12, 0x34, 0x56]),
        addr: SocketAddr::from((Ipv4Addr::new(192, 0, 2, 10), 56_700)),
        label: label.to_owned(),
        vendor_id: 1,
        product_id,
        product: lookup_product(product_id),
        firmware: FirmwareVersion::new(3, 70),
        topology,
    }
}

#[test]
fn device_info_describes_the_light() {
    let profile = profile(
        LifxTopology::Multizone {
            zones: 82,
            extended: true,
        },
        38,
        "Desk Beam",
    );
    let info = profile.device_info(20);
    assert_eq!(info.id, profile.device_id());
    assert_eq!(info.name, "Desk Beam");
    assert_eq!(info.vendor, "LIFX");
    assert_eq!(info.model.as_deref(), Some("LIFX Beam"));
    assert_eq!(info.firmware_version.as_deref(), Some("3.70"));
    assert_eq!(info.total_led_count(), 82);
    assert_eq!(info.capabilities.max_fps, 20);
    assert!(info.capabilities.supports_brightness);

    let unlabeled = self::profile(LifxTopology::SingleZone, 9_999, "");
    assert_eq!(unlabeled.display_name(), "LIFX d073d5123456");
    assert_eq!(unlabeled.model_name(), "LIFX product 9999");
    assert!(unlabeled.color_capable());
}

#[test]
fn device_ids_are_stable_per_serial() {
    let first = profile(LifxTopology::SingleZone, 27, "A");
    let mut moved = profile(LifxTopology::SingleZone, 27, "B");
    moved.addr = SocketAddr::from((Ipv4Addr::new(192, 0, 2, 99), 56_700));
    assert_eq!(first.device_id(), moved.device_id());
}

#[test]
fn max_fps_follows_the_message_budget() {
    assert_eq!(max_fps_for(&LifxTopology::SingleZone, 20), 20);
    assert_eq!(
        max_fps_for(
            &LifxTopology::Multizone {
                zones: 120,
                extended: true,
            },
            20
        ),
        10
    );
    assert_eq!(
        max_fps_for(
            &LifxTopology::Multizone {
                zones: 16,
                extended: false,
            },
            20
        ),
        1
    );
    let chain = matrix(vec![tile(8, 8, (0.0, 0.0), Orientation::RightSideUp); 5]);
    assert_eq!(max_fps_for(&chain, 20), 4);
    let mut ceiling = tile(16, 8, (0.0, 0.0), Orientation::RightSideUp);
    ceiling.supported_frame_buffers = 2;
    assert_eq!(max_fps_for(&matrix(vec![ceiling]), 20), 6);
    assert_eq!(max_fps_for(&chain, 100), 20);
}
