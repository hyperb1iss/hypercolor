//! The vendor's air gap between L-Wireless RF frames: a frame to another
//! receiver, or of another kind, waits 10 ms after an RGB frame and 5 ms
//! after any other, so a receiver never latches the trailing bytes of
//! another receiver's frame as its own address.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use hypercolor_hal::drivers::lianli::wireless::frame::{
    RfSubCommand, WallClock, clock_payload, clock_sync_envelope, pwm_envelope, rgb_transfer,
};
use hypercolor_hal::drivers::lianli::wireless::transport::{
    AIR_GAP_AFTER_RGB, AIR_GAP_BETWEEN_FRAMES, WirelessControllerTransport, air_gap,
};
use hypercolor_hal::protocol::TransferType;
use hypercolor_hal::transport::{Transport, TransportError};
use tokio::time::Instant;

const MASTER: [u8; 6] = [0xA0, 0x71, 0xAE, 0x72, 0xAB, 0x3C];
const FAN_A: [u8; 6] = [0x11; 6];
const FAN_B: [u8; 6] = [0x22; 6];
const RGB: u8 = RfSubCommand::SetRgb as u8;
const PWM: u8 = RfSubCommand::Pwm as u8;

#[test]
fn a_frame_to_another_target_or_of_another_kind_waits_out_the_gap() {
    let ms = Duration::from_millis;
    for (previous, since, target, sub, expected) in [
        (None, ms(0), FAN_A, RGB, ms(0)),
        (Some((FAN_A, RGB)), ms(0), FAN_A, RGB, ms(0)),
        (Some((FAN_A, PWM)), ms(0), FAN_A, PWM, ms(0)),
        (Some((FAN_A, RGB)), ms(0), FAN_B, RGB, AIR_GAP_AFTER_RGB),
        (Some((FAN_A, RGB)), ms(0), FAN_A, PWM, AIR_GAP_AFTER_RGB),
        (
            Some((FAN_A, PWM)),
            ms(0),
            FAN_B,
            PWM,
            AIR_GAP_BETWEEN_FRAMES,
        ),
        (
            Some((FAN_A, PWM)),
            ms(0),
            FAN_A,
            RGB,
            AIR_GAP_BETWEEN_FRAMES,
        ),
        (Some((FAN_A, RGB)), ms(4), FAN_B, PWM, ms(6)),
        (Some((FAN_A, PWM)), ms(3), FAN_B, PWM, ms(2)),
        (Some((FAN_A, RGB)), ms(25), FAN_B, PWM, ms(0)),
    ] {
        assert_eq!(
            air_gap(previous, since, target, sub),
            expected,
            "{previous:02x?} {since:?} ago, then {target:02x?}/{sub:02x}"
        );
    }
    assert_eq!(AIR_GAP_AFTER_RGB, ms(10));
    assert_eq!(AIR_GAP_BETWEEN_FRAMES, ms(5));
}

/// A pair that takes every write and logs when it did.
#[derive(Default)]
struct Recorder {
    writes: Mutex<Vec<(Instant, TransferType, Vec<u8>)>>,
}

struct Handle(Arc<Recorder>);

#[async_trait]
impl Transport for Handle {
    fn name(&self) -> &'static str {
        "recording pair"
    }

    async fn send(&self, data: &[u8]) -> Result<(), TransportError> {
        self.send_with_type(data, TransferType::Primary).await
    }

    async fn send_with_type(
        &self,
        data: &[u8],
        transfer_type: TransferType,
    ) -> Result<(), TransportError> {
        self.0.writes.lock().expect("write log").push((
            Instant::now(),
            transfer_type,
            data.to_vec(),
        ));
        Ok(())
    }

    async fn receive(&self, timeout: Duration) -> Result<Vec<u8>, TransportError> {
        self.receive_with_type(timeout, TransferType::Primary).await
    }

    async fn receive_with_type(
        &self,
        timeout: Duration,
        _transfer_type: TransferType,
    ) -> Result<Vec<u8>, TransportError> {
        tokio::time::sleep(timeout).await;
        Err(TransportError::Timeout {
            timeout_ms: u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
        })
    }

    async fn send_receive(
        &self,
        data: &[u8],
        timeout: Duration,
    ) -> Result<Vec<u8>, TransportError> {
        self.send(data).await?;
        self.receive(timeout).await
    }

    async fn close(&self) -> Result<(), TransportError> {
        Ok(())
    }
}

