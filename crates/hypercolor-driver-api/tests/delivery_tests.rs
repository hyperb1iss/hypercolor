//! One submission, one receipt.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use hypercolor_driver_api::{
    DeliveryResult, DeviceDeliveryId, Frame, FrameObserver, IoStage, MailboxFrame, PeerDisposition,
    Receipt, SkipReason,
};
use hypercolor_types::device::{DeviceError, DeviceId};

const ID: DeviceDeliveryId = DeviceDeliveryId {
    queue_generation: 4,
    sequence: 9,
};

#[derive(Default)]
struct Log {
    started: Mutex<Vec<DeviceDeliveryId>>,
    settled: Mutex<Vec<Receipt>>,
}

impl Log {
    fn started(&self) -> usize {
        self.started.lock().expect("log").len()
    }

    fn only_receipt(&self) -> Receipt {
        let settled = self.settled.lock().expect("log");
        assert_eq!(settled.len(), 1, "an attempt settles exactly once");
        settled[0].clone()
    }
}

impl FrameObserver for Log {
    fn transport_started(&self, id: DeviceDeliveryId) {
        self.started.lock().expect("log").push(id);
    }

    fn settled(&self, receipt: &Receipt) {
        self.settled.lock().expect("log").push(receipt.clone());
    }
}

fn frame(log: &Arc<Log>, leds: usize) -> Frame {
    Frame::new(
        DeviceId::new(),
        ID,
        Arc::clone(log) as Arc<dyn FrameObserver>,
        Instant::now(),
        Arc::new(vec![[1, 2, 3]; leds]),
    )
}

#[test]
fn a_written_frame_counts_three_bytes_per_led() {
    let log = Arc::new(Log::default());
    let (_, attempt) = frame(&log, 4).into_parts();

    let receipt = attempt.start().written();

    assert_eq!(receipt.id(), ID);
    assert_eq!(
        receipt.result(),
        &DeliveryResult::Written {
            bytes: 12,
            peer: None
        }
    );
    assert_eq!(log.started(), 1);
    assert_eq!(log.only_receipt(), receipt);
}

#[test]
fn a_partial_write_reports_what_it_covered() {
    let log = Arc::new(Log::default());
    let (_, attempt) = frame(&log, 4).into_parts();

    let receipt = attempt.start().written_part(3);

    assert_eq!(
        receipt.result(),
        &DeliveryResult::Written {
            bytes: 3,
            peer: None
        }
    );
}

#[test]
fn a_declined_frame_was_written_and_says_so() {
    let log = Arc::new(Log::default());
    let (_, attempt) = frame(&log, 1).into_parts();

    let receipt = attempt.start().answered(PeerDisposition::Declined);

    assert_eq!(
        receipt.result(),
        &DeliveryResult::Written {
            bytes: 3,
            peer: Some(PeerDisposition::Declined)
        }
    );
    assert_eq!(log.started(), 1);
    receipt.into_result().expect("a decline is not a failure");
}

#[test]
fn a_skipped_frame_never_starts_transport() {
    let log = Arc::new(Log::default());
    let (_, attempt) = frame(&log, 1).into_parts();

    let retry_at = Instant::now() + Duration::from_millis(5);
    let receipt = attempt.skip(SkipReason::Deferred { retry_at });

    assert_eq!(
        receipt.result(),
        &DeliveryResult::NotWritten(SkipReason::Deferred { retry_at })
    );
    assert_eq!(log.started(), 0);
    assert_eq!(receipt.transport_latency(), Duration::ZERO);
    receipt.into_result().expect("a skip is not a failure");
}

#[test]
fn a_frame_replaced_mid_write_is_superseded_not_written() {
    let log = Arc::new(Log::default());
    let (_, attempt) = frame(&log, 1).into_parts();

    let receipt = attempt.start().superseded();

    assert_eq!(
        receipt.result(),
        &DeliveryResult::NotWritten(SkipReason::Superseded)
    );
    assert_eq!(log.started(), 1, "part of it did go out");
}

#[test]
fn a_failure_says_where_it_stopped_and_keeps_its_type() {
    let error = DeviceError::Timeout {
        after: Duration::from_millis(25),
    };

    let log = Arc::new(Log::default());
    let (_, attempt) = frame(&log, 1).into_parts();
    let refused = attempt.reject(error.clone());
    assert_eq!(
        refused.result(),
        &DeliveryResult::Failed {
            stage: IoStage::Admission,
            error: error.clone(),
            transient: false,
        }
    );
    assert_eq!(log.started(), 0);

    let log = Arc::new(Log::default());
    let (_, attempt) = frame(&log, 1).into_parts();
    let failed = attempt.start().failed_transient(error.clone());
    assert_eq!(
        failed.result(),
        &DeliveryResult::Failed {
            stage: IoStage::Transport,
            error: error.clone(),
            transient: true,
        }
    );
    assert_eq!(failed.into_result(), Err(error));
}

