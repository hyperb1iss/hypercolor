#![cfg(feature = "ws-core")]

use bytes::Bytes;
use hypercolor_leptos_ext::ws::{ChannelDescriptor, Direction, SessionRecord, SessionRecorder};
use serde_json::json;

#[test]
fn session_recorder_captures_transport_metadata_and_external_records() {
    let mut recorder = SessionRecorder::new(vec![ChannelDescriptor::new(7, "control")]);

    recorder.record_transport_frame(7, Direction::ClientToServer, Bytes::from_static(b"request"));
    recorder.record_metadata(7, "method", json!("effects.apply"));
    recorder.record_external("fixture", Bytes::from_static(b"external"));

    assert_eq!(recorder.entries().len(), 3);

    let tape = recorder.finish();
    assert_eq!(tape.channels(), &[ChannelDescriptor::new(7, "control")]);
    assert!(
        tape.entries()
            .windows(2)
            .all(|window| { window[0].elapsed_ns <= window[1].elapsed_ns })
    );
    assert_eq!(
        tape.entries()[0].record,
        SessionRecord::TransportFrame {
            channel_id: 7,
            direction: Direction::ClientToServer,
            bytes: Bytes::from_static(b"request"),
        }
    );
    assert_eq!(
        tape.entries()[1].record,
        SessionRecord::Metadata {
            channel_id: 7,
            key: "method".to_owned(),
            value: json!("effects.apply"),
        }
    );
    assert_eq!(
        tape.entries()[2].record,
        SessionRecord::External {
            source: "fixture",
            body: Bytes::from_static(b"external"),
        }
    );
}

#[test]
fn session_tape_can_move_entries_out() {
    let mut recorder = SessionRecorder::new(Vec::new());
    recorder.record_transport_frame(1, Direction::ServerToClient, Bytes::from_static(b"hello"));

    let entries = recorder.finish().into_entries();

    assert_eq!(entries.len(), 1);
    assert!(matches!(
        entries[0].record,
        SessionRecord::TransportFrame {
            channel_id: 1,
            direction: Direction::ServerToClient,
            ..
        }
    ));
}