fn watched() -> (WirelessControllerTransport, Arc<Recorder>) {
    let recorder = Arc::new(Recorder::default());
    let transport =
        WirelessControllerTransport::new(Box::new(Handle(Arc::clone(&recorder))), "air-gap-test");
    (transport, recorder)
}

/// Every USB packet of an RGB transfer to `target`: the header twice, then
/// the data, as the protocol streams it.
fn rgb_packets(target: [u8; 6]) -> Vec<[u8; 64]> {
    let transfer = rgb_transfer(target, MASTER, [0x80, 0, 0, 1], 3, 1, 5000, &[7; 9]);
    let mut packets = Vec::new();
    for envelope in [&transfer.header, &transfer.header]
        .into_iter()
        .chain(&transfer.data)
    {
        packets.extend(envelope.usb_packets(8, 1));
    }
    packets
}

fn clock_packets() -> [[u8; 64]; 4] {
    clock_sync_envelope(MASTER, &clock_payload(WallClock::default())).usb_packets(8, 0xFF)
}

async fn send_all(transport: &WirelessControllerTransport, packets: &[[u8; 64]]) {
    for packet in packets {
        transport
            .send_with_type(packet, TransferType::Primary)
            .await
            .expect("the TX takes it");
    }
}

/// The gap between the last packet of the first `before` writes and the
/// next write.
fn gap_after(recorder: &Recorder, before: usize) -> Duration {
    let writes = recorder.writes.lock().expect("write log");
    writes[before].0 - writes[before - 1].0
}

#[tokio::test(start_paused = true)]
async fn the_clock_after_an_rgb_transfer_waits_ten_milliseconds() {
    let (transport, recorder) = watched();
    let rgb = rgb_packets(FAN_A);
    send_all(&transport, &rgb).await;
    send_all(&transport, &clock_packets()).await;

    let writes = recorder.writes.lock().expect("write log").clone();
    let start = writes[0].0;
    assert!(
        writes[..rgb.len()].iter().all(|(at, ..)| *at == start),
        "one transfer to one receiver goes out back to back"
    );
    drop(writes);
    assert_eq!(gap_after(&recorder, rgb.len()), AIR_GAP_AFTER_RGB);
    for slice in 1..4 {
        assert_eq!(
            gap_after(&recorder, rgb.len() + slice),
            Duration::ZERO,
            "the slices of one frame follow each other freely"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn rgb_to_another_receiver_waits_and_time_already_spent_counts() {
    let (transport, recorder) = watched();
    // The session's first clock takes the status drain the transport runs
    // once a second, so no drain read sits inside the gaps measured here.
    send_all(&transport, &clock_packets()).await;
    tokio::time::sleep(Duration::from_millis(25)).await;
    let first = rgb_packets(FAN_A);
    send_all(&transport, &first).await;
    tokio::time::sleep(Duration::from_millis(4)).await;
    let before = recorder.writes.lock().expect("write log").len();
    send_all(&transport, &rgb_packets(FAN_B)).await;
    let waited = gap_after(&recorder, before);
    assert_eq!(waited, AIR_GAP_AFTER_RGB, "4 ms elapsed, 6 ms waited");

    let fan_speed = pwm_envelope(FAN_A, MASTER, 1, 8, 1, [128; 4]).usb_packets(8, 1);
    let before = recorder.writes.lock().expect("write log").len();
    send_all(&transport, &fan_speed).await;
    assert_eq!(gap_after(&recorder, before), AIR_GAP_AFTER_RGB);

    let before = recorder.writes.lock().expect("write log").len();
    send_all(&transport, &rgb_packets(FAN_A)).await;
    assert_eq!(
        gap_after(&recorder, before),
        AIR_GAP_BETWEEN_FRAMES,
        "after a fan-speed frame the gap is five"
    );
}

#[tokio::test(start_paused = true)]
async fn control_packets_and_rx_polls_never_wait() {
    let (transport, recorder) = watched();
    send_all(&transport, &rgb_packets(FAN_A)).await;
    let before = recorder.writes.lock().expect("write log").len();
    let mut poll = [0_u8; 64];
    poll[..2].copy_from_slice(&[0x10, 0x02]);
    transport
        .send_with_type(&poll, TransferType::Companion)
        .await
        .expect("the RX takes it");
    let mut query = [0_u8; 64];
    query[..2].copy_from_slice(&[0x11, 0x08]);
    transport
        .send_with_type(&query, TransferType::Primary)
        .await
        .expect("the TX takes it");
    assert_eq!(gap_after(&recorder, before), Duration::ZERO);
    assert_eq!(gap_after(&recorder, before + 1), Duration::ZERO);
}
