#[path = "support/serial_fake.rs"]
mod serial_fake;

use std::sync::Arc;
use std::time::{Duration, Instant};

use hypercolor_hal::transport::Transport;
use hypercolor_hal::transport::TransportError;
use hypercolor_hal::transport::serial::{
    CLOSE_TIMEOUT, DEFAULT_WRITE_TIMEOUT, RELEASE_WAIT_TIMEOUT, UsbSerialTransport,
};
use serial_fake::{Ticker, TrapStream, trapped_transport, wait_released};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Slack on top of a deadline before a timing assertion counts as a hang.
const SLACK: Duration = Duration::from_millis(500);

/// Long enough that a test only sees it expire if close failed to wake it.
const NEVER: Duration = Duration::from_secs(30);

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).expect("test durations fit in u64")
}

/// Assert `error` is the disconnect a send returns once it misses its
/// deadline, which callers treat as fatal rather than transient.
fn assert_stalled_disconnect(error: &TransportError, write_timeout: Duration) {
    let expected = format!("stalled for {} ms", millis(write_timeout));
    assert!(
        matches!(error, TransportError::Disconnected { detail } if detail.contains(&expected)),
        "a missed write deadline should take the transport out of service: {error:?}"
    );
}

#[tokio::test]
async fn send_receive_reads_until_focus_terminator() {
    let (client, mut server) = tokio::io::duplex(256);
    let transport = trapped_transport("test-focus", client);

    let server_task = tokio::spawn(async move {
        let mut command = [0_u8; 16];
        let read = server
            .read(&mut command)
            .await
            .expect("server should read command");
        assert_eq!(&command[..read], b"help\n");

        server
            .write_all(b"hardware.firmware\r\nled.mode\r\n.\r\n")
            .await
            .expect("server should write response");
    });

    let response = transport
        .send_receive(b"help\n", Duration::from_millis(200))
        .await
        .expect("transport should return response");

    assert_eq!(
        String::from_utf8(response).expect("response should be utf-8"),
        "hardware.firmware\nled.mode"
    );
    server_task.await.expect("server task should finish");
}

#[tokio::test]
async fn receive_times_out_when_terminator_never_arrives() {
    let (client, _server) = tokio::io::duplex(64);
    let transport = trapped_transport("test-timeout", client);

    let error = transport
        .receive(Duration::from_millis(25))
        .await
        .expect_err("receive should time out");

    assert!(matches!(error, TransportError::Timeout { .. }));
}

#[tokio::test]
async fn receive_accepts_whitespace_wrapped_terminator_and_empty_ack() {
    let (client, mut server) = tokio::io::duplex(64);
    let transport = trapped_transport("test-empty-ack", client);

    tokio::spawn(async move {
        server
            .write_all(b" \t.\r\n")
            .await
            .expect("server should write response");
    });

    let response = transport
        .receive(Duration::from_millis(200))
        .await
        .expect("receive should succeed");

    assert!(response.is_empty());
}

#[tokio::test]
async fn receive_returns_closed_when_peer_disconnects_mid_response() {
    let (client, mut server) = tokio::io::duplex(64);
    let transport = trapped_transport("test-disconnect", client);

    tokio::spawn(async move {
        server
            .write_all(b"partial line")
            .await
            .expect("server should write partial response");
        drop(server);
    });

    let error = transport
        .receive(Duration::from_millis(200))
        .await
        .expect_err("disconnect should fail");

    assert!(matches!(error, TransportError::Closed));
}

#[tokio::test]
async fn sends_queue_frames_without_flushing_or_waiting_for_the_device() {
    let (client, mut device) = tokio::io::duplex(8 * 1024);
    let transport = trapped_transport("test-frame", client);
    let packets = (0_u8..49).map(|index| vec![index; 64]).collect::<Vec<_>>();

    for packet in &packets {
        transport
            .send(packet)
            .await
            .expect("send should complete while the device has not read yet");
    }

    let mut received = vec![0_u8; packets.len() * 64];
    device
        .read_exact(&mut received)
        .await
        .expect("device should read every packet");
    assert_eq!(received, packets.concat());
}

