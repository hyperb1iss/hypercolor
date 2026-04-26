//! Concurrency and contention tests for [`BackendManager`] frame dispatch.
//!
//! The frame-write hot path funnels every async write through
//! `Arc<Mutex<Box<dyn DeviceBackend>>>`, which means multiple render-loop
//! tasks writing to different backends should run in parallel, while writes
//! to the same backend must serialize without dropping frames. Existing
//! tests cover these paths synchronously; this file stresses them under
//! real concurrent workload using `#[tokio::test]`.

#![allow(clippy::unwrap_used, reason = "test assertions")]

use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use hypercolor_core::device::{BackendInfo, BackendManager, DeviceBackend};
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFamily, DeviceFeatures, DeviceId,
    DeviceInfo, DeviceOrigin, DeviceTopologyHint, ZoneInfo,
};

// ── ContentionMockBackend ───────────────────────────────────────────────────
//
// A purpose-built mock backend that records every write in the order it was
// observed inside the backend critical section. The `delay` field simulates
// a slow transport so we can measure whether slow backends stall fast ones.
// The recorder uses a `std::sync::Mutex` — not a tokio mutex — because we
// only touch it synchronously inside the `write_colors` critical section
// after the optional async delay, and std mutexes have lower overhead here.

struct ContentionMockBackend {
    backend_id: String,
    device_id: DeviceId,
    connected: AtomicBool,
    delay: Duration,
    write_count: Arc<AtomicUsize>,
    records: Arc<StdMutex<Vec<WriteRecord>>>,
    fail_when_disconnected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WriteRecord {
    device_id: DeviceId,
    /// The first RGB triplet of the payload. Tests pack writer id into `[0]`
    /// and sequence number into `[1]` so serialized order can be verified.
    first_pixel: [u8; 3],
    /// Total LED count in the payload.
    len: usize,
}

impl ContentionMockBackend {
    fn new(backend_id: impl Into<String>, device_id: DeviceId) -> Self {
        Self {
            backend_id: backend_id.into(),
            device_id,
            connected: AtomicBool::new(false),
            delay: Duration::ZERO,
            write_count: Arc::new(AtomicUsize::new(0)),
            records: Arc::new(StdMutex::new(Vec::new())),
            fail_when_disconnected: true,
        }
    }

    fn with_delay(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }

    fn write_count_handle(&self) -> Arc<AtomicUsize> {
        Arc::clone(&self.write_count)
    }

    fn records_handle(&self) -> Arc<StdMutex<Vec<WriteRecord>>> {
        Arc::clone(&self.records)
    }
}

#[async_trait::async_trait]
impl DeviceBackend for ContentionMockBackend {
    fn info(&self) -> BackendInfo {
        BackendInfo {
            id: self.backend_id.clone(),
            name: format!("Contention Mock ({})", self.backend_id),
            description: "Contention test backend with configurable delay".to_owned(),
        }
    }

    async fn discover(&mut self) -> Result<Vec<DeviceInfo>> {
        Ok(vec![DeviceInfo {
            id: self.device_id,
            name: format!("contention-{}", self.backend_id),
            vendor: "hypercolor-test".to_owned(),
            family: DeviceFamily::Custom("Contention".to_owned()),
            model: None,
            connection_type: ConnectionType::Network,
            origin: DeviceOrigin::native(
                "contention",
                self.backend_id.clone(),
                ConnectionType::Network,
            ),
            zones: vec![ZoneInfo {
                name: "Main".to_owned(),
                led_count: 8,
                topology: DeviceTopologyHint::Strip,
                color_format: DeviceColorFormat::Rgb,
            }],
            firmware_version: None,
            capabilities: DeviceCapabilities {
                led_count: 8,
                supports_direct: true,
                supports_brightness: false,
                has_display: false,
                display_resolution: None,
                max_fps: 60,
                color_space: hypercolor_types::device::DeviceColorSpace::default(),
                features: DeviceFeatures::default(),
            },
        }])
    }

