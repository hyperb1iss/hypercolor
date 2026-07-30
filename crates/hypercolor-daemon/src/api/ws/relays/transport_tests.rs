use axum::body::Bytes;
use hypercolor_leptos_ext::ws::{
    PREVIEW_CHUNK_FIXED_HEADER_LEN, PreviewFrame, PreviewFrameChannel, PreviewPixelFormat,
    PreviewPublicationMetadata, PreviewStreamId, PreviewTransportCapability, ZonePreviewFrame,
};

use super::{
    PreviewCursorQueue, PreviewOutboundError, PreviewOutboundItem, PreviewOutboundLimits,
    PreviewPublication, PreviewPublishOutcome, PreviewSendCursor,
    preview_outbound_channel_with_limits,
};
use crate::interactive_preview::{PreviewCapacityLedger, PreviewResourceLedger};

fn next_publication(receiver: &super::PreviewOutboundReceiver) -> PreviewPublication {
    loop {
        match receiver.try_recv().expect("preview outbound item") {
            PreviewOutboundItem::Publication(publication) => return publication,
            PreviewOutboundItem::Cancellation(_) => {}
        }
    }
}

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

fn zone_frame(index: u64) -> (PreviewStreamId, Bytes) {
    let mut scene_id = [0_u8; 16];
    scene_id[..8].copy_from_slice(&index.to_le_bytes());
    let zone_id = [7_u8; 16];
    let encoded = ZonePreviewFrame {
        scene_id,
        zone_id,
        frame_number: 1,
        timestamp_ms: u32::try_from(index).expect("fixture index fits u32"),
        width: 1,
        height: 1,
        format: PreviewPixelFormat::Rgba,
        payload: Bytes::from_static(&[1, 2, 3, 4]),
    }
    .try_encode()
    .expect("zone fixture encodes");
    (PreviewStreamId::Zone { scene_id, zone_id }, encoded)
}

#[test]
fn queued_replacement_holds_each_resource_generation_until_transport_releases_it() {
    let capacity = PreviewCapacityLedger::new(30);
    let old_lane = capacity
        .try_reserve(PreviewResourceLedger {
            metadata_bytes: 10,
            ..PreviewResourceLedger::default()
        })
        .expect("old generation should fit");
    let old_frame = passive_frame(PreviewFrameChannel::Canvas, 1, 4);
    let max_publication_bytes = old_frame.len();
    let (sender, _receiver) = preview_outbound_channel_with_limits(PreviewOutboundLimits {
        max_publication_bytes,
        max_connection_bytes: max_publication_bytes,
    });
    let stream = PreviewStreamId::Passive(PreviewFrameChannel::Canvas);
    sender
        .publish_with_resource_guard(stream.clone(), old_frame, None, old_lane.clone())
        .expect("old generation should queue");
    drop(old_lane);
    assert_eq!(
        capacity
            .snapshot()
            .used
            .total_bytes()
            .expect("old generation ledger should remain representable"),
        10
    );

    let new_lane = capacity
        .try_reserve(PreviewResourceLedger {
            metadata_bytes: 20,
            ..PreviewResourceLedger::default()
        })
        .expect("resize should admit old and new generations transactionally");
    assert_eq!(
        capacity
            .snapshot()
            .used
            .total_bytes()
            .expect("overlap ledger should remain representable"),
        30
    );
    let new_frame = passive_frame(PreviewFrameChannel::Canvas, 2, 4);
    assert_eq!(
        sender
            .publish_with_resource_guard(stream.clone(), new_frame, None, new_lane.clone())
            .expect("new generation should replace the queued old generation"),
        PreviewPublishOutcome::Replaced
    );
    assert_eq!(
        capacity
            .snapshot()
            .used
            .total_bytes()
            .expect("new generation ledger should remain representable"),
        20
    );

    drop(new_lane);
    assert_eq!(
        capacity
            .snapshot()
            .used
            .total_bytes()
            .expect("queued new generation should retain its reservation"),
        20
    );
    assert!(sender.cancel(&stream).expect("queued generation cancels"));
    assert_eq!(
        capacity
            .snapshot()
            .used
            .total_bytes()
            .expect("cancelled generation ledger should remain representable"),
        0
    );
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
    let in_flight = next_publication(&receiver);

    assert!(matches!(
        sender.publish(
            PreviewStreamId::Passive(PreviewFrameChannel::ScreenCanvas),
            screen.clone(),
            None,
        ),
        Err(PreviewOutboundError::ConnectionBudgetExceeded { .. })
    ));

    receiver.complete(&in_flight);
    assert!(matches!(
        sender.publish(
            PreviewStreamId::Passive(PreviewFrameChannel::ScreenCanvas),
            screen,
            None,
        ),
        Ok(PreviewPublishOutcome::Queued)
    ));
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
    let old = next_publication(&receiver);
    sender
        .publish(stream, second, None)
        .expect("new frame queues while old is in flight");
    assert!(!receiver.is_current(&old));
    receiver.complete(&old);
    let cancellation = receiver.try_recv().expect("supersession cancellation");
    assert!(matches!(cancellation, PreviewOutboundItem::Cancellation(_)));
    let new = next_publication(&receiver);
    assert!(receiver.is_current(&new));
    receiver.complete(&new);
}

