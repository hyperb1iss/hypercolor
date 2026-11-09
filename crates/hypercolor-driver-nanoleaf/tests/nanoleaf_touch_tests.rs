use hypercolor_driver_nanoleaf::{NanoleafPanelLayout, NanoleafTouchDecoder};
use hypercolor_types::device_input::DeviceInputEdge;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

fn decoder() -> NanoleafTouchDecoder {
    NanoleafTouchDecoder::new(
        &[
            NanoleafPanelLayout {
                panel_id: 41,
                x: 0,
                y: 0,
                o: 0,
                shape_type: 7,
            },
            NanoleafPanelLayout {
                panel_id: 42,
                x: 100,
                y: 0,
                o: 0,
                shape_type: 8,
            },
            NanoleafPanelLayout {
                panel_id: 43,
                x: 200,
                y: 0,
                o: 0,
                shape_type: 17,
            },
        ],
        0.0,
    )
}

fn packet(panel: u16, kind: u8, strength: u8, from: u16) -> Vec<u8> {
    let [high, low] = panel.to_be_bytes();
    let [from_high, from_low] = from.to_be_bytes();
    vec![0, 1, high, low, kind << 4 | strength, from_high, from_low]
}

#[test]
fn down_hold_up_preserve_panel_contact_and_normalize_strength() -> TestResult {
    let mut decoder = decoder();
    assert!(decoder.decode(&packet(41, 0, 0, u16::MAX))?.is_empty());
    let began = decoder.decode(&packet(41, 1, 15, u16::MAX))?;
    assert!(
        matches!(&began[..], [DeviceInputEdge::TouchBegan { contact: 41, position, segment }]
        if (position.x - 0.5).abs() < 0.0001 && (position.y - 0.5).abs() < 0.0001 && (position.pressure - 1.0).abs() < f32::EPSILON && segment.as_deref() == Some("Panel 41"))
    );
    let held = decoder.decode(&packet(41, 2, 6, u16::MAX))?;
    assert!(
        matches!(&held[..], [DeviceInputEdge::TouchMoved { contact: 41, position }]
        if (position.pressure - 0.4).abs() < f32::EPSILON)
    );
    let ended = decoder.decode(&packet(41, 3, 0, u16::MAX))?;
    assert!(
        matches!(&ended[..], [DeviceInputEdge::TouchEnded { contact: 41, position }]
        if position.pressure.abs() < f32::EPSILON)
    );
    assert!(decoder.decode(&packet(41, 3, 0, u16::MAX))?.is_empty());
    Ok(())
}

#[test]
fn hold_recovers_missing_down_and_swipe_transfers_to_destination_panel() -> TestResult {
    let mut decoder = decoder();
    assert!(matches!(
        &decoder.decode(&packet(41, 2, 9, u16::MAX))?[..],
        [DeviceInputEdge::TouchBegan { contact: 41, .. }]
    ));
    let edges = decoder.decode(&packet(42, 4, 8, 41))?;
    assert!(
        matches!(&edges[..], [DeviceInputEdge::TouchEnded { contact: 41, .. }, DeviceInputEdge::TouchBegan { contact: 42, segment, .. }]
        if segment.as_deref() == Some("Panel 42"))
    );
    assert!(decoder.decode(&packet(41, 3, 0, u16::MAX))?.is_empty());
    Ok(())
}

#[test]
fn swipe_to_unknown_or_unsupported_panel_releases_the_source() -> TestResult {
    for destination in [43, 600] {
        let mut decoder = decoder();
        decoder.decode(&packet(41, 1, 15, u16::MAX))?;
        assert!(matches!(
            decoder.decode(&packet(destination, 4, 8, 41))?.as_slice(),
            [DeviceInputEdge::TouchEnded { contact: 41, .. }]
        ));
        assert!(decoder.decode(&packet(41, 3, 0, u16::MAX))?.is_empty());
    }
    Ok(())
}

