//! USB serial transport behaviour on a real tty.
//!
//! A pty pair stands in for a CDC-ACM port: the transport owns the slave
//! end, which is a tty with real termios state and kernel buffering, and
//! the test plays the device on the master end. Frames and commands come
//! from the shipping Nollie CDC and Dygma Focus encoders. No serial or USB
//! device is opened.

#![cfg(target_os = "linux")]

#[path = "support/serial_fake.rs"]
mod serial_fake;

use std::sync::Arc;
use std::time::{Duration, Instant};

use hypercolor_hal::drivers::dygma::{DygmaProtocol, DygmaVariant};
use hypercolor_hal::drivers::nollie::{NollieModel, NollieProtocol};
use hypercolor_hal::protocol::Protocol;
use hypercolor_hal::transport::serial::UsbSerialTransport;
use hypercolor_hal::transport::{Transport, TransportError};
use serial_fake::{Ticker, focus_echo, trapped_transport};
use tokio::io::AsyncReadExt;
use tokio_serial::SerialStream;

/// Slack on top of a deadline before a timing assertion counts as a hang.
const SLACK: Duration = Duration::from_millis(500);

/// Long enough that a test only sees it expire if close failed to wake it.
const NEVER: Duration = Duration::from_secs(30);

/// More than a Linux pty buffers, so a write this large stalls until the
/// device end reads.
const OVERSIZED_WRITE: usize = 1 << 20;

/// Nollie 8 CDC drives eight channels of 126 LEDs.
const NOLLIE_8_CDC_LEDS: usize = 8 * 126;

/// Open a pty pair as (device end, port end).
fn pty_pair() -> (SerialStream, SerialStream) {
    SerialStream::pair().expect("a pty pair should open")
}

fn nollie_8_cdc_frame() -> Vec<Vec<u8>> {
    let protocol = NollieProtocol::new(NollieModel::Nollie8Cdc);
    let colors = (0..NOLLIE_8_CDC_LEDS)
        .map(|index| {
            let byte = u8::try_from(index % 251).expect("index is reduced below 256");
            [byte, byte.wrapping_add(1), byte.wrapping_add(2)]
        })
        .collect::<Vec<_>>();
    let commands = protocol.encode_frame(&colors);

    // The transport has no drain barrier, which is only sound while these
    // packets are write-only and carry no pacing of their own.
    assert!(
        commands
            .iter()
            .all(|command| !command.expects_response && command.post_delay.is_zero()),
        "Nollie CDC frames are write-only with no inter-packet delay"
    );
    commands.into_iter().map(|command| command.data).collect()
}

