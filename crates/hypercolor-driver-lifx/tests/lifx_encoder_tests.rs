//! Frame to datagram plans per device class (Spec 88 §5 and §9).

use std::time::{Duration, Instant};

use hypercolor_driver_lifx::color::{Hsbk, HsbkEncoder};
use hypercolor_driver_lifx::encoder::{FrameEncoder, OutputPlan, StreamAddress};
use hypercolor_driver_lifx::pacer::{
    DEFAULT_BURST_MESSAGES, MessageBudget, frame_interval, per_message_interval,
};
use hypercolor_driver_lifx::products::MatrixQuirk;
use hypercolor_driver_lifx::protocol::{Packet, Serial, decode_packet};
use hypercolor_driver_lifx::topology::{LifxTopology, Orientation, TileGeometry};

const SERIAL: Serial = Serial([0xd0, 0x73, 0xd5, 0, 0, 7]);
const RED: [u8; 3] = [255, 0, 0];
const BLUE: [u8; 3] = [0, 0, 255];

fn converter() -> HsbkEncoder {
    HsbkEncoder::new(6_500, true)
}

fn address() -> StreamAddress {
    StreamAddress {
        source: 77,
        target: SERIAL,
        sequence: 250,
    }
}

fn encode(topology: &LifxTopology, frame: &[[u8; 3]], duration_ms: u32) -> Vec<Vec<u8>> {
    let mut encoder = FrameEncoder::new(OutputPlan::from_topology(topology));
    let mut address = address();
    let count = encoder
        .encode(frame, &converter(), duration_ms, &mut address)
        .expect("frame encodes");
    let datagrams: Vec<Vec<u8>> = encoder.datagrams().map(<[u8]>::to_vec).collect();
    assert_eq!(count, datagrams.len());
    datagrams
}

fn packet(bytes: &[u8]) -> Packet<'_> {
    let packet = decode_packet(bytes).expect("datagram decodes");
    assert_eq!(packet.header.source, 77);
    assert_eq!(packet.header.target, SERIAL);
    assert!(!packet.header.tagged);
    assert!(!packet.header.ack_required);
    assert!(!packet.header.res_required);
    packet
}

fn hsbk_at(payload: &[u8], offset: usize) -> Hsbk {
    let mut raw = [0_u8; 8];
    raw.copy_from_slice(&payload[offset..offset + 8]);
    Hsbk::from_le_bytes(raw)
}

fn tile(width: u8, height: u8, frame_buffers: u8) -> TileGeometry {
    TileGeometry {
        width,
        height,
        user_x: 0.0,
        user_y: 0.0,
        orientation: Orientation::RightSideUp,
        supported_frame_buffers: frame_buffers,
    }
}

fn matrix(tiles: Vec<TileGeometry>, quirk: MatrixQuirk) -> LifxTopology {
    LifxTopology::Matrix {
        tiles,
        quirk,
        uplight: None,
    }
}

#[test]
fn single_zone_sends_one_set_color() {
    let datagrams = encode(&LifxTopology::SingleZone, &[BLUE], 40);
    assert_eq!(datagrams.len(), 1);
    let packet = packet(&datagrams[0]);
    assert_eq!(packet.header.message_type, 102);
    assert_eq!(packet.header.sequence, 250);
    assert_eq!(hsbk_at(packet.payload, 1), converter().encode(BLUE));
    assert_eq!(&packet.payload[9..13], &40_u32.to_le_bytes());
}

#[test]
fn extended_strips_chunk_by_82_and_apply_last() {
    let frame: Vec<[u8; 3]> = (0..120)
        .map(|index| if index < 82 { RED } else { BLUE })
        .collect();
    let topology = LifxTopology::Multizone {
        zones: 120,
        extended: true,
    };
    let datagrams = encode(&topology, &frame, 0);
    assert_eq!(datagrams.len(), 2);

    let first = packet(&datagrams[0]);
    assert_eq!(first.header.message_type, 510);
    assert_eq!(first.header.sequence, 250);
    assert_eq!(first.payload[4], 0, "first chunk is NO_APPLY");
    assert_eq!(&first.payload[5..7], &0_u16.to_le_bytes());
    assert_eq!(first.payload[7], 82);
    assert_eq!(hsbk_at(first.payload, 8), converter().encode(RED));

    let second = packet(&datagrams[1]);
    assert_eq!(second.header.sequence, 251);
    assert_eq!(second.payload[4], 1, "last chunk is APPLY");
    assert_eq!(&second.payload[5..7], &82_u16.to_le_bytes());
    assert_eq!(second.payload[7], 38);
    assert_eq!(hsbk_at(second.payload, 8), converter().encode(BLUE));
    assert_eq!(hsbk_at(second.payload, 8 + 38 * 8), Hsbk::default());
}