#[test]
fn cancellation_aborts_cursor_after_its_first_chunk() {
    let frame = passive_frame(PreviewFrameChannel::Canvas, 1, 512);
    let (sender, receiver) = preview_outbound_channel_with_limits(PreviewOutboundLimits {
        max_publication_bytes: frame.len(),
        max_connection_bytes: frame.len(),
    });
    let stream = PreviewStreamId::Passive(PreviewFrameChannel::Canvas);
    sender
        .publish(stream.clone(), frame, None)
        .expect("publication queues");
    let publication = next_publication(&receiver);
    let publication_id = publication.publication_id();
    let mut cursors = PreviewCursorQueue::new(1);
    let mut cursor = PreviewSendCursor::new(publication, 128).expect("cursor builds");
    assert!(
        cursor
            .next_message()
            .expect("first chunk encodes")
            .is_some()
    );
    assert!(!cursor.is_complete());
    cursors.try_insert(cursor).expect("cursor queues");

    assert!(matches!(sender.cancel(&stream), Ok(true)));
    let PreviewOutboundItem::Cancellation(cancellation) = receiver
        .try_recv()
        .expect("publication-specific cancellation")
    else {
        panic!("expected cancellation before more chunks");
    };
    assert_eq!(cancellation.stream, stream);
    assert_eq!(cancellation.publication_id, publication_id);
    let cancelled = cursors
        .remove_cancelled(&cancellation)
        .expect("partial cursor is removed");
    receiver.complete(cancelled.publication());
    assert!(cursors.is_empty());
}

#[test]
fn router_eviction_emits_cancellation_before_replacement_stream() {
    let canvas = passive_frame(PreviewFrameChannel::Canvas, 1, 64);
    let screen = passive_frame(PreviewFrameChannel::ScreenCanvas, 2, 64);
    let publication_bytes = canvas.len().max(screen.len());
    let (sender, receiver) = preview_outbound_channel_with_limits(PreviewOutboundLimits {
        max_publication_bytes: publication_bytes,
        max_connection_bytes: publication_bytes,
    });
    let canvas_stream = PreviewStreamId::Passive(PreviewFrameChannel::Canvas);
    sender
        .publish(canvas_stream.clone(), canvas, None)
        .expect("canvas queues");
    sender
        .publish(
            PreviewStreamId::Passive(PreviewFrameChannel::ScreenCanvas),
            screen,
            None,
        )
        .expect("screen evicts canvas");

    let PreviewOutboundItem::Cancellation(cancellation) =
        receiver.try_recv().expect("eviction cancellation")
    else {
        panic!("expected cancellation before remaining publication");
    };
    assert_eq!(cancellation.stream, canvas_stream);
    assert_eq!(
        next_publication(&receiver).stream(),
        &PreviewStreamId::Passive(PreviewFrameChannel::ScreenCanvas)
    );
}

