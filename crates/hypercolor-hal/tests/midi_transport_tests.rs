use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use hypercolor_hal::transport::TransportError;
use hypercolor_hal::transport::midi::{
    MidiResponseIngress, MidiResponseToken, NativeMidiSession, OpenedMidiSession,
    close_input_before_output, open_native_midi_worker,
};

struct CallbackSession {
    ingress: MidiResponseIngress,
    closed: Arc<AtomicBool>,
}

impl NativeMidiSession for CallbackSession {
    fn send(&mut self, packet: &[u8]) -> Result<(), TransportError> {
        self.ingress.forward(packet);
        Ok(())
    }

    fn close(&mut self) {
        self.closed.store(true, Ordering::Release);
    }
}

struct DropFlag(Arc<AtomicBool>);

impl Drop for DropFlag {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

fn first_byte_matches(token: MidiResponseToken, message: &[u8]) -> bool {
    message
        .first()
        .is_some_and(|byte| u32::from(*byte) == token.get())
}

#[tokio::test]
async fn retained_worker_supports_delayed_receive_and_releases_native_state() {
    let closed = Arc::new(AtomicBool::new(false));
    let guard_dropped = Arc::new(AtomicBool::new(false));
    let session_closed = Arc::clone(&closed);
    let client = open_native_midi_worker(
        "hypercolor-test-midi".to_owned(),
        "test MIDI worker".to_owned(),
        DropFlag(Arc::clone(&guard_dropped)),
        2,
        first_byte_matches,
        move |ingress| {
            Ok(OpenedMidiSession::new(
                CallbackSession {
                    ingress,
                    closed: session_closed,
                },
                "test input".to_owned(),
                "test output".to_owned(),
            ))
        },
    )
    .await
    .expect("native MIDI worker should open");

    let token = MidiResponseToken::new(0x2A).expect("test token is nonzero");
    client
        .send(vec![0x2A, 0x01], Some(token))
        .await
        .expect("send should arm the response");
    let response = client
        .receive(Duration::from_secs(1), token)
        .await
        .expect("delayed receive should consume the armed response");

    assert_eq!(response, vec![0x2A, 0x01]);
    assert_eq!(client.input_name(), "test input");
    assert_eq!(client.output_name(), "test output");
    client.close().await.expect("worker should close cleanly");
    assert!(closed.load(Ordering::Acquire));

    tokio::time::timeout(Duration::from_secs(1), async {
        while !guard_dropped.load(Ordering::Acquire) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("worker should release its lifetime guard");
}

struct BacklogSession {
    backlog: Option<usize>,
}

impl NativeMidiSession for BacklogSession {
    fn send(&mut self, _packet: &[u8]) -> Result<(), TransportError> {
        Ok(())
    }

    fn close(&mut self) {}

    fn output_backlog(&mut self) -> Option<usize> {
        self.backlog
    }
}

async fn open_backlog_worker(
    backlog: Option<usize>,
) -> hypercolor_hal::transport::midi::MidiWorkerClient {
    open_native_midi_worker(
        "hypercolor-test-midi-backlog".to_owned(),
        "test MIDI backlog worker".to_owned(),
        (),
        2,
        first_byte_matches,
        move |_ingress| {
            Ok(OpenedMidiSession::new(
                BacklogSession { backlog },
                "test input".to_owned(),
                "test output".to_owned(),
            ))
        },
    )
    .await
    .expect("native MIDI worker should open")
}

#[tokio::test]
async fn worker_reports_the_native_output_backlog() {
    let stuck = open_backlog_worker(Some(6)).await;
    assert_eq!(
        stuck.output_backlog().await.expect("worker answers"),
        Some(6)
    );
    stuck.close().await.expect("worker should close cleanly");

    let opaque = open_backlog_worker(None).await;
    assert_eq!(opaque.output_backlog().await.expect("worker answers"), None);
    opaque.close().await.expect("worker should close cleanly");
}

#[tokio::test]
async fn sessions_without_backlog_visibility_report_none() {
    let client = open_native_midi_worker(
        "hypercolor-test-midi-default".to_owned(),
        "test MIDI default worker".to_owned(),
        (),
        2,
        first_byte_matches,
        move |ingress| {
            Ok(OpenedMidiSession::new(
                CallbackSession {
                    ingress,
                    closed: Arc::new(AtomicBool::new(false)),
                },
                "test input".to_owned(),
                "test output".to_owned(),
            ))
        },
    )
    .await
    .expect("native MIDI worker should open");

    assert_eq!(client.output_backlog().await.expect("worker answers"), None);
    client.close().await.expect("worker should close cleanly");
}

#[test]
fn native_midi_close_orders_input_before_output() {
    let order = RefCell::new(Vec::new());
    let mut input = Some("input");
    let mut output = Some("output");

    close_input_before_output(
        &mut input,
        &mut output,
        |name| order.borrow_mut().push(name),
        |name| order.borrow_mut().push(name),
    );

    assert_eq!(order.into_inner(), vec!["input", "output"]);
    assert!(input.is_none());
    assert!(output.is_none());
}