#[test]
fn legacy_strips_merge_identical_runs() {
    let topology = LifxTopology::Multizone {
        zones: 16,
        extended: false,
    };
    let solid = encode(&topology, &[RED; 16], 0);
    assert_eq!(solid.len(), 1);
    let only = packet(&solid[0]);
    assert_eq!(only.header.message_type, 501);
    assert_eq!((only.payload[0], only.payload[1]), (0, 15));
    assert_eq!(only.payload[14], 1);

    let mut frame = [RED; 16];
    frame[4..8].copy_from_slice(&[BLUE; 4]);
    let runs = encode(&topology, &frame, 0);
    let ranges: Vec<(u8, u8, u8)> = runs
        .iter()
        .map(|bytes| {
            let packet = packet(bytes);
            (packet.payload[0], packet.payload[1], packet.payload[14])
        })
        .collect();
    assert_eq!(ranges, vec![(0, 3, 0), (4, 7, 0), (8, 15, 1)]);
}

#[test]
fn matrix_chains_send_one_set64_per_tile() {
    let topology = matrix(vec![tile(8, 8, 1), tile(8, 8, 1)], MatrixQuirk::None);
    let mut frame = vec![RED; 128];
    frame[64] = BLUE;
    let datagrams = encode(&topology, &frame, 25);
    assert_eq!(datagrams.len(), 2);
    for (index, bytes) in datagrams.iter().enumerate() {
        let packet = packet(bytes);
        assert_eq!(packet.header.message_type, 715);
        assert_eq!(
            &packet.payload[..6],
            &[u8::try_from(index).expect("tile index"), 1, 0, 0, 0, 8]
        );
        assert_eq!(&packet.payload[6..10], &25_u32.to_le_bytes());
    }
    assert_eq!(
        hsbk_at(packet(&datagrams[1]).payload, 10),
        converter().encode(BLUE)
    );
    assert_eq!(
        hsbk_at(packet(&datagrams[1]).payload, 18),
        converter().encode(RED)
    );
}

#[test]
fn large_matrices_use_a_hidden_frame_buffer() {
    let topology = matrix(vec![tile(16, 8, 2)], MatrixQuirk::None);
    let mut frame = vec![RED; 128];
    frame[64] = BLUE;
    let datagrams = encode(&topology, &frame, 120);
    assert_eq!(datagrams.len(), 3);

    let first = packet(&datagrams[0]);
    assert_eq!(first.header.message_type, 715);
    assert_eq!(&first.payload[..6], &[0, 1, 1, 0, 0, 16]);
    assert_eq!(&first.payload[6..10], &0_u32.to_le_bytes());
    let second = packet(&datagrams[1]);
    assert_eq!(&second.payload[..6], &[0, 1, 1, 0, 4, 16]);
    assert_eq!(hsbk_at(second.payload, 10), converter().encode(BLUE));

    let copy = packet(&datagrams[2]);
    assert_eq!(copy.header.message_type, 716);
    assert_eq!(&copy.payload[..10], &[0, 1, 1, 0, 0, 0, 0, 0, 16, 8]);
    assert_eq!(&copy.payload[10..14], &120_u32.to_le_bytes());
}

#[test]
fn large_matrices_without_spare_buffers_write_the_visible_buffer() {
    let topology = matrix(vec![tile(16, 8, 1)], MatrixQuirk::None);
    let datagrams = encode(&topology, &[RED; 128], 120);
    assert_eq!(datagrams.len(), 2);
    for bytes in &datagrams {
        let packet = packet(bytes);
        assert_eq!(packet.header.message_type, 715);
        assert_eq!(packet.payload[2], 0, "visible frame buffer");
        assert_eq!(&packet.payload[6..10], &120_u32.to_le_bytes());
    }
}

#[test]
fn candle_frames_fill_the_top_zone_and_wrap() {
    let topology = matrix(vec![tile(5, 6, 1)], MatrixQuirk::CandleTopZone);
    let mut frame = vec![RED; 26];
    frame[0] = BLUE;
    let datagrams = encode(&topology, &frame, 0);
    assert_eq!(datagrams.len(), 1);
    let packet = packet(&datagrams[0]);
    assert_eq!(packet.payload[5], 5, "candle width");
    let black = Hsbk::new(0, 0, 0, 6_500);
    assert_eq!(hsbk_at(packet.payload, 10), converter().encode(BLUE));
    for zone in 1..5 {
        assert_eq!(hsbk_at(packet.payload, 10 + zone * 8), black, "zone {zone}");
    }
    assert_eq!(hsbk_at(packet.payload, 10 + 5 * 8), converter().encode(RED));
    assert_eq!(
        hsbk_at(packet.payload, 10 + 29 * 8),
        converter().encode(RED)
    );
    assert_eq!(hsbk_at(packet.payload, 10 + 30 * 8), Hsbk::default());
}

#[test]
fn short_frames_pad_with_black_and_long_frames_truncate() {
    let topology = LifxTopology::Multizone {
        zones: 4,
        extended: true,
    };
    let short = encode(&topology, &[RED], 0);
    let payload = packet(&short[0]).payload.to_vec();
    assert_eq!(payload[7], 4);
    assert_eq!(hsbk_at(&payload, 16), Hsbk::new(0, 0, 0, 6_500));

    let long = encode(&topology, &[BLUE; 10], 0);
    assert_eq!(packet(&long[0]).payload[7], 4);
}