#[test]
fn replacement_moves_stream_to_fifo_tail() {
    let canvas = passive_frame(PreviewFrameChannel::Canvas, 1, 64);
    let screen = passive_frame(PreviewFrameChannel::ScreenCanvas, 2, 64);
    let canvas_next = passive_frame(PreviewFrameChannel::Canvas, 3, 64);
    let publication_bytes = canvas.len().max(screen.len()).max(canvas_next.len());
    let (sender, receiver) = preview_outbound_channel_with_limits(PreviewOutboundLimits {
        max_publication_bytes: publication_bytes,
        max_connection_bytes: publication_bytes * 2,
    });
    let canvas_stream = PreviewStreamId::Passive(PreviewFrameChannel::Canvas);
    sender
        .publish(canvas_stream.clone(), canvas, None)
        .expect("canvas queues");
    sender
        .publish(
            PreviewStreamId::Passive(PreviewFrameChannel::ScreenCanvas),
            screen,
            None,
        )
        .expect("screen queues");
    sender
        .publish(canvas_stream, canvas_next, None)
        .expect("canvas replacement queues");

    assert!(matches!(
        receiver.try_recv(),
        Some(PreviewOutboundItem::Cancellation(_))
    ));
    assert_eq!(
        next_publication(&receiver).stream(),
        &PreviewStreamId::Passive(PreviewFrameChannel::ScreenCanvas)
    );
    assert_eq!(
        next_publication(&receiver).stream(),
        &PreviewStreamId::Passive(PreviewFrameChannel::Canvas)
    );
}

#[test]
fn replacement_churn_coalesces_cancellation_state() {
    let frame = passive_frame(PreviewFrameChannel::Canvas, 1, 64);
    let (sender, receiver) = preview_outbound_channel_with_limits(PreviewOutboundLimits {
        max_publication_bytes: frame.len(),
        max_connection_bytes: frame.len(),
    });
    let stream = PreviewStreamId::Passive(PreviewFrameChannel::Canvas);
    for frame_number in 1..=100 {
        sender
            .publish(
                stream.clone(),
                passive_frame(PreviewFrameChannel::Canvas, frame_number, 64),
                None,
            )
            .expect("latest publication replaces cleanly");
    }

    let PreviewOutboundItem::Cancellation(cancellation) =
        receiver.try_recv().expect("coalesced cancellation")
    else {
        panic!("expected cancellation before latest publication");
    };
    assert_eq!(cancellation.publication_id, 99);
    assert_eq!(next_publication(&receiver).publication_id(), 100);
    assert!(receiver.try_recv().is_none());
}

#[test]
fn cancellation_churn_is_bounded_without_discarding_live_state() {
    let capability = PreviewTransportCapability::default().legacy_v1();
    let (sender, receiver) = preview_outbound_channel_with_limits(PreviewOutboundLimits {
        max_publication_bytes: 1_024,
        max_connection_bytes: 1_024 * 1_024,
    });
    sender
        .negotiate_transport(capability)
        .expect("legacy transport negotiates before activation");

    for index in 0..capability.max_tombstones {
        let (stream, encoded) = zone_frame(u64::try_from(index).expect("fixture index fits u64"));
        sender
            .publish(stream.clone(), encoded, None)
            .expect("publication within cancellation budget queues");
        assert!(sender.cancel(&stream).expect("cancellation queues"));
    }

    let overflow_index =
        u64::try_from(capability.max_tombstones).expect("tombstone limit fits u64");
    let (overflow_stream, overflow_encoded) = zone_frame(overflow_index);
    sender
        .publish(overflow_stream.clone(), overflow_encoded, None)
        .expect("overflow publication queues before cancellation");
    assert!(matches!(
        sender.cancel(&overflow_stream),
        Err(PreviewOutboundError::CancellationBudgetExceeded { maximum })
            if maximum == capability.max_tombstones
    ));

    {
        let state = sender
            .shared
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(state.pending_cancellations.len(), capability.max_tombstones);
        assert_eq!(state.cancellation_order.len(), capability.max_tombstones);
        assert_eq!(
            state.current.get(&overflow_stream),
            Some(&(overflow_index + 1))
        );
        assert!(state.queued.contains_key(&overflow_stream));
    }

    let PreviewOutboundItem::Cancellation(oldest) = receiver
        .try_recv()
        .expect("oldest cancellation remains queued")
    else {
        panic!("expected oldest cancellation before overflow publication");
    };
    assert_eq!(oldest.stream, zone_frame(0).0);
    assert_eq!(oldest.publication_id, 1);
    assert!(
        sender
            .cancel(&overflow_stream)
            .expect("released capacity admits newest cancellation")
    );

    let state = sender
        .shared
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    assert_eq!(state.pending_cancellations.len(), capability.max_tombstones);
    assert_eq!(state.cancellation_order.len(), capability.max_tombstones);
    assert_eq!(
        state.pending_cancellations.get(&overflow_stream),
        Some(&(overflow_index + 1))
    );
    assert!(!state.current.contains_key(&overflow_stream));
    assert!(!state.queued.contains_key(&overflow_stream));
}

