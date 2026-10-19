use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use hypercolor_hal::transport::smbus::{
    SmBusBusArbiter, SmBusClaim, SmBusOperation, SmBusTransportScope, decode_operations,
    encode_operations,
};

#[test]
fn smbus_operation_codec_round_trips() {
    let operations = vec![
        SmBusOperation::WriteWordData {
            register: 0x00,
            value: 0xA080,
        },
        SmBusOperation::Delay {
            duration: Duration::from_millis(1),
        },
        SmBusOperation::WriteByteData {
            register: 0x01,
            value: 0x01,
        },
        SmBusOperation::ReadByteData { register: 0x81 },
        SmBusOperation::WriteBlockData {
            register: 0x03,
            data: vec![1, 2, 3],
        },
    ];

    let encoded = encode_operations(&operations).expect("operations should encode");
    let decoded = decode_operations(&encoded).expect("operations should decode");
    assert_eq!(decoded, operations);
}

#[test]
fn smbus_decode_rejects_unknown_opcode() {
    let error = decode_operations(&[0xFF]).expect_err("unknown opcode should fail");
    let rendered = error.to_string();
    assert!(rendered.contains("unknown SMBus opcode"));
}

#[test]
fn smbus_decode_rejects_truncated_block_payload() {
    let error =
        decode_operations(&[0x04, 0x03, 0x03, 0xAA]).expect_err("truncated block should fail");
    let rendered = error.to_string();
    assert!(rendered.contains("truncated"));
}

#[test]
fn smbus_encode_rejects_delays_outside_u16_millis() {
    let operations = vec![SmBusOperation::Delay {
        duration: Duration::from_secs(70),
    }];

    let error = encode_operations(&operations).expect_err("long delay should fail");
    let rendered = error.to_string();
    assert!(rendered.contains("delay"));
}

#[tokio::test]
async fn process_bus_arbiters_serialize_independent_callers_on_the_same_bus() {
    let first = SmBusBusArbiter::for_bus("test:shared-smbus-arbiter");
    let second = SmBusBusArbiter::for_bus("test:shared-smbus-arbiter");
    let first_transaction = first.acquire_transaction().await;

    let mut waiter = tokio::spawn(async move {
        let _transaction = second.acquire_transaction().await;
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut waiter)
            .await
            .is_err(),
        "callers for one physical bus must share transaction ownership"
    );

    drop(first_transaction);
    tokio::time::timeout(Duration::from_secs(1), waiter)
        .await
        .expect("waiting caller should proceed after the bus is released")
        .expect("waiting caller should complete");
}

fn frame_batch() -> Vec<SmBusOperation> {
    vec![
        SmBusOperation::WriteWordData {
            register: 0x00,
            value: 0x0081,
        },
        SmBusOperation::Delay {
            duration: Duration::from_millis(1),
        },
        SmBusOperation::WriteBlockData {
            register: 0x03,
            data: vec![1, 2, 3],
        },
    ]
}

#[test]
fn device_scoped_write_batches_claim_only_their_address() {
    assert_eq!(
        SmBusClaim::for_batch(SmBusTransportScope::Device, 0x71, &frame_batch()),
        SmBusClaim::Device { address: 0x71 }
    );
}

#[test]
fn reads_and_bus_scoped_batches_claim_the_whole_bus() {
    let mut read_batch = frame_batch();
    read_batch.push(SmBusOperation::ReadByteData { register: 0x81 });

    assert_eq!(
        SmBusClaim::for_batch(SmBusTransportScope::Device, 0x71, &read_batch),
        SmBusClaim::Bus
    );
    assert_eq!(
        SmBusClaim::for_batch(SmBusTransportScope::Bus, 0x77, &frame_batch()),
        SmBusClaim::Bus
    );
    assert_eq!(SmBusTransportScope::default(), SmBusTransportScope::Bus);
}

/// Counts claims running at once; each claim waits, bounded, for a partner.
#[derive(Default)]
struct OverlapGauge {
    active: AtomicUsize,
    max_active: AtomicUsize,
}

impl OverlapGauge {
    fn enter(&self) {
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_active.fetch_max(active, Ordering::SeqCst);
    }

