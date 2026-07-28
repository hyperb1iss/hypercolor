use axum::body::Bytes;
use hypercolor_leptos_ext::ws::{
    PreviewFrame, PreviewFrameChannel, PreviewPixelFormat, PreviewStreamId,
    PreviewTransportCapability, ZonePreviewFrame,
};

use super::{
    PreviewCursorQueue, PreviewOutboundError, PreviewOutboundLimits, PreviewPublishOutcome,
    PreviewSendCursor, preview_outbound_channel_with_limits,
};

fn passive_frame(channel: PreviewFrameChannel, frame_number: u32, payload_len: usize) -> Bytes {
    PreviewFrame {
        channel,
        frame_number,
        timestamp_ms: frame_number,
        width: u32::try_from(payload_len / 4).expect("fixture width fits u32"),
        height: 1,
        format: PreviewPixelFormat::Rgba,
        payload: Bytes::from(vec![
            u8::try_from(frame_number).unwrap_or(u8::MAX);
            payload_len
        ]),
    }
    .try_encode()
    .expect("fixture frame encodes")
}

#[test]
fn router_counts_in_flight_publications_against_connection_budget() {
    let canvas = passive_frame(PreviewFrameChannel::Canvas, 1, 64);
    let screen = passive_frame(PreviewFrameChannel::ScreenCanvas, 2, 64);
    let max_publication_bytes = canvas.len().max(screen.len());
    let (sender, receiver) = preview_outbound_channel_with_limits(PreviewOutboundLimits {
        max_publication_bytes,
        max_connection_bytes: max_publication_bytes,
    });
    sender
        .publish(
            PreviewStreamId::Passive(PreviewFrameChannel::Canvas),
            canvas,
            None,
        )
        .expect("canvas queues");
    let in_flight = receiver.try_recv().expect("canvas enters flight");

    assert!(matches!(
        sender.publish(
            PreviewStreamId::Passive(PreviewFrameChannel::ScreenCanvas),
            screen.clone(),
            None,
        ),
        Err(PreviewOutboundError::ConnectionBudgetExceeded { .. })
    ));

    receiver.complete(&in_flight);
    assert_eq!(
        sender.publish(
            PreviewStreamId::Passive(PreviewFrameChannel::ScreenCanvas),
            screen,
            None,
        ),
        Ok(PreviewPublishOutcome::Queued)
    );
}

#[test]
fn completing_superseded_in_flight_frame_preserves_new_high_water() {
    let first = passive_frame(PreviewFrameChannel::Canvas, 1, 64);
    let second = passive_frame(PreviewFrameChannel::Canvas, 2, 64);
    let publication_bytes = first.len().max(second.len());
    let (sender, receiver) = preview_outbound_channel_with_limits(PreviewOutboundLimits {
        max_publication_bytes: publication_bytes,
        max_connection_bytes: publication_bytes * 2,
    });
    let stream = PreviewStreamId::Passive(PreviewFrameChannel::Canvas);
    sender
        .publish(stream.clone(), first, None)
        .expect("first frame queues");
    let old = receiver.try_recv().expect("first frame enters flight");
    sender
        .publish(stream, second, None)
        .expect("new frame queues while old is in flight");
    assert!(!receiver.is_current(&old));
    receiver.complete(&old);
    let new = receiver.try_recv().expect("new frame enters flight");
    assert!(receiver.is_current(&new));
    receiver.complete(&new);
}

#[test]
fn cursor_queue_rotates_chunked_streams_round_robin() {
    let canvas = passive_frame(PreviewFrameChannel::Canvas, 1, 512);
    let screen = passive_frame(PreviewFrameChannel::ScreenCanvas, 2, 512);
    let publication_bytes = canvas.len().max(screen.len());
    let (sender, receiver) = preview_outbound_channel_with_limits(PreviewOutboundLimits {
        max_publication_bytes: publication_bytes,
        max_connection_bytes: publication_bytes * 2,
    });
    sender
        .publish(
            PreviewStreamId::Passive(PreviewFrameChannel::Canvas),
            canvas,
            None,
        )
        .expect("canvas queues");
    sender
        .publish(
            PreviewStreamId::Passive(PreviewFrameChannel::ScreenCanvas),
            screen,
            None,
        )
        .expect("screen queues");

    let mut cursors = PreviewCursorQueue::new(2);
    for publication in [
        receiver.try_recv().expect("first publication"),
        receiver.try_recv().expect("second publication"),
    ] {
        cursors
            .try_insert(PreviewSendCursor::new(publication, 128).expect("cursor builds"))
            .expect("cursor queues");
    }

    let mut turns = Vec::new();
    for _ in 0..4 {
        let mut cursor = cursors.pop_next().expect("cursor turn available");
        turns.push(cursor.publication().stream().clone());
        assert!(cursor.next_message().expect("chunk encodes").is_some());
        cursors.requeue(cursor).expect("cursor requeues");
    }
    assert_eq!(
        turns,
        [
            PreviewStreamId::Passive(PreviewFrameChannel::Canvas),
            PreviewStreamId::Passive(PreviewFrameChannel::ScreenCanvas),
            PreviewStreamId::Passive(PreviewFrameChannel::Canvas),
            PreviewStreamId::Passive(PreviewFrameChannel::ScreenCanvas),
        ]
    );
}

#[test]
fn router_enforces_shared_stream_metadata_budget() {
    let capability = PreviewTransportCapability::default();
    let (sender, _receiver) = preview_outbound_channel_with_limits(PreviewOutboundLimits {
        max_publication_bytes: 1024,
        max_connection_bytes: 1024 * 1024,
    });
    for index in 0..capability.max_streams {
        let identity = u16::try_from(index)
            .expect("fixture index fits u16")
            .to_le_bytes();
        let mut scene_id = [0_u8; 16];
        scene_id[..2].copy_from_slice(&identity);
        let zone_id = [7_u8; 16];
        let encoded = ZonePreviewFrame {
            scene_id,
            zone_id,
            frame_number: 1,
            timestamp_ms: 1,
            width: 1,
            height: 1,
            format: PreviewPixelFormat::Rgba,
            payload: Bytes::from_static(&[1, 2, 3, 4]),
        }
        .try_encode()
        .expect("zone frame encodes");
        sender
            .publish(PreviewStreamId::Zone { scene_id, zone_id }, encoded, None)
            .expect("stream within metadata budget queues");
    }

    let scene_id = [0xFF_u8; 16];
    let zone_id = [0xEE_u8; 16];
    let encoded = ZonePreviewFrame {
        scene_id,
        zone_id,
        frame_number: 2,
        timestamp_ms: 2,
        width: 1,
        height: 1,
        format: PreviewPixelFormat::Rgba,
        payload: Bytes::from_static(&[1, 2, 3, 4]),
    }
    .try_encode()
    .expect("overflow frame encodes");
    assert!(matches!(
        sender.publish(PreviewStreamId::Zone { scene_id, zone_id }, encoded, None),
        Err(PreviewOutboundError::StreamBudgetExceeded { maximum })
            if maximum == capability.max_streams
    ));
}

#[test]
fn hello_capabilities_advertise_shared_preview_transport_limits() {
    let capabilities = super::super::protocol::ws_capabilities();
    assert_eq!(
        PreviewTransportCapability::from_capabilities(capabilities.iter().map(String::as_str)),
        Some(PreviewTransportCapability::default())
    );
}