#[test]
fn v2_sender_uses_bytes_instead_of_legacy_stream_counts() {
    let capability = PreviewTransportCapability {
        max_streams: 1,
        ..PreviewTransportCapability::default()
    };
    let (sender, _receiver) = preview_outbound_channel_with_limits(PreviewOutboundLimits {
        max_publication_bytes: 1_024,
        max_connection_bytes: 1_024 * 1_024,
    });
    sender
        .negotiate_transport(capability)
        .expect("v2 transport negotiates before activation");

    for index in 0..257_u64 {
        let (stream, encoded) = zone_frame(index);
        sender
            .publish(stream, encoded, None)
            .expect("byte budget admits streams beyond the legacy count");
    }

    let state = sender
        .shared
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    assert_eq!(state.current.len(), 257);
    assert!(state.sender_state_bytes() <= capability.max_sender_state_bytes);
}

#[test]
fn v2_cancellation_delivery_releases_sender_state_bytes() {
    let (sender, receiver) = preview_outbound_channel_with_limits(PreviewOutboundLimits {
        max_publication_bytes: 1_024,
        max_connection_bytes: 1_024,
    });
    sender
        .negotiate_transport(PreviewTransportCapability::default())
        .expect("v2 transport negotiates before activation");
    let (stream, encoded) = zone_frame(1);
    sender
        .publish(stream.clone(), encoded, None)
        .expect("publication queues");
    assert!(sender.cancel(&stream).expect("publication cancels"));
    {
        let state = sender
            .shared
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(state.sender_state_bytes() > 0);
    }

    assert!(matches!(
        receiver.try_recv(),
        Some(PreviewOutboundItem::Cancellation(_))
    ));
    let state = sender
        .shared
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    assert_eq!(state.sender_state_bytes(), 0);
}

#[test]
fn router_allocation_failure_is_reported_without_panicking() {
    let frame = passive_frame(PreviewFrameChannel::Canvas, 1, 64);
    let (sender, _receiver) = preview_outbound_channel_with_limits(PreviewOutboundLimits {
        max_publication_bytes: frame.len(),
        max_connection_bytes: frame.len(),
    });
    let mut state = sender
        .shared
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    state.capability = PreviewTransportCapability::default().legacy_v1();
    state.capability.max_tombstones = usize::MAX;

    assert!(matches!(
        state.try_reserve_cancellations(usize::MAX, usize::MAX),
        Err(PreviewOutboundError::RouterAllocationFailed { .. })
    ));
    assert!(state.pending_cancellations.is_empty());
}

