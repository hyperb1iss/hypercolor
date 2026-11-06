//! Backend connect, write, brightness, and cadence against the loopback
//! fake responder (Spec 88 §9 and §11.5).

mod support;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use hypercolor_driver_api::{
    DeviceBackend, DeviceDeliveryAck, DeviceDeliveryId, DeviceDeliveryObserver,
    DeviceDeliveryStatus, DiscoveredDevice,
};
use hypercolor_driver_lifx::LifxBackend;
use hypercolor_driver_lifx::backend::FrameCommitment;
use hypercolor_driver_lifx::color::{Hsbk, HsbkEncoder};
use hypercolor_driver_lifx::config::LifxConfig;
use hypercolor_driver_lifx::scanner::{LifxScanOptions, LifxScanner, discovered_device};
use hypercolor_driver_lifx::transport::RequestPolicy;
use hypercolor_types::device::DeviceError;
use support::{
    FakeDevice, FakeKind, FakeLifx, FakeTile, Received, TestResult, assert_budget_respected,
};

const RED: [u8; 3] = [255, 0, 0];
const BLUE: [u8; 3] = [0, 0, 255];

fn policy() -> RequestPolicy {
    RequestPolicy {
        reply_timeout: Duration::from_millis(200),
        attempts: 2,
    }
}

async fn discover(fake: &FakeLifx, config: &LifxConfig) -> TestResult<Vec<DiscoveredDevice>> {
    let profiles = LifxScanner::new(LifxScanOptions {
        unicast_targets: vec![fake.addr()],
        broadcast_targets: Vec::new(),
        timeout: Duration::from_millis(600),
        request_policy: policy(),
        ..LifxScanOptions::default()
    })
    .scan()
    .await?
    .profiles;
    Ok(profiles
        .iter()
        .map(|profile| discovered_device(profile, config.max_messages_per_second))
        .collect())
}

fn hsbk_at(payload: &[u8], offset: usize) -> Hsbk {
    let mut raw = [0_u8; 8];
    raw.copy_from_slice(&payload[offset..offset + 8]);
    Hsbk::from_le_bytes(raw)
}

fn last(messages: &[Received]) -> &Received {
    messages.last().expect("at least one message")
}