#[tokio::test]
async fn write_error_surfaces_and_fails_the_transport() {
    let (client, device) = tokio::io::duplex(64);
    let transport = trapped_transport("test-unplug", client);
    drop(device);

    let error = transport
        .send(b"frame")
        .await
        .expect_err("a write to a vanished device must fail");
    assert!(
        matches!(error, TransportError::NotFound { .. }),
        "broken pipe should classify as a missing device: {error:?}"
    );

    assert!(matches!(
        transport.send(b"frame").await,
        Err(TransportError::Closed)
    ));
    assert!(matches!(
        transport.receive(Duration::from_millis(25)).await,
        Err(TransportError::Closed)
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn write_deadline_fails_a_stalled_transport_without_parking_the_runtime() {
    let write_timeout = Duration::from_millis(50);
    let (client, _device) = tokio::io::duplex(64);
    let transport = trapped_transport("test-stall", client).with_write_timeout(write_timeout);
    let ticker = Ticker::start();

    let started = Instant::now();
    let error = transport
        .send(&[0x5A; 1024])
        .await
        .expect_err("a write the device never takes must miss its deadline");
    let elapsed = started.elapsed();

    assert_stalled_disconnect(&error, write_timeout);
    assert!(
        elapsed >= write_timeout,
        "deadline fired early: {elapsed:?}"
    );
    assert!(
        elapsed < write_timeout + SLACK,
        "deadline fired late: {elapsed:?}"
    );
    assert!(
        ticker.ticks() > 0,
        "the stalled write parked the only runtime thread"
    );
    assert!(matches!(
        transport.send(b"next").await,
        Err(TransportError::Closed)
    ));
}

#[tokio::test(start_paused = true)]
async fn stalled_send_reports_the_default_write_deadline() {
    let (client, _device) = tokio::io::duplex(64);
    let transport = trapped_transport("test-default-deadline", client);

    let error = transport
        .send(&[0x5A; 1024])
        .await
        .expect_err("a write the device never takes must miss its deadline");

    assert_stalled_disconnect(&error, DEFAULT_WRITE_TIMEOUT);
}

#[tokio::test(flavor = "current_thread")]
async fn close_interrupts_a_stalled_write() {
    let (client, _device) = tokio::io::duplex(64);
    let transport =
        Arc::new(trapped_transport("test-close-stalled-write", client).with_write_timeout(NEVER));
    let writer = tokio::spawn({
        let transport = Arc::clone(&transport);
        async move { transport.send(&[0x5A; 1024]).await }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        !writer.is_finished(),
        "write should be stalled on the full pipe"
    );

    let started = Instant::now();
    transport.close().await.expect("close should succeed");
    assert!(
        started.elapsed() < SLACK,
        "close waited behind the stalled write: {:?}",
        started.elapsed()
    );

    let result = tokio::time::timeout(SLACK, writer)
        .await
        .expect("the stalled write should resolve once the transport closes")
        .expect("writer task should not panic");
    assert!(matches!(result, Err(TransportError::Closed)), "{result:?}");
    assert!(matches!(
        transport.send(b"next").await,
        Err(TransportError::Closed)
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn close_interrupts_a_pending_receive() {
    let (client, _device) = tokio::io::duplex(64);
    let transport = Arc::new(trapped_transport("test-close-pending-read", client));
    let reader = tokio::spawn({
        let transport = Arc::clone(&transport);
        async move { transport.receive(NEVER).await }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!reader.is_finished(), "receive should wait for a reply");

    let started = Instant::now();
    transport.close().await.expect("close should succeed");
    assert!(started.elapsed() < SLACK);

    let result = tokio::time::timeout(SLACK, reader)
        .await
        .expect("the pending receive should resolve once the transport closes")
        .expect("reader task should not panic");
    assert!(matches!(result, Err(TransportError::Closed)), "{result:?}");
}

#[tokio::test(flavor = "current_thread")]
async fn close_releases_the_port_before_returning() {
    let (client, _device) = tokio::io::duplex(64);
    let (stream, probe) = TrapStream::new(client);
    let transport =
        UsbSerialTransport::from_stream("test-close-release", stream).expect("release service");
    transport.send(b"last frame").await.expect("send");

    transport.close().await.expect("close should succeed");

    assert!(
        probe.released(),
        "close should hand the port back so a reconnect can reopen it"
    );
    transport
        .close()
        .await
        .expect("a second close should be a no-op");
}

#[tokio::test(flavor = "current_thread")]
async fn close_is_bounded_when_the_port_release_blocks() {
    let (client, _device) = tokio::io::duplex(64);
    let (stream, probe, release) = TrapStream::holding_release(client);
    let transport =
        UsbSerialTransport::from_stream("test-close-drain", stream).expect("release service");
    let ticker = Ticker::start();

    let started = Instant::now();
    transport.close().await.expect("close should succeed");
    let elapsed = started.elapsed();

    assert!(
        elapsed + Duration::from_millis(10) >= CLOSE_TIMEOUT,
        "close should give the release its whole bound: {elapsed:?}"
    );
    assert!(
        elapsed < CLOSE_TIMEOUT + SLACK,
        "close outlived its bound: {elapsed:?}"
    );
    assert!(
        !probe.released(),
        "the held release should still be finishing in the background"
    );
    assert!(
        ticker.ticks() >= 10,
        "close parked the runtime thread while the port drained"
    );

    release
        .send(())
        .expect("release worker should still be waiting");
    wait_released(&probe, SLACK).await;
}

#[tokio::test(flavor = "current_thread")]
async fn dropping_an_open_transport_releases_the_port_off_the_runtime() {
    let (client, _device) = tokio::io::duplex(64);
    let (stream, probe, release) = TrapStream::holding_release(client);
    let transport = UsbSerialTransport::from_stream("test-drop", stream).expect("release service");
    let ticker = Ticker::start();

    let started = Instant::now();
    drop(transport);
    assert!(
        started.elapsed() < SLACK,
        "drop blocked on the port release"
    );

    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!probe.released());
    assert!(ticker.ticks() > 0);

    release
        .send(())
        .expect("release worker should still be waiting");
    wait_released(&probe, SLACK).await;
}

#[tokio::test(flavor = "current_thread")]
async fn reopening_a_port_waits_for_its_previous_release() {
    let path = "test-release-barrier";
    let (client, _device) = tokio::io::duplex(64);
    let (stream, probe, release) = TrapStream::holding_release(client);
    let transport = UsbSerialTransport::from_stream(path, stream).expect("release service");
    UsbSerialTransport::wait_for_release(path, Duration::ZERO)
        .await
        .expect("an open port has no release in flight");

    drop(transport);
    let error = UsbSerialTransport::wait_for_release(path, Duration::from_millis(50))
        .await
        .expect_err("the next open must not overlap a release still in flight");
    assert!(
        matches!(&error, TransportError::NotReady { detail } if detail.contains(path)),
        "the barrier should name the port it is waiting on: {error:?}"
    );
    UsbSerialTransport::wait_for_release("test-release-barrier-other", Duration::ZERO)
        .await
        .expect("the barrier only holds back the same path");

    let waiter = tokio::spawn(UsbSerialTransport::wait_for_release(
        path,
        RELEASE_WAIT_TIMEOUT,
    ));
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!waiter.is_finished(), "the reopen should still be waiting");
    release
        .send(())
        .expect("release worker should still be waiting");
    tokio::time::timeout(SLACK, waiter)
        .await
        .expect("the reopen should proceed once the release finishes")
        .expect("waiter task should not panic")
        .expect("the barrier should clear");
    assert!(probe.released());
}