#[test]
fn cursor_rejects_more_than_advertised_chunk_count() {
    let capability = PreviewTransportCapability::default();
    let publication = PreviewPublication {
        metadata: PreviewPublicationMetadata {
            stream: PreviewStreamId::Passive(PreviewFrameChannel::Canvas),
            publication_id: 1,
            frame_number: 1,
            timestamp_ms: 1,
            width: 1,
            height: 1,
            format: PreviewPixelFormat::Rgba,
        },
        encoded: Bytes::from(vec![
            0;
            usize::try_from(capability.max_chunk_count)
                .expect("chunk count fits usize")
                + 1
        ]),
        interactive_fence: None,
        _resource_guard: None,
    };

    assert!(matches!(
        PreviewSendCursor::new(publication, PREVIEW_CHUNK_FIXED_HEADER_LEN + 1),
        Err(PreviewOutboundError::ChunkEncoding(_))
    ));
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
    for publication in [next_publication(&receiver), next_publication(&receiver)] {
        cursors
            .try_insert(PreviewSendCursor::new(publication, 128).expect("cursor builds"))
            .expect("cursor queues");
    }

    let mut turns = Vec::new();
    for _ in 0..4 {
        let mut cursor = cursors.pop_next().expect("cursor turn available");
        turns.push(cursor.publication().stream().clone());
        assert!(cursor.next_message().expect("chunk encodes").is_some());
        cursors.requeue(cursor);
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
fn v2_cursor_queue_enforces_exact_byte_budget_and_releases_on_remove() {
    let capability = PreviewTransportCapability::default();
    let (sender, receiver) = preview_outbound_channel_with_limits(PreviewOutboundLimits {
        max_publication_bytes: 1_024,
        max_connection_bytes: 2_048,
    });
    for index in 1..=2 {
        let (stream, encoded) = zone_frame(index);
        sender
            .publish(stream, encoded, None)
            .expect("cursor fixture queues");
    }
    let first = PreviewSendCursor::with_capability(next_publication(&receiver), capability)
        .expect("first cursor builds");
    let second = PreviewSendCursor::with_capability(next_publication(&receiver), capability)
        .expect("second cursor builds");

    let mut probe = PreviewCursorQueue::with_capability(capability);
    probe.try_insert(first).expect("probe cursor queues");
    let exact_state_bytes = probe.state_bytes;
    let first_stream = probe.head.clone().expect("probe cursor has a head");
    assert!(probe.remove(&first_stream).is_some());
    assert_eq!(probe.state_bytes, 0);

    let exact_capability = PreviewTransportCapability {
        max_streams: 0,
        max_cursor_state_bytes: exact_state_bytes,
        ..capability
    };
    let mut exact = PreviewCursorQueue::with_capability(exact_capability);
    exact
        .try_insert(second)
        .expect("exact byte budget admits cursor");
    assert_eq!(exact.state_bytes, exact_state_bytes);

    let (third_stream, third_encoded) = zone_frame(3);
    sender
        .publish(third_stream, third_encoded, None)
        .expect("third cursor fixture queues");
    let third = PreviewSendCursor::with_capability(next_publication(&receiver), exact_capability)
        .expect("third cursor builds");
    assert!(matches!(
        exact.try_insert(third),
        Err(PreviewOutboundError::CursorStateBudgetExceeded {
            maximum,
            actual,
        }) if maximum == exact_state_bytes && actual > maximum
    ));
}

#[test]
fn router_enforces_shared_stream_metadata_budget() {
    let capability = PreviewTransportCapability::default().legacy_v1();
    let (sender, _receiver) = preview_outbound_channel_with_limits(PreviewOutboundLimits {
        max_publication_bytes: 1024,
        max_connection_bytes: 1024 * 1024,
    });
    sender
        .negotiate_transport(capability)
        .expect("legacy transport negotiates before activation");
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
    let v2 = PreviewTransportCapability::default();
    let v1 = v2.legacy_v1();
    assert!(capabilities.contains(&v2.encode()));
    assert!(capabilities.contains(&v1.encode()));
    assert_eq!(
        PreviewTransportCapability::from_capabilities(capabilities.iter().map(String::as_str)),
        Some(v2)
    );

    let (sender, _receiver) = super::preview_outbound_channel();
    let state = sender
        .shared
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    assert_eq!(state.capability, v1);
}