#[tokio::test]
async fn connect_powers_on_and_streams_extended_zones() -> TestResult {
    let fake = FakeLifx::spawn(vec![FakeDevice::new(
        3,
        38,
        (3, 70),
        FakeKind::ExtendedStrip { zones: 82 },
    )])
    .await?;
    let config = LifxConfig {
        transition_ms: 30,
        ..LifxConfig::default()
    };
    let discovered = discover(&fake, &config).await?.remove(0);
    let device_id = discovered.info.id;
    let backend = LifxBackend::new(config).with_request_policy(policy());
    backend.adopt_device(&discovered)?;
    backend.connect(&device_id).await?;

    let power = fake.wait_for(117, 1).await;
    assert_eq!(power.len(), 1);
    assert!(
        power[0].ack_required,
        "power on asks for an acknowledgement"
    );
    assert_eq!(&power[0].payload, &[0xff, 0xff, 0, 0, 0, 0]);

    let info = backend
        .connected_device_info(&device_id)
        .await?
        .expect("connected info");
    assert_eq!(info.total_led_count(), 82);
    assert_eq!(info.capabilities.max_fps, 20);
    let metadata = backend
        .connected_device_metadata(&device_id)
        .await?
        .expect("connected metadata");
    assert_eq!(metadata["device_class"], "extended_multizone");

    let cadence = backend.output_cadence(&device_id).expect("cadence");
    assert_eq!(cadence.min_interval(), Some(Duration::from_millis(50)));
    assert_eq!(cadence.max_frame_silence(), Some(Duration::from_secs(1)));
    assert_eq!(backend.target_fps(&device_id), Some(20));

    backend.write_colors(&device_id, &[RED; 82]).await?;
    let frames = fake.wait_for(510, 1).await;
    let frame = last(&frames);
    assert!(!frame.ack_required && !frame.res_required && !frame.tagged);
    assert_eq!(&frame.payload[0..4], &30_u32.to_le_bytes());
    assert_eq!(frame.payload[4], 1);
    assert_eq!(frame.payload[7], 82);
    let full = HsbkEncoder::new(6_500, true).encode(RED);
    assert_eq!(hsbk_at(&frame.payload, 8), full);

    backend.set_brightness(&device_id, 128).await?;
    backend.write_colors(&device_id, &[RED; 82]).await?;
    let frames = fake.wait_for(510, 2).await;
    let dimmed = hsbk_at(&last(&frames).payload, 8);
    assert_eq!(dimmed.hue, full.hue);
    assert_eq!(dimmed.saturation, full.saturation);
    assert_eq!(dimmed.brightness, 32_896);

    backend.disconnect(&device_id).await?;
    assert!(matches!(
        backend.write_colors(&device_id, &[RED; 82]).await,
        Err(DeviceError::Disconnected { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn tile_chains_stream_one_set64_per_tile_at_the_budgeted_cadence() -> TestResult {
    let fake = FakeLifx::spawn(vec![FakeDevice::new(
        5,
        55,
        (3, 50),
        FakeKind::Matrix {
            tiles: vec![FakeTile::upright(8, 8); 2],
        },
    )])
    .await?;
    let config = LifxConfig::default();
    let discovered = discover(&fake, &config).await?.remove(0);
    let device_id = discovered.info.id;
    let backend = LifxBackend::new(config).with_request_policy(policy());
    backend.adopt_device(&discovered)?;
    backend.connect(&device_id).await?;

    let cadence = backend.output_cadence(&device_id).expect("cadence");
    assert_eq!(cadence.min_interval(), Some(Duration::from_millis(100)));
    assert_eq!(cadence.target_fps(), 10);

    // Three frames are six messages: the first five fit the burst and the
    // sixth waits one message interval.
    let started = Instant::now();
    for _ in 0..3 {
        backend.write_colors(&device_id, &[RED; 128]).await?;
    }
    assert!(
        started.elapsed() >= Duration::from_millis(45),
        "the budget holds the sixth message back"
    );

    let frames = fake.wait_for(715, 6).await;
    assert_eq!(frames.len(), 6);
    let tiles: Vec<u8> = frames.iter().map(|frame| frame.payload[0]).collect();
    assert_eq!(tiles, vec![0, 1, 0, 1, 0, 1]);
    assert_budget_respected(&frames, started, 5, Duration::from_millis(50));
    Ok(())
}

fn alternating(zones: usize) -> Vec<[u8; 3]> {
    (0..zones)
        .map(|zone| if zone % 2 == 0 { RED } else { BLUE })
        .collect()
}

async fn connect_legacy_strip(
    fake: &FakeLifx,
) -> TestResult<(LifxBackend, hypercolor_types::device::DeviceId)> {
    let config = LifxConfig::default();
    let discovered = discover(fake, &config).await?.remove(0);
    let device_id = discovered.info.id;
    let backend = LifxBackend::new(config).with_request_policy(policy());
    backend.adopt_device(&discovered)?;
    backend.connect(&device_id).await?;
    Ok((backend, device_id))
}

#[tokio::test]
async fn worst_case_legacy_frames_are_paced_message_by_message() -> TestResult {
    let fake = FakeLifx::spawn(vec![FakeDevice::new(
        2,
        31,
        (2, 60),
        FakeKind::LegacyStrip { zones: 16 },
    )])
    .await?;
    let (backend, device_id) = connect_legacy_strip(&fake).await?;

    let cadence = backend.output_cadence(&device_id).expect("cadence");
    assert_eq!(
        cadence.min_interval(),
        Some(Duration::from_millis(50)),
        "the host offers frames at the cheapest frame's cost"
    );
    assert_eq!(backend.target_fps(&device_id), Some(20));

    let started = Instant::now();
    backend.write_colors(&device_id, &alternating(16)).await?;
    let elapsed = started.elapsed();
    assert!(
        elapsed >= Duration::from_millis(540),
        "16 runs cost 11 paced intervals after the burst, took {elapsed:?}"
    );

    let runs = fake.wait_for(501, 16).await;
    assert_eq!(runs.len(), 16);
    assert!(runs[..15].iter().all(|run| run.payload[14] == 0));
    assert_eq!(runs[15].payload[14], 1, "only the last run applies");
    assert_budget_respected(&runs, started, 5, Duration::from_millis(50));
    Ok(())
}

fn delivery(sequence: u64) -> DeviceDeliveryId {
    DeviceDeliveryId {
        queue_generation: 1,
        sequence,
    }
}

fn is_solid_run(run: &Received, zones: u8) -> bool {
    run.payload[0] == 0 && run.payload[1] == zones - 1
}

#[tokio::test]
async fn a_cheaper_newer_frame_replaces_a_paced_tail() -> TestResult {
    let fake = FakeLifx::spawn(vec![FakeDevice::new(
        2,
        31,
        (2, 60),
        FakeKind::LegacyStrip { zones: 16 },
    )])
    .await?;
    let (backend, device_id) = connect_legacy_strip(&fake).await?;
    let sink = backend.frame_sink(&device_id).expect("frame sink");
    let observer = Arc::new(RecordingObserver::default());

    // Sixteen runs: five leave in the burst, the rest are paced 50 ms apart.
    let old = sink
        .deliver_colors_shared_observed(delivery(1), Arc::new(alternating(16)), observer.clone())
        .await;
    assert_eq!(
        old.status,
        DeviceDeliveryStatus::Completed,
        "handed to the sender"
    );
    tokio::time::sleep(Duration::from_millis(80)).await;
    let newer = sink
        .deliver_colors_shared_observed(delivery(2), Arc::new(vec![RED; 16]), observer.clone())
        .await;
    assert_eq!(newer.status, DeviceDeliveryStatus::Completed);

    let runs = loop {
        let runs = fake.wait_for(501, 1).await;
        if runs.iter().any(|run| is_solid_run(run, 16)) {
            break runs;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    let solid = runs
        .iter()
        .position(|run| is_solid_run(run, 16))
        .expect("solid frame sent");
    assert!(
        solid < 15,
        "the old frame's tail was abandoned after {solid} runs"
    );
    assert!(
        runs[..solid].iter().all(|run| run.payload[14] == 0),
        "the abandoned frame never applied"
    );
    assert_eq!(
        runs[solid].payload[14], 1,
        "the newer frame carries the apply"
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        fake.received_of_type(501).len(),
        solid + 1,
        "nothing of the old frame follows"
    );
    Ok(())
}

#[tokio::test]
async fn an_equally_costly_newer_frame_waits_so_frames_still_finish() -> TestResult {
    let fake = FakeLifx::spawn(vec![FakeDevice::new(
        2,
        31,
        (2, 60),
        FakeKind::LegacyStrip { zones: 16 },
    )])
    .await?;
    let (backend, device_id) = connect_legacy_strip(&fake).await?;
    let sink = backend.frame_sink(&device_id).expect("frame sink");
    let observer = Arc::new(RecordingObserver::default());

    let mut flipped = alternating(16);
    flipped.reverse();
    sink.deliver_colors_shared_observed(delivery(1), Arc::new(alternating(16)), observer.clone())
        .await;
    for sequence in 2..12 {
        tokio::time::sleep(Duration::from_millis(30)).await;
        let colors = if sequence % 2 == 0 {
            flipped.clone()
        } else {
            alternating(16)
        };
        sink.deliver_colors_shared_observed(delivery(sequence), Arc::new(colors), observer.clone())
            .await;
    }

    let runs = fake.wait_for(501, 32).await;
    assert!(runs.len() >= 32, "{} runs", runs.len());
    assert_eq!(runs[15].payload[14], 1, "the first frame finished");
    assert_eq!(
        runs[31].payload[14], 1,
        "the newest frame finished after it"
    );
    Ok(())
}

#[tokio::test]
async fn direct_writes_preempt_a_paced_tail_and_wait_for_their_frame() -> TestResult {
    let fake = FakeLifx::spawn(vec![FakeDevice::new(
        2,
        31,
        (2, 60),
        FakeKind::LegacyStrip { zones: 16 },
    )])
    .await?;
    let (backend, device_id) = connect_legacy_strip(&fake).await?;
    let sink = backend.frame_sink(&device_id).expect("frame sink");
    sink.deliver_colors_shared_observed(
        delivery(1),
        Arc::new(alternating(16)),
        Arc::new(RecordingObserver::default()),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(20)).await;

    // An identify flash: a solid frame written directly. write_colors only
    // returns once that frame (or a newer one) is on the wire.
    backend.write_colors(&device_id, &[BLUE; 16]).await?;
    let runs = loop {
        let runs = fake.wait_for(501, 1).await;
        if runs.iter().any(|run| is_solid_run(run, 16)) {
            break runs;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    };
    let last = runs.last().expect("runs sent");
    assert!(
        is_solid_run(last, 16),
        "the identify frame is the last thing sent"
    );
    assert_eq!(last.payload[14], 1);
    assert!(
        runs.len() < 16,
        "the identify frame did not wait for the old tail"
    );
    Ok(())
}

#[derive(Default)]
struct RecordingObserver {
    started: Mutex<Vec<DeviceDeliveryId>>,
}

impl DeviceDeliveryObserver for RecordingObserver {
    fn transport_started(&self, id: DeviceDeliveryId) {
        self.started.lock().expect("observer lock").push(id);
    }

    fn delivery_terminal(&self, _ack: &DeviceDeliveryAck) {}
}

#[tokio::test]
async fn frame_sink_reports_completed_deliveries() -> TestResult {
    let fake = FakeLifx::spawn(vec![FakeDevice::new(1, 27, (3, 70), FakeKind::Bulb)]).await?;
    let config = LifxConfig::default();
    let discovered = discover(&fake, &config).await?.remove(0);
    let device_id = discovered.info.id;
    let backend = LifxBackend::new(config).with_request_policy(policy());
    backend.adopt_device(&discovered)?;
    backend.connect(&device_id).await?;

    let sink = backend.frame_sink(&device_id).expect("frame sink");
    let observer = Arc::new(RecordingObserver::default());
    let id = DeviceDeliveryId {
        queue_generation: 1,
        sequence: 1,
    };
    let ack = sink
        .deliver_colors_shared_observed(id, Arc::new(vec![RED]), observer.clone())
        .await;
    assert_eq!(ack.status, DeviceDeliveryStatus::Completed);
    assert!(
        !ack.transport_started,
        "the handoff itself is not transport"
    );
    let frames = fake.wait_for(102, 1).await;
    assert_eq!(frames.len(), 1);
    assert_eq!(
        observer.started.lock().expect("observer lock").as_slice(),
        &[id],
        "the sender reports transport start when the frame goes out"
    );
    Ok(())
}

#[tokio::test]
async fn adoption_requires_routing_metadata() -> TestResult {
    let fake = FakeLifx::spawn(vec![FakeDevice::new(1, 27, (3, 70), FakeKind::Bulb)]).await?;
    let mut discovered = discover(&fake, &LifxConfig::default()).await?.remove(0);
    discovered.metadata = HashMap::from([("ip".to_owned(), "127.0.0.1".to_owned())]);
    let backend = LifxBackend::new(LifxConfig::default());
    assert!(matches!(
        backend.adopt_device(&discovered),
        Err(DeviceError::NotAdopted { .. })
    ));
    assert!(matches!(
        backend.connect(&discovered.info.id).await,
        Err(DeviceError::NotAdopted { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn connect_fails_when_the_light_goes_away() -> TestResult {
    let fake = FakeLifx::spawn(vec![FakeDevice::new(1, 27, (3, 70), FakeKind::Bulb)]).await?;
    let discovered = discover(&fake, &LifxConfig::default()).await?.remove(0);
    drop(fake);
    let backend = LifxBackend::new(LifxConfig::default()).with_request_policy(RequestPolicy {
        reply_timeout: Duration::from_millis(30),
        attempts: 1,
    });
    backend.adopt_device(&discovered)?;
    assert!(backend.connect(&discovered.info.id).await.is_err());
    assert!(backend.output_cadence(&discovered.info.id).is_none());
    Ok(())
}

fn legacy_strip_fake() -> Vec<FakeDevice> {
    vec![FakeDevice::new(
        2,
        31,
        (2, 60),
        FakeKind::LegacyStrip { zones: 16 },
    )]
}

// Gap: a handoff must not report transport start, or success while a send
// failure is unresolved.
#[tokio::test(flavor = "current_thread")]
#[cfg_attr(
    windows,
    ignore = "Windows does not surface ICMP refusal on a connected UDP send"
)]
async fn handoffs_never_report_success_over_an_unresolved_failure() -> TestResult {
    let fake = FakeLifx::spawn(legacy_strip_fake()).await?;
    let addr = fake.addr();
    let (backend, device_id) = connect_legacy_strip(&fake).await?;
    let sink = backend.frame_sink(&device_id).expect("frame sink");
    let observer = Arc::new(RecordingObserver::default());

    // Two frames handed over back to back: the first is replaced before it
    // reaches the wire, so only the second ever reports transport start.
    let first = sink
        .deliver_colors_shared_observed(delivery(1), Arc::new(vec![RED; 16]), observer.clone())
        .await;
    let second = sink
        .deliver_colors_shared_observed(delivery(2), Arc::new(vec![BLUE; 16]), observer.clone())
        .await;
    for ack in [&first, &second] {
        assert_eq!(ack.status, DeviceDeliveryStatus::Completed);
        assert!(!ack.transport_started, "a handoff is not transport");
    }
    fake.wait_for(501, 1).await;
    assert_eq!(
        observer.started.lock().expect("observer lock").as_slice(),
        &[delivery(2)]
    );

    // With the light gone, the connected socket starts refusing sends.
    drop(fake);
    let mut sequence = 3;
    let failed = loop {
        tokio::time::sleep(Duration::from_millis(60)).await;
        let ack = sink
            .deliver_colors_shared_observed(
                delivery(sequence),
                Arc::new(vec![RED; 16]),
                observer.clone(),
            )
            .await;
        sequence += 1;
        if ack.status == DeviceDeliveryStatus::Failed {
            break ack;
        }
        assert!(sequence < 40, "the refused send never surfaced");
    };
    assert!(failed.transient, "the session keeps running");

    // No send has happened since, so the failure must still be reported.
    let still = sink
        .deliver_colors_shared_observed(
            delivery(sequence),
            Arc::new(vec![RED; 16]),
            observer.clone(),
        )
        .await;
    assert_eq!(
        still.status,
        DeviceDeliveryStatus::Failed,
        "failures are sticky"
    );

    // The light comes back on the same port; a later successful send clears
    // the failure and handoffs succeed again.
    let fake = FakeLifx::spawn_on(addr, legacy_strip_fake()).await?;
    let mut recovered = false;
    for attempt in 0..40 {
        tokio::time::sleep(Duration::from_millis(60)).await;
        let ack = sink
            .deliver_colors_shared_observed(
                delivery(sequence + 1 + attempt),
                Arc::new(vec![BLUE; 16]),
                observer.clone(),
            )
            .await;
        if ack.status == DeviceDeliveryStatus::Completed {
            recovered = true;
            break;
        }
    }
    assert!(recovered, "a successful send clears the failure");
    assert!(!fake.wait_for(501, 1).await.is_empty());
    Ok(())
}

// Gap: cheap and expensive frames alternating between transmission slots
// must not keep every frame from reaching its final apply.
#[test]
fn commitment_survives_replacements_before_the_first_datagram() {
    let mut commitment = FrameCommitment::default();
    assert!(commitment.admits(80), "nothing started yet: newest wins");

    // An 80-run frame starts and sends ten runs.
    for remaining in (70..80).rev() {
        commitment.sent(remaining);
    }
    assert_eq!(commitment.remaining(), Some(70));

    // A solid frame beats the 80-run tail and becomes the commitment.
    assert!(commitment.admits(1));
    commitment.replaced(1);

    // Expensive and cheap frames keep arriving before its first datagram.
    for _ in 0..10 {
        assert!(
            !commitment.admits(80),
            "an expensive frame may not undo the cheap commitment before it starts"
        );
        assert!(
            !commitment.admits(1),
            "an equally cheap frame waits for the committed one to apply"
        );
    }
    assert_eq!(commitment.remaining(), Some(1), "the bound never grew");

    commitment.sent(0);
    commitment.finished();
    assert!(
        commitment.admits(80),
        "after an apply the next frame starts fresh"
    );
}

// On a current-thread runtime the sender only runs while the test sleeps,
// so each step below lands where the schedule says: a solid frame while a
// started frame waits for its next slot, then an expensive frame before the
// solid one's first datagram.
#[tokio::test(flavor = "current_thread")]
async fn alternating_cheap_and_expensive_frames_still_apply() -> TestResult {
    let fake = FakeLifx::spawn(legacy_strip_fake()).await?;
    let (backend, device_id) = connect_legacy_strip(&fake).await?;
    let sink = backend.frame_sink(&device_id).expect("frame sink");
    let observer = Arc::new(RecordingObserver::default());

    sink.deliver_colors_shared_observed(delivery(1), Arc::new(alternating(16)), observer.clone())
        .await;
    let mut sequence = 2;
    for _ in 0..8 {
        // Let the sender spend its burst or send at its slot, then park.
        tokio::time::sleep(Duration::from_millis(55)).await;
        sink.deliver_colors_shared_observed(
            delivery(sequence),
            Arc::new(vec![RED; 16]),
            observer.clone(),
        )
        .await;
        tokio::time::sleep(Duration::from_millis(2)).await;
        sink.deliver_colors_shared_observed(
            delivery(sequence + 1),
            Arc::new(alternating(16)),
            observer.clone(),
        )
        .await;
        tokio::time::sleep(Duration::from_millis(2)).await;
        sequence += 2;
    }
    tokio::time::sleep(Duration::from_millis(60)).await;
    let applied = fake
        .received_of_type(501)
        .iter()
        .filter(|run| run.payload[14] == 1)
        .count();
    assert!(applied >= 1, "no frame ever reached its final apply");
    Ok(())
}

// Gap: concurrent submissions must leave the slot's sequence monotonic so
// every waiter wakes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_direct_writes_all_settle() -> TestResult {
    let fake = FakeLifx::spawn(vec![FakeDevice::new(1, 27, (3, 70), FakeKind::Bulb)]).await?;
    let config = LifxConfig {
        max_messages_per_second: 200,
        ..LifxConfig::default()
    };
    let discovered = discover(&fake, &config).await?.remove(0);
    let device_id = discovered.info.id;
    let backend = Arc::new(LifxBackend::new(config).with_request_policy(policy()));
    backend.adopt_device(&discovered)?;
    backend.connect(&device_id).await?;

    let mut writers = tokio::task::JoinSet::new();
    for writer in 0..32_u8 {
        let backend = Arc::clone(&backend);
        writers.spawn(async move {
            for step in 0..8_u8 {
                backend
                    .write_colors(&device_id, &[[writer, step, 0]])
                    .await
                    .expect("write settles");
            }
        });
    }
    tokio::time::timeout(Duration::from_secs(20), async {
        while let Some(joined) = writers.join_next().await {
            joined.expect("writer task");
        }
    })
    .await
    .map_err(|_| "a direct write never settled")?;
    Ok(())
}

// Gap: once disconnect returns, nothing more leaves and retained sinks
// reject at once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn disconnect_stops_the_sender_before_returning() -> TestResult {
    let fake = FakeLifx::spawn(legacy_strip_fake()).await?;
    let (backend, device_id) = connect_legacy_strip(&fake).await?;
    let sink = backend.frame_sink(&device_id).expect("frame sink");
    let observer = Arc::new(RecordingObserver::default());

    // Keep a paced worst-case frame in flight, then disconnect mid-tail.
    sink.deliver_colors_shared_observed(delivery(1), Arc::new(alternating(16)), observer.clone())
        .await;
    tokio::time::sleep(Duration::from_millis(120)).await;
    backend.disconnect(&device_id).await?;

    assert!(sink.is_closed(), "retained sinks see a closed lane");
    let rejected = sink
        .deliver_colors_shared_observed(delivery(2), Arc::new(vec![RED; 16]), observer.clone())
        .await;
    assert_eq!(rejected.status, DeviceDeliveryStatus::Failed);
    assert!(matches!(
        rejected.error,
        Some(DeviceError::Disconnected { .. })
    ));

    // Whatever was sent before disconnect returned has landed well within
    // this pause; nothing may arrive after it.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let landed = fake.received_of_type(501).len();
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        fake.received_of_type(501).len(),
        landed,
        "a datagram left after disconnect"
    );
    assert!(landed < 16, "the tail was cut off, {landed} runs sent");
    Ok(())
}