#[test]
fn dropping_an_attempt_settles_it_as_cancelled() {
    let log = Arc::new(Log::default());
    drop(frame(&log, 1));

    assert!(matches!(
        log.only_receipt().result(),
        DeliveryResult::Failed {
            stage: IoStage::Admission,
            error: DeviceError::Disconnected { .. },
            transient: false,
        }
    ));
}

#[test]
fn dropping_a_transmission_settles_it_as_failed_in_transport() {
    let log = Arc::new(Log::default());
    let (_, attempt) = frame(&log, 1).into_parts();

    drop(attempt.start());

    assert!(matches!(
        log.only_receipt().result(),
        DeliveryResult::Failed {
            stage: IoStage::Transport,
            error: DeviceError::Disconnected { .. },
            transient: false,
        }
    ));
}

#[test]
fn a_receipt_splits_its_time_at_the_transport_start() {
    let log = Arc::new(Log::default());
    let (_, attempt) = frame(&log, 1).into_parts();

    std::thread::sleep(Duration::from_millis(2));
    let transmission = attempt.start();
    std::thread::sleep(Duration::from_millis(2));
    let receipt = transmission.written();

    assert!(receipt.queue_wait() >= Duration::from_millis(2));
    assert!(receipt.transport_latency() >= Duration::from_millis(2));
    assert!(receipt.total_latency() >= receipt.queue_wait() + receipt.transport_latency());
}

#[tokio::test]
async fn a_waiter_gets_the_receipt_the_observer_got() {
    let log = Arc::new(Log::default());
    let mut frame = frame(&log, 2);
    let waiting = frame.wait_for_receipt();
    let (_, attempt) = frame.into_parts();

    let receipt = attempt.start().written();

    assert_eq!(waiting.received().await, Some(receipt));
}

#[tokio::test]
async fn a_submitter_and_a_blocking_sink_can_both_wait() {
    let log = Arc::new(Log::default());
    let mut frame = frame(&log, 2);
    let submitter = frame.wait_for_receipt();
    let sink = frame.wait_for_receipt();
    let (_, attempt) = frame.into_parts();

    let receipt = attempt.start().written();

    assert_eq!(sink.received().await.as_ref(), Some(&receipt));
    assert_eq!(submitter.received().await, Some(receipt));
}

#[tokio::test]
async fn a_waiter_hears_about_a_dropped_attempt() {
    let log = Arc::new(Log::default());
    let mut frame = frame(&log, 2);
    let waiting = frame.wait_for_receipt();

    drop(frame);

    let receipt = waiting.received().await.expect("a dropped attempt settles");
    assert!(matches!(receipt.result(), DeliveryResult::Failed { .. }));
}

#[tokio::test]
async fn a_direct_frame_needs_no_observer() {
    let mut frame = Frame::direct(DeviceId::new(), Arc::new(vec![[1, 2, 3]]));
    let waiting = frame.wait_for_receipt();
    let (_, attempt) = frame.into_parts();

    let receipt = attempt.start().written();

    assert_eq!(waiting.received().await, Some(receipt));
}

#[test]
fn a_mailbox_gives_the_attempt_to_exactly_one_taker() {
    let log = Arc::new(Log::default());
    let posted = MailboxFrame::new(frame(&log, 1));
    assert_eq!(posted.colors(), &[[1, 2, 3]]);

    let attempt = posted.claim().expect("nobody claimed it yet");
    assert!(posted.claim().is_none());

    // A worker owns it now, so a newer frame changes nothing.
    posted.supersede();
    posted.reject(DeviceError::Disconnected {
        device: "fixture".to_owned(),
    });
    assert!(log.settled.lock().expect("log").is_empty());

    attempt.start().written();
    assert!(matches!(
        log.only_receipt().result(),
        DeliveryResult::Written { .. }
    ));
}

#[test]
fn a_replaced_mailbox_frame_settles_as_superseded() {
    let log = Arc::new(Log::default());
    let posted = MailboxFrame::new(frame(&log, 1));

    posted.supersede();

    assert_eq!(
        log.only_receipt().result(),
        &DeliveryResult::NotWritten(SkipReason::Superseded)
    );
    assert_eq!(log.started(), 0);
    assert!(posted.claim().is_none());
}

#[test]
fn a_mailbox_frame_nobody_took_settles_when_it_drops() {
    let log = Arc::new(Log::default());

    drop(MailboxFrame::new(frame(&log, 1)));

    assert!(matches!(
        log.only_receipt().result(),
        DeliveryResult::Failed {
            stage: IoStage::Admission,
            ..
        }
    ));
}