#[test]
fn simultaneous_contacts_and_duplicate_down_do_not_restart_holds() -> TestResult {
    let mut decoder = decoder();
    let mut multiple = packet(41, 1, 15, u16::MAX);
    multiple[1] = 2;
    multiple.extend_from_slice(&packet(42, 1, 8, u16::MAX)[2..]);
    assert_eq!(decoder.decode(&multiple)?.len(), 2);
    assert!(matches!(
        &decoder.decode(&packet(41, 1, 15, u16::MAX))?[..],
        [DeviceInputEdge::TouchMoved { contact: 41, .. }]
    ));
    assert!(matches!(
        &decoder.decode(&packet(42, 3, 0, u16::MAX))?[..],
        [DeviceInputEdge::TouchEnded { contact: 42, .. }]
    ));
    Ok(())
}

#[test]
fn malformed_packet_is_atomic_and_unknown_panels_are_ignored() -> TestResult {
    let mut decoder = decoder();
    for malformed in [&[][..], &[0][..], &[0, 1][..], &[0, 0, 1][..]] {
        assert!(decoder.decode(malformed).is_err());
    }
    let mut multiple = packet(41, 1, 15, u16::MAX);
    multiple[1] = 2;
    multiple.extend_from_slice(&packet(42, 5, 8, u16::MAX)[2..]);
    assert!(decoder.decode(&multiple).is_err());
    assert!(matches!(
        &decoder.decode(&packet(41, 1, 15, u16::MAX))?[..],
        [DeviceInputEdge::TouchBegan { .. }]
    ));
    assert!(decoder.decode(&packet(43, 1, 15, u16::MAX))?.is_empty());
    assert!(decoder.decode(&packet(600, 1, 15, u16::MAX))?.is_empty());
    assert!(decoder.decode(&packet(41, 7, 15, u16::MAX)).is_err());
    Ok(())
}

#[test]
fn canvas_control_panels_and_mini_triangles_are_touch_surfaces() -> TestResult {
    for shape_type in [2, 3, 4, 7, 8, 9] {
        let mut decoder = NanoleafTouchDecoder::new(
            &[NanoleafPanelLayout {
                panel_id: 256,
                x: 0,
                y: 0,
                o: 0,
                shape_type,
            }],
            0.0,
        );
        assert!(matches!(
            &decoder.decode(&packet(256, 1, 15, u16::MAX))?[..],
            [DeviceInputEdge::TouchBegan { contact: 256, .. }]
        ));
    }
    Ok(())
}

#[test]
fn triangle_touch_uses_rotated_panel_centroid_for_begin_and_swipe_release() -> TestResult {
    let panels = [
        NanoleafPanelLayout {
            panel_id: 41,
            x: 0,
            y: 0,
            o: 0,
            shape_type: 8,
        },
        NanoleafPanelLayout {
            panel_id: 42,
            x: 100,
            y: 0,
            o: 0,
            shape_type: 7,
        },
    ];
    let mut decoder = NanoleafTouchDecoder::new(&panels, 90.0);
    let began = decoder.decode(&packet(41, 1, 15, u16::MAX))?;
    let [DeviceInputEdge::TouchBegan { position, .. }] = began.as_slice() else {
        panic!("touch begins");
    };
    assert!((position.x - 2.0 / 3.0).abs() < 0.0001);
    assert!((position.y - 0.5).abs() < 0.0001);
    let swipe = decoder.decode(&packet(42, 4, 15, 41))?;
    let [
        DeviceInputEdge::TouchEnded { position: from, .. },
        DeviceInputEdge::TouchBegan { position: to, .. },
    ] = swipe.as_slice()
    else {
        panic!("swipe transfers touch");
    };
    assert!((from.x - position.x).abs() < 0.0001);
    assert!((from.y - position.y).abs() < 0.0001);
    assert!((to.x - 0.5).abs() < 0.0001);
    assert!((to.y - 0.5).abs() < 0.0001);
    Ok(())
}

#[test]
fn reserved_bit_does_not_change_touch_type_or_strength() -> TestResult {
    let mut clear = decoder();
    let mut reserved = decoder();
    for kind in [0, 1, 2, 4, 3] {
        let clear_packet = packet(41, kind, 9, u16::MAX);
        let mut reserved_packet = clear_packet.clone();
        reserved_packet[4] |= 0x80;
        assert_eq!(
            clear.decode(&clear_packet)?,
            reserved.decode(&reserved_packet)?
        );
    }
    Ok(())
}