#[test]
fn sequence_numbers_wrap() {
    let topology = matrix(vec![tile(8, 8, 1); 5], MatrixQuirk::None);
    let datagrams = encode(&topology, &[RED; 320], 0);
    let sequences: Vec<u8> = datagrams
        .iter()
        .map(|bytes| packet(bytes).header.sequence)
        .collect();
    assert_eq!(sequences, vec![250, 251, 252, 253, 254]);
    let mut encoder = FrameEncoder::new(OutputPlan::from_topology(&topology));
    let mut address = StreamAddress {
        sequence: 254,
        ..address()
    };
    encoder
        .encode(&[RED; 320], &converter(), 0, &mut address)
        .expect("frame encodes");
    assert_eq!(address.sequence, 3);
}

#[test]
fn worst_case_messages_per_plan() {
    let plan = |topology: LifxTopology| OutputPlan::from_topology(&topology).worst_case_messages();
    assert_eq!(plan(LifxTopology::SingleZone), 1);
    assert_eq!(
        plan(LifxTopology::Multizone {
            zones: 82,
            extended: true,
        }),
        1
    );
    assert_eq!(
        plan(LifxTopology::Multizone {
            zones: 83,
            extended: true,
        }),
        2
    );
    assert_eq!(
        plan(LifxTopology::Multizone {
            zones: 16,
            extended: false,
        }),
        16
    );
    assert_eq!(plan(matrix(vec![tile(16, 8, 2)], MatrixQuirk::None)), 3);
    assert_eq!(plan(matrix(vec![tile(16, 8, 1)], MatrixQuirk::None)), 2);
    assert_eq!(
        plan(matrix(vec![tile(5, 6, 1)], MatrixQuirk::CandleTopZone)),
        1
    );
}

#[test]
fn best_case_cost_only_differs_for_legacy_strips() {
    let best = |topology: LifxTopology| OutputPlan::from_topology(&topology).best_case_messages();
    assert_eq!(
        best(LifxTopology::Multizone {
            zones: 80,
            extended: false,
        }),
        1
    );
    assert_eq!(
        best(LifxTopology::Multizone {
            zones: 120,
            extended: true,
        }),
        2
    );
    assert_eq!(best(matrix(vec![tile(8, 8, 1); 5], MatrixQuirk::None)), 5);
    assert_eq!(best(LifxTopology::SingleZone), 1);
}

#[test]
fn message_intervals_follow_the_budget() {
    assert_eq!(per_message_interval(20), Duration::from_millis(50));
    assert_eq!(frame_interval(20, 5), Duration::from_millis(250));
    assert_eq!(frame_interval(20, 0), Duration::from_millis(50));
}

#[test]
fn budget_allows_a_short_burst_then_paces_every_message() {
    let start = Instant::now();
    let mut budget = MessageBudget::new(20);
    for _ in 0..DEFAULT_BURST_MESSAGES {
        assert!(budget.ready_at(start));
        budget.record(start);
    }
    // The burst is spent: the next message waits one interval, the one
    // after that two, and so on.
    assert_eq!(budget.delay_at(start), Duration::from_millis(50));
    budget.record(start + Duration::from_millis(50));
    assert_eq!(budget.delay_at(start), Duration::from_millis(100));
    assert!(!budget.ready_at(start + Duration::from_millis(99)));
    assert!(budget.ready_at(start + Duration::from_millis(100)));
}

#[test]
fn budget_never_exceeds_burst_plus_rate_in_any_window() {
    let start = Instant::now();
    let mut budget = MessageBudget::new(20);
    let mut now = start;
    let mut sent = Vec::new();
    while sent.len() < 200 {
        now += budget.delay_at(now);
        budget.record(now);
        sent.push(now);
    }
    // In any window of length L at most burst + L / interval messages leave.
    let interval = budget.per_message();
    for (index, first) in sent.iter().enumerate() {
        for (offset, later) in sent[index..].iter().enumerate() {
            let window = later.duration_since(*first);
            let allowed = DEFAULT_BURST_MESSAGES as usize
                + usize::try_from(window.as_nanos() / interval.as_nanos()).expect("fits");
            assert!(offset < allowed, "{offset} messages after {window:?}");
        }
    }
    // Over the run the rate converges on the budget.
    let elapsed = sent[199].duration_since(start);
    assert!(elapsed >= Duration::from_millis(50 * 195), "{elapsed:?}");
}

#[test]
fn idle_budget_refills_to_a_full_burst_only() {
    let start = Instant::now();
    let mut budget = MessageBudget::new(20);
    budget.record(start);
    let later = start + Duration::from_secs(60);
    for _ in 0..DEFAULT_BURST_MESSAGES {
        assert!(budget.ready_at(later));
        budget.record(later);
    }
    assert!(!budget.ready_at(later));
    assert_eq!(MessageBudget::new(2).delay_at(start), Duration::ZERO);
    let mut tiny = MessageBudget::new(2);
    tiny.record(start);
    tiny.record(start);
    assert_eq!(
        tiny.delay_at(start),
        Duration::from_millis(500),
        "burst capped at the rate"
    );
}