/// Spawn a send that cannot finish because nobody reads the device end.
async fn spawn_stalled_send(
    transport: &Arc<UsbSerialTransport>,
) -> tokio::task::JoinHandle<Result<(), TransportError>> {
    let writer = tokio::spawn({
        let transport = Arc::clone(transport);
        async move { transport.send(&vec![0x5A; OVERSIZED_WRITE]).await }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !writer.is_finished(),
        "the send should be stalled on a full pty"
    );
    writer
}

#[tokio::test(flavor = "current_thread")]
async fn nollie_cdc_frame_reaches_the_tty_without_a_drain() {
    let (mut device, port) = pty_pair();
    let transport = trapped_transport("pty-nollie", port);
    let frame = nollie_8_cdc_frame();

    let started = Instant::now();
    for packet in &frame {
        transport
            .send(packet)
            .await
            .expect("each packet should queue on the tty");
    }
    assert!(
        started.elapsed() < SLACK,
        "a whole frame should queue without waiting on the device: {:?}",
        started.elapsed()
    );

    let expected = frame.concat();
    let mut received = vec![0_u8; expected.len()];
    tokio::time::timeout(SLACK, device.read_exact(&mut received))
        .await
        .expect("the device should see the frame promptly")
        .expect("the device should read the frame");
    assert_eq!(
        received, expected,
        "packets should reach the device in write order"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn dygma_focus_handshake_uses_each_reply_as_its_barrier() {
    let (device, port) = pty_pair();
    let transport = trapped_transport("pty-dygma", port);
    let protocol = DygmaProtocol::new(DygmaVariant::DefyWired);
    let commands = protocol.init_sequence();
    assert!(
        commands.iter().all(|command| command.expects_response),
        "every Focus command is answered, so its reply orders the next one"
    );
    let device = tokio::spawn(focus_echo(device, commands.len()));

    for command in &commands {
        let reply = transport
            .send_receive(&command.data, protocol.response_timeout())
            .await
            .expect("the device should answer each command");
        assert_eq!(reply, command.data[..command.data.len() - 1]);
    }

    tokio::time::timeout(SLACK, device)
        .await
        .expect("the device should finish")
        .expect("the device task should not panic");
}

#[tokio::test(flavor = "current_thread")]
async fn stalled_tty_does_not_hold_up_another_device_on_the_runtime() {
    let (_wedged_device, wedged_port) = pty_pair();
    let wedged = Arc::new(trapped_transport("pty-wedged", wedged_port).with_write_timeout(NEVER));
    let ticker = Ticker::start();
    let stalled = spawn_stalled_send(&wedged).await;

    let (healthy_device, healthy_port) = pty_pair();
    let healthy = trapped_transport("pty-healthy", healthy_port);
    let protocol = DygmaProtocol::new(DygmaVariant::DefyWired);
    let commands = protocol.init_sequence();
    let rounds = 10;
    let device = tokio::spawn(focus_echo(healthy_device, commands.len() * rounds));
    for _ in 0..rounds {
        for command in &commands {
            healthy
                .send_receive(&command.data, protocol.response_timeout())
                .await
                .expect("the healthy device should keep answering");
        }
    }
    tokio::time::timeout(SLACK, device)
        .await
        .expect("the healthy device should finish")
        .expect("the healthy device task should not panic");

    assert!(
        !stalled.is_finished(),
        "the wedged send should still be waiting"
    );
    assert!(
        ticker.ticks() > 0,
        "the wedged port parked the only runtime thread"
    );

    let started = Instant::now();
    wedged.close().await.expect("close should succeed");
    assert!(
        started.elapsed() < SLACK,
        "close waited behind the wedged write: {:?}",
        started.elapsed()
    );
    let result = tokio::time::timeout(SLACK, stalled)
        .await
        .expect("the wedged send should resolve once the transport closes")
        .expect("the wedged send task should not panic");
    assert!(matches!(result, Err(TransportError::Closed)), "{result:?}");
}

#[tokio::test(flavor = "current_thread")]
async fn write_deadline_fails_a_wedged_tty() {
    let write_timeout = Duration::from_millis(100);
    let (_device, port) = pty_pair();
    let transport = trapped_transport("pty-deadline", port).with_write_timeout(write_timeout);
    let ticker = Ticker::start();

    let started = Instant::now();
    let error = transport
        .send(&vec![0x5A; OVERSIZED_WRITE])
        .await
        .expect_err("a write the device never takes must miss its deadline");
    let elapsed = started.elapsed();

    assert!(matches!(error, TransportError::Timeout { .. }), "{error:?}");
    assert!(
        elapsed >= write_timeout,
        "deadline fired early: {elapsed:?}"
    );
    assert!(
        elapsed < write_timeout + SLACK,
        "deadline fired late: {elapsed:?}"
    );
    assert!(ticker.ticks() > 0);
    assert!(matches!(
        transport.send(b"next").await,
        Err(TransportError::Closed)
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn hangup_surfaces_as_a_write_error() {
    let (device, port) = pty_pair();
    let transport = trapped_transport("pty-hangup", port);
    transport
        .send(b"before unplug")
        .await
        .expect("send before hangup");

    // Closing the master hangs up the slave, as an unplug hangs up ttyACM.
    drop(device);

    let error = transport
        .send(b"after unplug")
        .await
        .expect_err("a write to a hung-up tty must fail");
    assert!(
        matches!(error, TransportError::IoError { .. }),
        "EIO from a hung-up tty should surface as an I/O error: {error:?}"
    );
    assert!(matches!(
        transport.send(b"again").await,
        Err(TransportError::Closed)
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn close_releases_the_tty_before_returning() {
    let (mut device, port) = pty_pair();
    let transport = trapped_transport("pty-close", port);

    transport.close().await.expect("close should succeed");

    // Once the last slave descriptor closes, reading the master fails. The
    // read only resolves if close really released the port.
    let mut byte = [0_u8; 1];
    let read = tokio::time::timeout(SLACK, device.read(&mut byte))
        .await
        .expect("the device should see the port close promptly");
    assert!(
        read.is_err(),
        "the master should report the closed slave: {read:?}"
    );
}