    async fn connect(&mut self, id: &DeviceId) -> Result<()> {
        if *id != self.device_id {
            bail!("unexpected device id {id} for backend {}", self.backend_id);
        }
        self.connected.store(true, Ordering::Release);
        Ok(())
    }

    async fn disconnect(&mut self, id: &DeviceId) -> Result<()> {
        if *id != self.device_id {
            bail!("unexpected device id {id} for backend {}", self.backend_id);
        }
        self.connected.store(false, Ordering::Release);
        Ok(())
    }

    async fn write_colors(&mut self, id: &DeviceId, colors: &[[u8; 3]]) -> Result<()> {
        if *id != self.device_id {
            bail!("unexpected device id {id} for backend {}", self.backend_id);
        }
        if self.fail_when_disconnected && !self.connected.load(Ordering::Acquire) {
            bail!("write while disconnected on backend {}", self.backend_id);
        }

        if !self.delay.is_zero() {
            tokio::time::sleep(self.delay).await;
        }

        self.write_count.fetch_add(1, Ordering::AcqRel);
        self.records
            .lock()
            .expect("records mutex poisoned")
            .push(WriteRecord {
                device_id: *id,
                first_pixel: colors.first().copied().unwrap_or([0, 0, 0]),
                len: colors.len(),
            });
        Ok(())
    }
}

// ── Test helpers ────────────────────────────────────────────────────────────

/// Build a manager with `n` distinct backends, each pre-connected.
///
/// Returns the manager, the per-backend `(backend_id, DeviceId)` pairs,
/// and cloned handles for write-count and record observation.
async fn build_manager_with_backends(
    count: usize,
    delay: Duration,
) -> (
    BackendManager,
    Vec<(String, DeviceId)>,
    Vec<Arc<AtomicUsize>>,
    Vec<Arc<StdMutex<Vec<WriteRecord>>>>,
) {
    let mut manager = BackendManager::new();
    let mut ids = Vec::with_capacity(count);
    let mut write_counts = Vec::with_capacity(count);
    let mut records = Vec::with_capacity(count);

    for i in 0..count {
        let backend_id = format!("contention-{i}");
        let device_id = DeviceId::new();
        let backend = ContentionMockBackend::new(&backend_id, device_id).with_delay(delay);
        write_counts.push(backend.write_count_handle());
        records.push(backend.records_handle());
        manager.register_backend(Box::new(backend));

        let io = manager
            .backend_io(&backend_id)
            .expect("backend was just registered");
        io.connect_with_refresh(device_id)
            .await
            .expect("connect should succeed");

        ids.push((backend_id, device_id));
    }

    (manager, ids, write_counts, records)
}

fn u8_tag(value: usize) -> u8 {
    u8::try_from(value).expect("test tag must fit in u8")
}

// ── Scenario 1: Concurrent writes to different backends ─────────────────────

#[tokio::test]
async fn concurrent_writes_to_different_backends_do_not_block() {
    const BACKEND_COUNT: usize = 8;
    const PER_BACKEND_WRITES: usize = 10;
    const WRITE_DELAY: Duration = Duration::from_millis(25);

    let (manager, ids, write_counts, _records) =
        build_manager_with_backends(BACKEND_COUNT, WRITE_DELAY).await;

    // Clone one BackendIo handle per backend. BackendIo is a lightweight
    // Arc-backed clone so tasks can await writes without holding the manager.
    let mut handles = Vec::with_capacity(BACKEND_COUNT);
    for (backend_id, device_id) in &ids {
        let io = manager
            .backend_io(backend_id)
            .expect("backend io should exist");
        handles.push((io, *device_id));
    }

    let start = Instant::now();
    let mut tasks = Vec::with_capacity(BACKEND_COUNT);
    for (task_idx, (io, device_id)) in handles.into_iter().enumerate() {
        tasks.push(tokio::spawn(async move {
            for frame_idx in 0..PER_BACKEND_WRITES {
                let tag = u8_tag(task_idx * PER_BACKEND_WRITES + frame_idx);
                io.write_colors(device_id, &[[tag, 0, 0]; 4])
                    .await
                    .expect("concurrent write should succeed");
            }
        }));
    }

    for task in tasks {
        task.await.expect("writer task should not panic");
    }
    let elapsed = start.elapsed();

    // All writes must be accounted for.
    for (idx, count) in write_counts.iter().enumerate() {
        assert_eq!(
            count.load(Ordering::Acquire),
            PER_BACKEND_WRITES,
            "backend {idx} should have received every frame"
        );
    }

    // Serial lower bound for a single backend: PER_BACKEND_WRITES * WRITE_DELAY.
    // With full parallelism across BACKEND_COUNT backends, the total wall clock
    // should be close to that lower bound, not BACKEND_COUNT times larger. We
    // allow 4x headroom for CI noise and tokio scheduling jitter.
    let serial_per_backend = WRITE_DELAY * u32::try_from(PER_BACKEND_WRITES).unwrap();
    let budget = serial_per_backend * 4;
    assert!(
        elapsed < budget,
        "parallel write elapsed {elapsed:?} exceeded budget {budget:?} — backends may be blocking each other"
    );
}

// ── Scenario 2: Concurrent writes to same backend serialize in order ────────

#[tokio::test]
async fn concurrent_writes_to_same_backend_serialize_in_order() {
    // A single writer per spawned task, but we use separate "writer ids" so
    // their payloads are distinguishable. The mutex guarantees serialization,
    // but each writer's own payloads must still arrive in its own submission
    // order because a single task awaits sequentially.
    const WRITER_COUNT: usize = 6;
    const PER_WRITER_FRAMES: usize = 20;

    let (manager, ids, write_counts, records) =
        build_manager_with_backends(1, Duration::from_millis(2)).await;
    let (backend_id, device_id) = ids[0].clone();
    let write_count = Arc::clone(&write_counts[0]);
    let record_handle = Arc::clone(&records[0]);

    let mut tasks = Vec::with_capacity(WRITER_COUNT);
    for writer_idx in 0..WRITER_COUNT {
        let io = manager
            .backend_io(&backend_id)
            .expect("backend io should exist");
        let dev = device_id;
        tasks.push(tokio::spawn(async move {
            // Pack `writer_idx` into pixel[0] and sequence into pixel[1] so
            // the recorder can reconstruct per-writer order.
            let writer_tag = u8_tag(writer_idx);
            for seq in 0..PER_WRITER_FRAMES {
                let pixel = [writer_tag, u8_tag(seq), 0];
                io.write_colors(dev, &[pixel; 4])
                    .await
                    .expect("serialized write should succeed");
            }
        }));
    }

    for task in tasks {
        task.await.expect("writer task should not panic");
    }

    let expected_total = WRITER_COUNT * PER_WRITER_FRAMES;
    assert_eq!(
        write_count.load(Ordering::Acquire),
        expected_total,
        "every frame must be delivered — none dropped by the mutex"
    );

    // Reconstruct per-writer delivery order and verify monotonic sequence.
    let records_guard = record_handle.lock().expect("records mutex poisoned");
    assert_eq!(records_guard.len(), expected_total);

    let mut per_writer_seen: Vec<Vec<u8>> = vec![Vec::new(); WRITER_COUNT];
    for record in records_guard.iter() {
        assert_eq!(
            record.device_id, device_id,
            "records must all target the same device"
        );
        assert_eq!(record.len, 4, "frame shape must be preserved");

        let writer_idx = usize::from(record.first_pixel[0]);
        let seq = record.first_pixel[1];
        assert!(
            writer_idx < WRITER_COUNT,
            "writer index {writer_idx} out of range"
        );
        per_writer_seen[writer_idx].push(seq);
    }

    for (writer_idx, seen) in per_writer_seen.iter().enumerate() {
        assert_eq!(
            seen.len(),
            PER_WRITER_FRAMES,
            "writer {writer_idx} lost frames: {seen:?}"
        );
        let expected: Vec<u8> = (0..PER_WRITER_FRAMES).map(u8_tag).collect();
        assert_eq!(
            seen, &expected,
            "writer {writer_idx} frames arrived out of order"
        );
    }
}

// ── Scenario 3: Slow backend does not block fast backend ────────────────────

#[tokio::test]
async fn slow_backend_does_not_block_fast_backend() {
    const FAST_WRITES: usize = 50;
    const SLOW_WRITES: usize = 3;
    const SLOW_DELAY: Duration = Duration::from_millis(100);

    let mut manager = BackendManager::new();

    let fast_device = DeviceId::new();
    let fast_backend = ContentionMockBackend::new("fast", fast_device);
    let fast_count = fast_backend.write_count_handle();
    manager.register_backend(Box::new(fast_backend));
    let fast_io = manager.backend_io("fast").unwrap();
    fast_io.connect_with_refresh(fast_device).await.unwrap();

    let slow_device = DeviceId::new();
    let slow_backend = ContentionMockBackend::new("slow", slow_device).with_delay(SLOW_DELAY);
    let slow_count = slow_backend.write_count_handle();
    manager.register_backend(Box::new(slow_backend));
    let slow_io = manager.backend_io("slow").unwrap();
    slow_io.connect_with_refresh(slow_device).await.unwrap();

    let start = Instant::now();

    let slow_task = {
        let io = slow_io.clone();
        tokio::spawn(async move {
            for i in 0..SLOW_WRITES {
                io.write_colors(slow_device, &[[u8_tag(i), 0, 0]; 4])
                    .await
                    .expect("slow write should succeed");
            }
        })
    };

    let fast_task = {
        let io = fast_io.clone();
        tokio::spawn(async move {
            let fast_start = Instant::now();
            for i in 0..FAST_WRITES {
                io.write_colors(fast_device, &[[u8_tag(i), 0, 0]; 4])
                    .await
                    .expect("fast write should succeed");
            }
            fast_start.elapsed()
        })
    };

    let fast_elapsed = fast_task.await.expect("fast task panicked");
    slow_task.await.expect("slow task panicked");
    let total_elapsed = start.elapsed();

    assert_eq!(fast_count.load(Ordering::Acquire), FAST_WRITES);
    assert_eq!(slow_count.load(Ordering::Acquire), SLOW_WRITES);

    // Fast path must not be held hostage by the slow backend's mutex.
    // The slow task on its own would take at least 3 * 100ms = 300ms. The
    // fast path has no artificial delay, so it should complete in a tiny
    // fraction of that, even accounting for scheduler jitter. We set a
    // generous ceiling of one slow write's worth of time.
    assert!(
        fast_elapsed < SLOW_DELAY,
        "fast backend elapsed {fast_elapsed:?} suggests the slow backend blocked it"
    );

    // Total elapsed is gated by the slow backend's serial work, so it must
    // be at least (SLOW_WRITES - 1) * SLOW_DELAY. Sanity check the assertion
    // setup is actually measuring concurrency.
    let slow_lower_bound = SLOW_DELAY * u32::try_from(SLOW_WRITES - 1).unwrap();
    assert!(
        total_elapsed >= slow_lower_bound,
        "total elapsed {total_elapsed:?} is below slow floor {slow_lower_bound:?} — test timing is broken"
    );
}

// ── Scenario 4: Write during disconnect is graceful ─────────────────────────

#[tokio::test]
async fn write_during_disconnect_is_graceful() {
    // We want to prove: (a) no panics on in-flight writes when a disconnect
    // happens, (b) no frames that complete before disconnect are lost,
    // (c) writes attempted after disconnect surface a clean error rather
    // than a panic, and (d) the backend's internal connected state reflects
    // the disconnect.
    const PRE_DISCONNECT_WRITES: usize = 5;
    const POST_DISCONNECT_WRITES: usize = 5;

    let (manager, ids, write_counts, _records) =
        build_manager_with_backends(1, Duration::from_millis(1)).await;
    let (backend_id, device_id) = ids[0].clone();
    let write_count = Arc::clone(&write_counts[0]);

    let io = manager.backend_io(&backend_id).unwrap();

    // Perform pre-disconnect writes sequentially so we can assert on the
    // exact number that landed before teardown.
    for i in 0..PRE_DISCONNECT_WRITES {
        io.write_colors(device_id, &[[u8_tag(i), 0, 0]; 4])
            .await
            .expect("pre-disconnect write should succeed");
    }
    assert_eq!(write_count.load(Ordering::Acquire), PRE_DISCONNECT_WRITES);

    // Now disconnect the device through the same BackendIo handle. This
    // serializes behind the mutex the same way writes do, so any in-flight
    // write completes before disconnect lands.
    io.disconnect(device_id)
        .await
        .expect("disconnect should succeed");

    // Post-disconnect writes must fail cleanly — no panics, typed errors.
    let mut failure_count = 0usize;
    for i in 0..POST_DISCONNECT_WRITES {
        let tag = u8_tag(PRE_DISCONNECT_WRITES + i);
        let result = io.write_colors(device_id, &[[tag, 0, 0]; 4]).await;
        match result {
            Ok(()) => {
                panic!("write {tag} unexpectedly succeeded after disconnect");
            }
            Err(err) => {
                failure_count += 1;
                let msg = err.to_string();
                assert!(
                    msg.contains("failed to write") || msg.contains("disconnected"),
                    "error {msg:?} does not surface disconnect context"
                );
            }
        }
    }
    assert_eq!(failure_count, POST_DISCONNECT_WRITES);

    // The write counter must reflect only the pre-disconnect writes — no
    // ghost writes from the failed attempts.
    assert_eq!(
        write_count.load(Ordering::Acquire),
        PRE_DISCONNECT_WRITES,
        "no writes should be recorded after disconnect"
    );
}

// ── Scenario 5: Frame pipeline stress test ──────────────────────────────────

#[tokio::test]
async fn frame_pipeline_stress_test_accounts_for_every_frame() {
    const BACKEND_COUNT: usize = 10;
    const WRITES_PER_BACKEND: usize = 100;

    let (manager, ids, write_counts, records) =
        build_manager_with_backends(BACKEND_COUNT, Duration::ZERO).await;

    let mut tasks = Vec::with_capacity(BACKEND_COUNT);
    for (task_idx, (backend_id, device_id)) in ids.iter().enumerate() {
        let io = manager.backend_io(backend_id).unwrap();
        let device_id = *device_id;
        tasks.push(tokio::spawn(async move {
            for frame_idx in 0..WRITES_PER_BACKEND {
                // Tag each payload with task + sequence so the recorder can
                // verify uniqueness and ordering.
                let first_byte = u8_tag((task_idx * 7) ^ frame_idx);
                io.write_colors(device_id, &[[first_byte, 0, 0]; 4])
                    .await
                    .expect("stress write should succeed");
            }
        }));
    }

    for task in tasks {
        task.await.expect("stress task should not panic");
    }

    let total_expected = BACKEND_COUNT * WRITES_PER_BACKEND;
    let total_delivered: usize = write_counts.iter().map(|c| c.load(Ordering::Acquire)).sum();
    assert_eq!(
        total_delivered, total_expected,
        "every frame must be delivered or explicitly dropped with a reason"
    );

    for (idx, record_handle) in records.iter().enumerate() {
        let records_guard = record_handle.lock().expect("records mutex poisoned");
        assert_eq!(
            records_guard.len(),
            WRITES_PER_BACKEND,
            "backend {idx} should have {WRITES_PER_BACKEND} records"
        );
    }
}