    fn exit(&self) {
        self.active.fetch_sub(1, Ordering::SeqCst);
    }

    /// Wait until `count` claims are inside at once, giving up after a
    /// bound so a serialized arbiter fails the test instead of hanging it.
    fn wait_for(&self, count: usize, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        while self.active.load(Ordering::SeqCst) < count && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

async fn run_gauged_claim(
    arbiter: &SmBusBusArbiter,
    claim: SmBusClaim,
    gauge: &Arc<OverlapGauge>,
    partners: usize,
) {
    let gauge = Arc::clone(gauge);
    arbiter
        .run_blocking_claimed(claim, move || {
            gauge.enter();
            gauge.wait_for(partners, Duration::from_millis(500));
            gauge.exit();
            Ok(())
        })
        .await
        .expect("claimed transaction should succeed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn device_claims_on_different_addresses_overlap() {
    let arbiter = SmBusBusArbiter::default();
    let gauge = Arc::new(OverlapGauge::default());

    tokio::join!(
        run_gauged_claim(&arbiter, SmBusClaim::Device { address: 0x71 }, &gauge, 2),
        run_gauged_claim(&arbiter, SmBusClaim::Device { address: 0x73 }, &gauge, 2),
    );

    assert_eq!(
        gauge.max_active.load(Ordering::SeqCst),
        2,
        "writes to different addresses must share the bus"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn device_claims_on_one_address_never_overlap() {
    let arbiter = SmBusBusArbiter::default();
    let gauge = Arc::new(OverlapGauge::default());

    tokio::join!(
        run_gauged_claim(&arbiter, SmBusClaim::Device { address: 0x71 }, &gauge, 2),
        run_gauged_claim(&arbiter, SmBusClaim::Device { address: 0x71 }, &gauge, 2),
    );

    assert_eq!(
        gauge.max_active.load(Ordering::SeqCst),
        1,
        "one address must see one writer at a time"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bus_claims_exclude_device_claims_in_both_directions() {
    let arbiter = SmBusBusArbiter::default();

    let device = arbiter.acquire(SmBusClaim::Device { address: 0x71 }).await;
    let mut bus = tokio::spawn({
        let arbiter = arbiter.clone();
        async move { arbiter.acquire_transaction().await }
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut bus)
            .await
            .is_err(),
        "a bus claim must wait for in-flight device writes"
    );
    drop(device);
    let bus = tokio::time::timeout(Duration::from_secs(1), bus)
        .await
        .expect("bus claim should proceed once device writes finish")
        .expect("bus claim task should complete");

    let mut device = tokio::spawn({
        let arbiter = arbiter.clone();
        async move {
            let _device = arbiter.acquire(SmBusClaim::Device { address: 0x73 }).await;
        }
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut device)
            .await
            .is_err(),
        "device writes must wait for a bus claim to finish"
    );
    drop(bus);
    tokio::time::timeout(Duration::from_secs(1), device)
        .await
        .expect("device claim should proceed once the bus claim ends")
        .expect("device claim task should complete");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn waiting_bus_claim_holds_back_new_device_claims() {
    let arbiter = SmBusBusArbiter::default();

    let streaming = arbiter.acquire(SmBusClaim::Device { address: 0x71 }).await;
    let mut probe = tokio::spawn({
        let arbiter = arbiter.clone();
        async move { arbiter.acquire_transaction().await }
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut probe)
            .await
            .is_err(),
        "the probe should be queued behind the in-flight write"
    );

    let mut late_writer = tokio::spawn({
        let arbiter = arbiter.clone();
        async move {
            let _device = arbiter.acquire(SmBusClaim::Device { address: 0x73 }).await;
        }
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut late_writer)
            .await
            .is_err(),
        "a queued bus claim must hold back new device writes"
    );

    drop(streaming);
    let probe = tokio::time::timeout(Duration::from_secs(1), probe)
        .await
        .expect("probe should run before the late writer")
        .expect("probe task should complete");
    assert!(
        !late_writer.is_finished(),
        "the late writer must still wait while the probe holds the bus"
    );
    drop(probe);
    tokio::time::timeout(Duration::from_secs(1), late_writer)
        .await
        .expect("late writer should run after the probe")
        .expect("late writer task should complete");
}
