use hypercolor_core::input::media::{
    ArtCache, ArtworkBlockingBackend, ArtworkError, ArtworkFetcher, ArtworkPolicy, ArtworkRequest,
    ArtworkSource, MediaMetadataProvider, MediaProviderError, MediaProviderFailure,
    MediaProviderSession, MediaSource, PlaybackStatus, PlayerSnapshot, collect_player_snapshots,
    media_state_from_player, pick_active_player, run_artwork_loop,
};
use hypercolor_core::input::{InputData, InputSource};
use hypercolor_core::input::{SourceFreshness, SourceState};
use image::ImageEncoder as _;
use stats_alloc::{INSTRUMENTED_SYSTEM, Region, StatsAlloc};
use std::alloc::System;
use std::collections::VecDeque;
use std::io::{Cursor, Read as _, Write as _};
use std::net::TcpListener;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

#[global_allocator]
static GLOBAL: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;

fn player(bus_name: &str, status: PlaybackStatus, track: &str) -> PlayerSnapshot {
    PlayerSnapshot {
        bus_name: bus_name.to_owned(),
        status,
        track: track.to_owned(),
        artist: "Artist".to_owned(),
        album: "Album".to_owned(),
        artwork: None,
        position_ms: 1_000,
        duration_ms: 200_000,
    }
}

#[test]
fn playing_player_beats_paused_player() {
    let players = vec![
        player(
            "org.mpris.MediaPlayer2.firefox",
            PlaybackStatus::Paused,
            "a",
        ),
        player(
            "org.mpris.MediaPlayer2.spotify",
            PlaybackStatus::Playing,
            "b",
        ),
    ];

    let picked = pick_active_player(&players, None).expect("a player should be picked");
    assert_eq!(picked.bus_name, "org.mpris.MediaPlayer2.spotify");
}

#[test]
fn paused_player_beats_stopped_player() {
    let players = vec![
        player("org.mpris.MediaPlayer2.mpv", PlaybackStatus::Stopped, "a"),
        player(
            "org.mpris.MediaPlayer2.firefox",
            PlaybackStatus::Paused,
            "b",
        ),
    ];

    let picked = pick_active_player(&players, None).expect("a player should be picked");
    assert_eq!(picked.bus_name, "org.mpris.MediaPlayer2.firefox");
}

#[test]
fn previously_active_player_wins_ties() {
    let players = vec![
        player(
            "org.mpris.MediaPlayer2.firefox",
            PlaybackStatus::Paused,
            "a",
        ),
        player(
            "org.mpris.MediaPlayer2.spotify",
            PlaybackStatus::Paused,
            "b",
        ),
    ];

    let picked = pick_active_player(&players, Some("org.mpris.MediaPlayer2.spotify"))
        .expect("a player should be picked");
    assert_eq!(picked.bus_name, "org.mpris.MediaPlayer2.spotify");

    let picked = pick_active_player(&players, None).expect("a player should be picked");
    assert_eq!(picked.bus_name, "org.mpris.MediaPlayer2.firefox");
}

#[test]
fn stickiness_does_not_override_a_playing_player() {
    let players = vec![
        player(
            "org.mpris.MediaPlayer2.spotify",
            PlaybackStatus::Playing,
            "a",
        ),
        player(
            "org.mpris.MediaPlayer2.firefox",
            PlaybackStatus::Paused,
            "b",
        ),
    ];

    let picked = pick_active_player(&players, Some("org.mpris.MediaPlayer2.firefox"))
        .expect("a player should be picked");
    assert_eq!(picked.bus_name, "org.mpris.MediaPlayer2.spotify");
}

#[test]
fn no_players_picks_nothing() {
    assert!(pick_active_player(&[], None).is_none());
}

#[test]
fn art_cache_fetches_once_per_track() {
    let mut cache = ArtCache::new();
    let mut snapshot = player(
        "org.mpris.MediaPlayer2.spotify",
        PlaybackStatus::Playing,
        "a",
    );
    snapshot.artwork = Some(ArtworkSource::Url("file:///art/a.jpg".to_owned()));

    let mut fetches = 0;
    let art = cache.resolve(&snapshot, |source| {
        fetches += 1;
        Some(format!("data:{}", artwork_url(source)))
    });
    assert_eq!(art.as_deref(), Some("data:file:///art/a.jpg"));
    assert_eq!(fetches, 1);

    // Same track polled again: cached, fetcher not invoked.
    let art = cache.resolve(&snapshot, |_| {
        fetches += 1;
        Some("data:second".to_owned())
    });
    assert_eq!(art.as_deref(), Some("data:file:///art/a.jpg"));
    assert_eq!(fetches, 1);

    // Track change: fetcher runs again.
    snapshot.track = "b".to_owned();
    snapshot.artwork = Some(ArtworkSource::Url("file:///art/b.jpg".to_owned()));
    let art = cache.resolve(&snapshot, |source| {
        fetches += 1;
        Some(format!("data:{}", artwork_url(source)))
    });
    assert_eq!(art.as_deref(), Some("data:file:///art/b.jpg"));
    assert_eq!(fetches, 2);
}

fn artwork_url(source: &ArtworkSource) -> &str {
    match source {
        ArtworkSource::Url(url) => url,
        ArtworkSource::WindowsSession(_) => panic!("test expected URL artwork"),
    }
}

fn artwork_policy(max_source_bytes: usize) -> ArtworkPolicy {
    ArtworkPolicy {
        fetch_timeout: Duration::from_millis(250),
        max_source_bytes,
        max_source_dimension: 1_024,
        max_source_pixels: 1_048_576,
        max_decode_bytes: 4 * 1_048_576,
        max_output_dimension: 64,
        max_data_url_bytes: 128 * 1_024,
        max_redirects: 1,
    }
}

#[test]
fn custom_artwork_policy_cannot_expand_the_hard_safety_envelope() {
    let mut policy = ArtworkPolicy::default();
    policy.max_source_bytes += 1;
    assert!(matches!(
        ArtworkFetcher::new(policy),
        Err(ArtworkError::InvalidPolicy)
    ));
}

fn png_with_icc_profile(profile_bytes: usize) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut encoder = image::codecs::png::PngEncoder::new(&mut bytes);
    encoder
        .set_icc_profile(vec![0; profile_bytes])
        .expect("test ICC profile is accepted");
    encoder
        .write_image(&[0, 0, 0, 255], 1, 1, image::ExtendedColorType::Rgba8)
        .expect("test PNG encodes");
    bytes
}

#[test]
fn compressed_png_metadata_obeys_the_decode_allocation_limit() {
    const CHILD_ENV: &str = "HYPERCOLOR_MEDIA_ICC_ALLOCATION_CHILD";
    if std::env::var_os(CHILD_ENV).is_none() {
        let status = std::process::Command::new(std::env::current_exe().expect("test path exists"))
            .args([
                "--exact",
                "compressed_png_metadata_obeys_the_decode_allocation_limit",
                "--nocapture",
            ])
            .env(CHILD_ENV, "1")
            .status()
            .expect("allocation probe child starts");
        assert!(status.success(), "allocation probe child failed");
        return;
    }

    let bytes = png_with_icc_profile(64 * 1024 * 1024);
    assert!(
        bytes.len() < 256 * 1024,
        "fixture must remain highly compressed"
    );
    let mut policy = artwork_policy(bytes.len());
    policy.max_decode_bytes = 512 * 1024;
    let fetcher = ArtworkFetcher::new(policy).expect("bounded policy builds");
    let mut region = Region::new(GLOBAL);
    region.reset();

    let result = fetcher.encode_data_url(&bytes);
    let allocated = region.change().bytes_allocated;

    assert!(
        result.is_ok(),
        "oversized optional metadata is ignored safely"
    );
    assert!(
        allocated < 16 * 1024 * 1024,
        "dimension probing allocated {allocated} bytes for bounded ancillary metadata"
    );
}

#[derive(Clone, Copy)]
enum SnapshotOutcome {
    Healthy(&'static str),
    Failed,
    Hung,
}

async fn scripted_snapshot(
    outcome: SnapshotOutcome,
) -> std::result::Result<PlayerSnapshot, MediaProviderError> {
    match outcome {
        SnapshotOutcome::Healthy(track) => {
            Ok(player("native-player", PlaybackStatus::Playing, track))
        }
        SnapshotOutcome::Failed => Err(MediaProviderError::new("player failed")),
        SnapshotOutcome::Hung => std::future::pending().await,
    }
}

#[tokio::test]
async fn unhealthy_native_players_do_not_starve_healthy_siblings() {
    for outcomes in [
        [
            SnapshotOutcome::Hung,
            SnapshotOutcome::Healthy("after-hang"),
        ],
        [
            SnapshotOutcome::Healthy("before-hang"),
            SnapshotOutcome::Hung,
        ],
        [
            SnapshotOutcome::Failed,
            SnapshotOutcome::Healthy("after-failure"),
        ],
        [
            SnapshotOutcome::Healthy("before-failure"),
            SnapshotOutcome::Failed,
        ],
    ] {
        let started = Instant::now();
        let players = collect_player_snapshots(
            outcomes.into_iter().map(scripted_snapshot),
            Duration::from_millis(20),
        )
        .await
        .expect("one unhealthy player must not fail a healthy sibling");
        assert_eq!(players.len(), 1);
        assert!(players[0].track.contains("hang") || players[0].track.contains("failure"));
        assert!(started.elapsed() < Duration::from_millis(250));
    }

    let outcomes = std::iter::repeat_n(SnapshotOutcome::Hung, 8)
        .chain(std::iter::once(SnapshotOutcome::Healthy("survivor")));
    let started = Instant::now();
    let players =
        collect_player_snapshots(outcomes.map(scripted_snapshot), Duration::from_millis(150))
            .await
            .expect("concurrent timeouts must preserve the healthy player");
    assert_eq!(players.len(), 1);
    assert_eq!(players[0].track, "survivor");
    assert!(started.elapsed() < Duration::from_millis(400));

    let error = collect_player_snapshots(
        [SnapshotOutcome::Hung, SnapshotOutcome::Failed]
            .into_iter()
            .map(scripted_snapshot),
        Duration::from_millis(20),
    )
    .await
    .expect_err("every attempted player failed");
    assert!(
        error
            .to_string()
            .starts_with("native media player 0 exceeded"),
        "lowest-indexed failure must win, got {error}"
    );
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BlockingStage {
    Read,
    Encode,
}

struct CancelAwareBlockingBackend {
    stage: BlockingStage,
    active: AtomicUsize,
    max_active: AtomicUsize,
    starts: AtomicUsize,
}

impl CancelAwareBlockingBackend {
    fn new(stage: BlockingStage) -> Self {
        Self {
            stage,
            active: AtomicUsize::new(0),
            max_active: AtomicUsize::new(0),
            starts: AtomicUsize::new(0),
        }
    }

    fn block_until_cancelled<T>(
        &self,
        cancel: &CancellationToken,
    ) -> std::result::Result<T, ArtworkError> {
        self.starts.fetch_add(1, Ordering::SeqCst);
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_active.fetch_max(active, Ordering::SeqCst);
        while !cancel.is_cancelled() {
            std::thread::sleep(Duration::from_millis(1));
        }
        self.active.fetch_sub(1, Ordering::SeqCst);
        Err(ArtworkError::Cancelled)
    }
}

impl ArtworkBlockingBackend for CancelAwareBlockingBackend {
    fn read_file(
        &self,
        _path: &Path,
        _limit: usize,
        cancel: &CancellationToken,
    ) -> std::result::Result<Vec<u8>, ArtworkError> {
        if self.stage == BlockingStage::Read {
            self.block_until_cancelled(cancel)
        } else {
            Ok(vec![1])
        }
    }

    fn encode(
        &self,
        _bytes: &[u8],
        _policy: ArtworkPolicy,
        cancel: &CancellationToken,
    ) -> std::result::Result<String, ArtworkError> {
        if self.stage == BlockingStage::Encode {
            self.block_until_cancelled(cancel)
        } else {
            Ok("data:image/jpeg;base64,test".to_owned())
        }
    }
}

#[tokio::test]
async fn blocking_artwork_timeout_reaps_read_and_decode_jobs() {
    for stage in [BlockingStage::Read, BlockingStage::Encode] {
        let backend = Arc::new(CancelAwareBlockingBackend::new(stage));
        let mut policy = artwork_policy(64);
        policy.fetch_timeout = Duration::from_millis(20);
        let fetcher = ArtworkFetcher::with_blocking_backend(policy, backend.clone())
            .expect("test backend builds");
        let source = ArtworkSource::Url("file:///C:/hypercolor-artwork-test".to_owned());

        assert_eq!(
            fetcher.fetch_data_url(&source).await,
            Err(ArtworkError::Timeout)
        );
        assert_eq!(backend.active.load(Ordering::SeqCst), 0);
        assert_eq!(backend.starts.load(Ordering::SeqCst), 1);
        assert_eq!(backend.max_active.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn explicit_artwork_cancellation_reaps_the_blocking_job() {
    let backend = Arc::new(CancelAwareBlockingBackend::new(BlockingStage::Encode));
    let fetcher = ArtworkFetcher::with_blocking_backend(artwork_policy(64), backend.clone())
        .expect("test backend builds");
    let source = ArtworkSource::Url("file:///C:/hypercolor-artwork-test".to_owned());
    let cancel = CancellationToken::new();
    let cancel_after_start = async {
        wait_for_attempts(&backend.starts, 1).await;
        cancel.cancel();
    };

    let (result, ()) = tokio::join!(
        fetcher.fetch_data_url_cancellable(&source, &cancel),
        cancel_after_start,
    );
    assert_eq!(result, Err(ArtworkError::Cancelled));
    assert_eq!(backend.active.load(Ordering::SeqCst), 0);
    assert_eq!(backend.starts.load(Ordering::SeqCst), 1);
    assert_eq!(backend.max_active.load(Ordering::SeqCst), 1);
}

struct UncancellableBlockingBackend {
    released: AtomicBool,
    active: AtomicUsize,
    starts: AtomicUsize,
}

impl ArtworkBlockingBackend for UncancellableBlockingBackend {
    fn read_file(
        &self,
        _path: &Path,
        _limit: usize,
        _cancel: &CancellationToken,
    ) -> std::result::Result<Vec<u8>, ArtworkError> {
        Ok(vec![1])
    }

    fn encode(
        &self,
        _bytes: &[u8],
        _policy: ArtworkPolicy,
        _cancel: &CancellationToken,
    ) -> std::result::Result<String, ArtworkError> {
        self.starts.fetch_add(1, Ordering::SeqCst);
        self.active.fetch_add(1, Ordering::SeqCst);
        while !self.released.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(1));
        }
        self.active.fetch_sub(1, Ordering::SeqCst);
        Err(ArtworkError::Cancelled)
    }
}

#[tokio::test]
async fn uncooperative_artwork_job_is_quarantined_without_overlap() {
    let backend = Arc::new(UncancellableBlockingBackend {
        released: AtomicBool::new(false),
        active: AtomicUsize::new(0),
        starts: AtomicUsize::new(0),
    });
    let mut policy = artwork_policy(64);
    policy.fetch_timeout = Duration::from_millis(20);
    let fetcher = ArtworkFetcher::with_blocking_backend(policy, backend.clone())
        .expect("test backend builds");
    let source = ArtworkSource::Url("file:///C:/hypercolor-artwork-test".to_owned());

    let started = Instant::now();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(3), fetcher.fetch_data_url(&source))
            .await
            .expect("the quarantined job cannot wedge its caller"),
        Err(ArtworkError::Timeout)
    );
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(backend.active.load(Ordering::SeqCst), 1);
    assert_eq!(backend.starts.load(Ordering::SeqCst), 1);

    assert_eq!(
        fetcher.fetch_data_url(&source).await,
        Err(ArtworkError::Timeout)
    );
    assert_eq!(backend.starts.load(Ordering::SeqCst), 1);
    backend.released.store(true, Ordering::SeqCst);
    tokio::time::timeout(Duration::from_secs(2), async {
        while backend.active.load(Ordering::SeqCst) != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("quarantined artwork job eventually exits");
}

struct ReplacementBlockingBackend {
    active: AtomicUsize,
    max_active: AtomicUsize,
    starts: AtomicUsize,
}

impl ReplacementBlockingBackend {
    fn new() -> Self {
        Self {
            active: AtomicUsize::new(0),
            max_active: AtomicUsize::new(0),
            starts: AtomicUsize::new(0),
        }
    }
}

impl ArtworkBlockingBackend for ReplacementBlockingBackend {
    fn read_file(
        &self,
        path: &Path,
        _limit: usize,
        _cancel: &CancellationToken,
    ) -> std::result::Result<Vec<u8>, ArtworkError> {
        Ok(path.to_string_lossy().as_bytes().to_vec())
    }

    fn encode(
        &self,
        _bytes: &[u8],
        _policy: ArtworkPolicy,
        cancel: &CancellationToken,
    ) -> std::result::Result<String, ArtworkError> {
        let attempt = self.starts.fetch_add(1, Ordering::SeqCst) + 1;
        if attempt == 2 {
            return Err(ArtworkError::Io("retry me".to_owned()));
        }
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_active.fetch_max(active, Ordering::SeqCst);
        while !cancel.is_cancelled() {
            std::thread::sleep(Duration::from_millis(1));
        }
        self.active.fetch_sub(1, Ordering::SeqCst);
        Err(ArtworkError::Cancelled)
    }
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
struct ImmediateArtworkBackend {
    starts: AtomicUsize,
    released: AtomicBool,
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
impl ArtworkBlockingBackend for ImmediateArtworkBackend {
    fn read_file(
        &self,
        _path: &Path,
        _limit: usize,
        _cancel: &CancellationToken,
    ) -> std::result::Result<Vec<u8>, ArtworkError> {
        Ok(vec![1])
    }

    fn encode(
        &self,
        _bytes: &[u8],
        _policy: ArtworkPolicy,
        _cancel: &CancellationToken,
    ) -> std::result::Result<String, ArtworkError> {
        self.starts.fetch_add(1, Ordering::SeqCst);
        while !self.released.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(1));
        }
        Ok("data:image/jpeg;base64,stale".to_owned())
    }
}

async fn wait_for_attempts(attempts: &AtomicUsize, expected: usize) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while attempts.load(Ordering::SeqCst) < expected {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("artwork attempt starts before deadline");
}

#[tokio::test]
async fn replacement_retry_and_stop_keep_blocking_artwork_single_flight() {
    let backend = Arc::new(ReplacementBlockingBackend::new());
    let mut policy = artwork_policy(128);
    policy.fetch_timeout = Duration::from_secs(5);
    let fetcher = ArtworkFetcher::with_blocking_backend(policy, backend.clone())
        .expect("test backend builds");
    let source = MediaSource::new();
    let (art_tx, art_rx) = tokio::sync::watch::channel(None);
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let worker = run_artwork_loop(fetcher, source.publisher(), art_rx, stop_rx);
    let drive = async {
        art_tx.send_replace(Some(Arc::new(ArtworkRequest {
            key: "old".to_owned(),
            source: ArtworkSource::Url("file:///C:/old-art".to_owned()),
        })));
        wait_for_attempts(&backend.starts, 1).await;
        art_tx.send_replace(Some(Arc::new(ArtworkRequest {
            key: "new".to_owned(),
            source: ArtworkSource::Url("file:///C:/new-art".to_owned()),
        })));
        wait_for_attempts(&backend.starts, 2).await;
        wait_for_attempts(&backend.starts, 3).await;
        stop_tx.send_replace(true);
    };

    tokio::time::timeout(Duration::from_secs(4), async {
        tokio::join!(worker, drive);
    })
    .await
    .expect("artwork worker stops before deadline");
    assert_eq!(backend.active.load(Ordering::SeqCst), 0);
    assert_eq!(backend.starts.load(Ordering::SeqCst), 3);
    assert_eq!(backend.max_active.load(Ordering::SeqCst), 1);
}

fn spawn_http_response(headers: &str, body: Vec<u8>, delay: Duration) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("test listener binds");
    let address = listener.local_addr().expect("test listener has an address");
    let headers = headers.to_owned();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("test server accepts one request");
        let mut request = [0_u8; 2_048];
        let mut received = 0;
        while received < request.len() {
            let count = stream
                .read(&mut request[received..])
                .expect("test server reads request");
            if count == 0 {
                break;
            }
            received += count;
            if request[..received]
                .windows(4)
                .any(|window| window == b"\r\n\r\n")
            {
                break;
            }
        }
        std::thread::sleep(delay);
        let response = format!("HTTP/1.1 200 OK\r\n{headers}\r\n");
        let _ = stream.write_all(response.as_bytes());
        let _ = stream.write_all(&body);
    });
    format!("http://{address}/art")
}

fn one_pixel_png() -> Vec<u8> {
    let image = image::DynamicImage::new_rgb8(1, 1);
    let mut bytes = Cursor::new(Vec::new());
    image
        .write_to(&mut bytes, image::ImageFormat::Png)
        .expect("test PNG encodes");
    bytes.into_inner()
}

fn huge_png_header(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
    bytes.extend_from_slice(&13_u32.to_be_bytes());
    let mut header = b"IHDR".to_vec();
    header.extend_from_slice(&width.to_be_bytes());
    header.extend_from_slice(&height.to_be_bytes());
    header.extend_from_slice(&[8, 2, 0, 0, 0]);
    bytes.extend_from_slice(&header);
    bytes.extend_from_slice(&crc32(&header).to_be_bytes());
    for chunk_type in [b"IDAT", b"IEND"] {
        bytes.extend_from_slice(&0_u32.to_be_bytes());
        bytes.extend_from_slice(chunk_type);
        bytes.extend_from_slice(&crc32(chunk_type).to_be_bytes());
    }
    bytes
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = 0_u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

#[tokio::test]
async fn unknown_length_http_art_aborts_at_the_streaming_byte_limit() {
    let fetcher = ArtworkFetcher::new(artwork_policy(64)).expect("test policy builds");
    let url = spawn_http_response("Connection: close\r\n", vec![0xAB; 65], Duration::ZERO);

    let error = fetcher
        .fetch_data_url(&ArtworkSource::Url(url))
        .await
        .expect_err("unknown-length body must be bounded while streaming");
    assert_eq!(error, ArtworkError::SourceTooLarge { limit: 64 });
}

#[tokio::test]
async fn declared_oversized_http_art_is_rejected_before_body_buffering() {
    let fetcher = ArtworkFetcher::new(artwork_policy(64)).expect("test policy builds");
    let url = spawn_http_response(
        "Content-Length: 65\r\nConnection: close\r\n",
        vec![0xAB; 65],
        Duration::ZERO,
    );

    let error = fetcher
        .fetch_data_url(&ArtworkSource::Url(url))
        .await
        .expect_err("declared oversized body must be rejected");
    assert_eq!(error, ArtworkError::SourceTooLarge { limit: 64 });
}

#[tokio::test]
async fn oversized_local_art_is_rejected_before_full_read() {
    let mut file = tempfile::NamedTempFile::new().expect("temporary artwork opens");
    file.write_all(&[0xAB; 65])
        .expect("temporary artwork writes");
    let url = url::Url::from_file_path(file.path())
        .expect("temporary path becomes a file URL")
        .to_string();
    let fetcher = ArtworkFetcher::new(artwork_policy(64)).expect("test policy builds");

    let error = fetcher
        .fetch_data_url(&ArtworkSource::Url(url))
        .await
        .expect_err("oversized local artwork must be rejected");
    assert_eq!(error, ArtworkError::SourceTooLarge { limit: 64 });
}

#[tokio::test]
async fn non_regular_local_art_is_rejected_before_open() {
    let directory = tempfile::tempdir().expect("temporary artwork directory opens");
    let url = url::Url::from_directory_path(directory.path())
        .expect("temporary directory becomes a file URL")
        .to_string();
    let fetcher = ArtworkFetcher::new(artwork_policy(64)).expect("test policy builds");

    let error = fetcher
        .fetch_data_url(&ArtworkSource::Url(url))
        .await
        .expect_err("non-regular local artwork must be rejected");
    assert_eq!(error, ArtworkError::UnsupportedSource);
}

#[test]
fn huge_compressed_dimensions_are_rejected_before_pixel_allocation() {
    let fetcher = ArtworkFetcher::new(artwork_policy(256)).expect("test policy builds");
    let error = fetcher
        .encode_data_url(&huge_png_header(50_000, 50_000))
        .expect_err("huge dimensions must fail before decode allocation");
    assert_eq!(
        error,
        ArtworkError::DimensionsTooLarge {
            width: 50_000,
            height: 50_000,
        }
    );
}

#[test]
fn encoded_data_url_has_an_independent_output_limit() {
    let mut policy = artwork_policy(1_024);
    policy.max_data_url_bytes = 32;
    let fetcher = ArtworkFetcher::new(policy).expect("test policy builds");

    assert_eq!(
        fetcher
            .encode_data_url(&one_pixel_png())
            .expect_err("data URL over the output limit must fail"),
        ArtworkError::OutputTooLarge { limit: 32 }
    );
}

struct ScriptedProvider {
    connect_results: VecDeque<Result<(), MediaProviderError>>,
    poll_results: VecDeque<Result<Vec<PlayerSnapshot>, MediaProviderError>>,
    connects: Arc<AtomicUsize>,
    disconnects: Arc<AtomicUsize>,
}

#[async_trait::async_trait(?Send)]
impl MediaMetadataProvider for ScriptedProvider {
    fn backend_name(&self) -> &'static str {
        "scripted"
    }

    async fn connect(&mut self) -> Result<(), MediaProviderError> {
        self.connects.fetch_add(1, Ordering::Relaxed);
        self.connect_results.pop_front().unwrap_or(Ok(()))
    }

    async fn poll_players(&mut self) -> Result<Vec<PlayerSnapshot>, MediaProviderError> {
        self.poll_results
            .pop_front()
            .unwrap_or_else(|| Ok(Vec::new()))
    }

    fn disconnect(&mut self) {
        self.disconnects.fetch_add(1, Ordering::Relaxed);
    }
}

#[tokio::test]
async fn provider_recovers_after_bus_loss_and_tracks_replacement_player() {
    let connects = Arc::new(AtomicUsize::new(0));
    let disconnects = Arc::new(AtomicUsize::new(0));
    let provider = ScriptedProvider {
        connect_results: VecDeque::from([Ok(()), Ok(())]),
        poll_results: VecDeque::from([
            Ok(vec![player("player-a", PlaybackStatus::Playing, "first")]),
            Err(MediaProviderError::new("session bus disappeared")),
            Ok(vec![player(
                "player-b",
                PlaybackStatus::Playing,
                "replacement",
            )]),
        ]),
        connects: Arc::clone(&connects),
        disconnects: Arc::clone(&disconnects),
    };
    let mut session = MediaProviderSession::new(Box::new(provider));

    assert_eq!(
        session
            .poll()
            .await
            .expect("initial poll works")
            .state
            .player,
        "player-a"
    );
    assert!(matches!(
        session.poll().await,
        Err(MediaProviderFailure::Poll(_))
    ));
    assert!(!session.is_connected());
    assert_eq!(
        session
            .poll()
            .await
            .expect("next poll reconnects")
            .state
            .player,
        "player-b"
    );
    assert_eq!(connects.load(Ordering::Relaxed), 2);
    assert_eq!(disconnects.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn provider_retries_an_initially_unavailable_session_bus() {
    let connects = Arc::new(AtomicUsize::new(0));
    let disconnects = Arc::new(AtomicUsize::new(0));
    let provider = ScriptedProvider {
        connect_results: VecDeque::from([
            Err(MediaProviderError::new("no desktop session bus")),
            Ok(()),
        ]),
        poll_results: VecDeque::from([Ok(vec![player(
            "player",
            PlaybackStatus::Paused,
            "recovered",
        )])]),
        connects: Arc::clone(&connects),
        disconnects: Arc::clone(&disconnects),
    };
    let mut session = MediaProviderSession::new(Box::new(provider));

    assert!(matches!(
        session.poll().await,
        Err(MediaProviderFailure::Connect(_))
    ));
    assert!(!session.is_connected());
    assert_eq!(
        session
            .poll()
            .await
            .expect("provider retries connection")
            .state
            .track,
        "recovered"
    );
    assert_eq!(connects.load(Ordering::Relaxed), 2);
    assert_eq!(disconnects.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn identical_same_app_sessions_keep_the_selected_artwork_identity() {
    let artwork = |session: &str| {
        ArtworkSource::windows_session_fixture(session, "browser", "same", "Artist", "Album")
    };
    let mut stopped = player("browser", PlaybackStatus::Stopped, "same");
    stopped.artwork = Some(artwork("stopped-session"));
    let mut playing = player("browser", PlaybackStatus::Playing, "same");
    playing.artwork = Some(artwork("playing-session"));
    let provider = ScriptedProvider {
        connect_results: VecDeque::new(),
        poll_results: VecDeque::from([Ok(vec![stopped, playing])]),
        connects: Arc::new(AtomicUsize::new(0)),
        disconnects: Arc::new(AtomicUsize::new(0)),
    };
    let mut session = MediaProviderSession::new(Box::new(provider));

    let selected = session
        .poll()
        .await
        .expect("poll succeeds")
        .artwork
        .expect("poll defers artwork")
        .source;

    assert_eq!(selected, artwork("playing-session"));
    assert_ne!(selected, artwork("stopped-session"));
}

#[tokio::test]
async fn slow_or_broken_artwork_never_delays_metadata_publication() {
    let slow_url = spawn_http_response(
        "Content-Length: 1\r\nConnection: close\r\n",
        vec![0],
        Duration::from_millis(200),
    );
    let mut snapshot = player("player", PlaybackStatus::Playing, "track");
    snapshot.artwork = Some(ArtworkSource::Url(slow_url.clone()));
    let provider = ScriptedProvider {
        connect_results: VecDeque::new(),
        poll_results: VecDeque::from([Ok(vec![snapshot])]),
        connects: Arc::new(AtomicUsize::new(0)),
        disconnects: Arc::new(AtomicUsize::new(0)),
    };
    let mut session = MediaProviderSession::new(Box::new(provider));

    let started = Instant::now();
    let metadata = session.poll().await.expect("metadata poll succeeds");
    assert!(started.elapsed() < Duration::from_millis(50));
    assert_eq!(metadata.state.track, "track");
    assert!(metadata.state.art_data_url.is_none());
    assert_eq!(
        metadata.artwork.expect("art is deferred").source,
        ArtworkSource::Url(slow_url.clone())
    );

    let mut policy = artwork_policy(64);
    policy.fetch_timeout = Duration::from_millis(20);
    let fetcher = ArtworkFetcher::new(policy).expect("test policy builds");
    assert_eq!(
        fetcher
            .fetch_data_url(&ArtworkSource::Url(slow_url))
            .await
            .expect_err("slow source is bounded independently"),
        ArtworkError::Timeout
    );
    assert_eq!(
        fetcher
            .fetch_data_url(&ArtworkSource::Url("invalid:art".to_owned()))
            .await
            .expect_err("broken source fails independently"),
        ArtworkError::UnsupportedSource
    );
}

#[test]
fn art_cache_caches_missing_art_without_fetching() {
    let mut cache = ArtCache::new();
    let snapshot = player("org.mpris.MediaPlayer2.mpv", PlaybackStatus::Playing, "a");

    let art = cache.resolve(&snapshot, |_| panic!("no art URL means no fetch"));
    assert!(art.is_none());

    // Failed/missing art is remembered for the track, not retried per poll.
    let art = cache.resolve(&snapshot, |_| panic!("cached miss should not refetch"));
    assert!(art.is_none());
}

#[test]
fn media_state_reflects_picked_player() {
    let snapshot = player(
        "org.mpris.MediaPlayer2.spotify",
        PlaybackStatus::Playing,
        "Song",
    );
    let state = media_state_from_player(Some(&snapshot), Some("data:art".to_owned()));

    assert!(state.available);
    assert!(state.playing);
    assert_eq!(state.track, "Song");
    assert_eq!(state.player, "org.mpris.MediaPlayer2.spotify");
    assert_eq!(state.art_data_url.as_deref(), Some("data:art"));
    assert_eq!(state.position_ms, 1_000);
    assert_eq!(state.duration_ms, 200_000);
}

#[test]
fn media_state_without_player_is_unavailable() {
    let state = media_state_from_player(None, None);

    assert!(!state.available);
    assert!(!state.playing);
    assert!(state.track.is_empty());
    assert!(state.art_data_url.is_none());
}

#[test]
fn paused_player_state_is_available_but_not_playing() {
    let snapshot = player("org.mpris.MediaPlayer2.mpv", PlaybackStatus::Paused, "Song");
    let state = media_state_from_player(Some(&snapshot), None);

    assert!(state.available);
    assert!(!state.playing);
}

#[test]
fn initial_unavailable_payload_is_not_a_successful_poll_or_live_status() {
    let mut source = MediaSource::new();
    let handle = source
        .source_status_handle()
        .expect("production media source exposes status");
    source.set_source_graph_generation(1);
    source
        .source_status_reporter()
        .expect("production media source exposes reporter")
        .begin_session()
        .expect("media status session starts");

    let first = source.sample().expect("sample should succeed");
    assert!(matches!(first, InputData::None));
    let second = source.sample().expect("sample should succeed");
    assert!(matches!(second, InputData::None));
    let status = handle.snapshot();
    assert_eq!(status.state, SourceState::Starting);
    assert_eq!(status.freshness, SourceFreshness::AwaitingSample);
}

#[test]
fn stopping_media_clears_existing_state_from_the_receiver() {
    let mut source = MediaSource::new();
    let publisher = source.publisher();
    let receiver = source.receiver();
    let snapshot = player(
        "org.mpris.MediaPlayer2.test",
        PlaybackStatus::Playing,
        "before-stop",
    );

    assert!(publisher.publish_completed(
        media_state_from_player(Some(&snapshot), None),
        Instant::now(),
    ));
    assert!(receiver.borrow().available);

    source.stop();

    assert!(!receiver.borrow().available);
}

#[test]
fn retained_media_publisher_cannot_publish_after_stop() {
    let mut source = MediaSource::new();
    let publisher = source.publisher();
    let receiver = source.receiver();
    source.stop();
    let snapshot = player(
        "org.mpris.MediaPlayer2.test",
        PlaybackStatus::Playing,
        "after-stop",
    );

    assert!(!publisher.publish_completed(
        media_state_from_player(Some(&snapshot), None),
        Instant::now(),
    ));
    assert!(!receiver.borrow().available);
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
#[test]
fn old_media_generation_cannot_overwrite_a_restarted_successor() {
    let mut source = MediaSource::new();
    let stale = source.publisher();
    let receiver = source.receiver();
    source.start().expect("media source should start");
    let successor = source.publisher();
    let current = player(
        "org.mpris.MediaPlayer2.test",
        PlaybackStatus::Playing,
        "successor",
    );
    let stale_state = player(
        "org.mpris.MediaPlayer2.test",
        PlaybackStatus::Playing,
        "stale-generation",
    );

    assert!(successor.publish_completed(
        media_state_from_player(Some(&current), None),
        Instant::now(),
    ));
    assert!(!stale.publish_completed(
        media_state_from_player(Some(&stale_state), None),
        Instant::now(),
    ));
    assert_ne!(receiver.borrow().track, "stale-generation");

    source.stop();
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
#[tokio::test]
async fn old_media_generation_cannot_enrich_the_same_track_after_restart() {
    let mut source = MediaSource::new();
    let stale = source.publisher();
    let receiver = source.receiver();
    let snapshot = player(
        "org.mpris.MediaPlayer2.test",
        PlaybackStatus::Playing,
        "same-track",
    );
    let state = media_state_from_player(Some(&snapshot), None);
    let key = state.track_key();
    assert!(stale.publish_completed(state.clone(), Instant::now()));

    source.start().expect("media source should start");
    assert!(
        source
            .publisher()
            .publish_completed(state.clone(), Instant::now())
    );

    let backend = Arc::new(ImmediateArtworkBackend {
        starts: AtomicUsize::new(0),
        released: AtomicBool::new(false),
    });
    let mut policy = artwork_policy(64);
    policy.fetch_timeout = Duration::from_secs(5);
    let fetcher = ArtworkFetcher::with_blocking_backend(policy, backend.clone())
        .expect("test backend builds");
    let (art_tx, art_rx) = tokio::sync::watch::channel(Some(Arc::new(ArtworkRequest {
        key,
        source: ArtworkSource::Url("file:///C:/stale-art".to_owned()),
    })));
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let worker = run_artwork_loop(fetcher, stale, art_rx, stop_rx);
    let current = source.publisher();
    let drive = async {
        wait_for_attempts(&backend.starts, 1).await;
        assert!(current.publish_completed(state.clone(), Instant::now()));
        backend.released.store(true, Ordering::SeqCst);
        let stale_art_landed = tokio::time::timeout(Duration::from_millis(250), async {
            loop {
                assert!(current.publish_completed(state.clone(), Instant::now()));
                if receiver.borrow().art_data_url.as_deref() == Some("data:image/jpeg;base64,stale")
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .is_ok();
        stop_tx.send_replace(true);
        stale_art_landed
    };

    let ((), stale_art_landed) = tokio::time::timeout(Duration::from_secs(4), async {
        tokio::join!(worker, drive)
    })
    .await
    .expect("stale artwork worker stops before deadline");

    drop(art_tx);
    assert_eq!(backend.starts.load(Ordering::SeqCst), 1);
    assert!(!stale_art_landed);
    assert!(receiver.borrow().art_data_url.is_none());
    source.stop();
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
#[test]
fn exited_media_poller_marks_the_source_restartable() {
    let mut source = MediaSource::new();
    source.start().expect("media source should start");

    source.report_poller_exit("injected worker exit");

    assert!(!source.is_running());
    source.start().expect("media source should restart");
    assert!(source.is_running());
    source.stop();
}

#[test]
fn stopped_media_source_samples_none_even_after_a_retained_write_attempt() {
    let mut source = MediaSource::new();
    let publisher = source.publisher();
    source.stop();
    let snapshot = player(
        "org.mpris.MediaPlayer2.test",
        PlaybackStatus::Playing,
        "never-sampled",
    );

    assert!(!publisher.publish_completed(
        media_state_from_player(Some(&snapshot), None),
        Instant::now(),
    ));
    assert!(matches!(
        source.sample().expect("stopped sample should succeed"),
        InputData::None
    ));
}

#[test]
fn stop_racing_media_publication_always_finishes_unavailable() {
    for round in 0..64 {
        let mut source = MediaSource::new();
        let publisher = source.publisher();
        let receiver = source.receiver();
        let barrier = Arc::new(Barrier::new(2));
        let snapshot = player(
            "org.mpris.MediaPlayer2.test",
            PlaybackStatus::Playing,
            &format!("racing-{round}"),
        );
        let state = media_state_from_player(Some(&snapshot), None);
        let completed_at = Instant::now();

        std::thread::scope(|scope| {
            let worker_barrier = Arc::clone(&barrier);
            let publisher = publisher.clone();
            let publish = scope.spawn(move || {
                worker_barrier.wait();
                publisher.publish_completed(state, completed_at)
            });
            barrier.wait();
            source.stop();
            publish
                .join()
                .expect("racing media publisher should finish");
        });

        assert!(!receiver.borrow().available, "failed in round {round}");
    }
}

#[test]
fn successful_media_poll_heartbeat_advances_when_payload_is_unchanged() {
    let mut source = MediaSource::new();
    let handle = source
        .source_status_handle()
        .expect("production media source exposes status");
    source.set_source_graph_generation(1);
    source
        .source_status_reporter()
        .expect("production media source exposes reporter")
        .begin_session()
        .expect("media status session starts");
    let publisher = source.publisher();
    let snapshot = player(
        "org.mpris.MediaPlayer2.test",
        PlaybackStatus::Playing,
        "heartbeat",
    );
    let state = media_state_from_player(Some(&snapshot), None);
    let first_completed_at = Instant::now();
    assert!(publisher.publish_completed(state.clone(), first_completed_at));
    std::thread::sleep(Duration::from_millis(20));
    assert!(matches!(
        source.sample().expect("first completed poll samples"),
        InputData::Media(_)
    ));
    assert_eq!(handle.snapshot().last_sample_at, Some(first_completed_at));

    let second_completed_at = Instant::now();
    assert!(publisher.publish_completed(state, second_completed_at));
    assert!(matches!(
        source.sample().expect("unchanged heartbeat samples"),
        InputData::None
    ));
    let refreshed = handle.snapshot();
    assert_eq!(refreshed.last_sample_at, Some(second_completed_at));
    assert_eq!(refreshed.freshness, SourceFreshness::Fresh);
    assert_eq!(
        handle
            .snapshot_at(second_completed_at + Duration::from_secs(2))
            .freshness,
        SourceFreshness::Stale
    );
}

#[test]
fn concurrent_media_publishers_keep_state_and_poll_coherent() {
    for round in 0..16 {
        let mut source = MediaSource::new();
        let publisher = source.publisher();
        let barrier = Arc::new(Barrier::new(9));
        std::thread::scope(|scope| {
            for worker in 0..8 {
                let publisher = publisher.clone();
                let barrier = Arc::clone(&barrier);
                scope.spawn(move || {
                    barrier.wait();
                    for update in 0..64 {
                        let snapshot = player(
                            &format!("publisher-{worker}"),
                            PlaybackStatus::Playing,
                            &format!("round-{round}-update-{update}"),
                        );
                        assert!(publisher.publish_completed(
                            media_state_from_player(Some(&snapshot), None),
                            Instant::now(),
                        ));
                    }
                });
            }
            barrier.wait();
        });

        let receiver = source.receiver();
        let published = receiver.borrow().clone();
        let InputData::Media(sampled) = source.sample().expect("latest poll should sample") else {
            panic!("concurrent publishers must leave one completed media poll");
        };
        assert!(Arc::ptr_eq(&published, &sampled));
    }
}

/// Lifecycle smoke test against the native provider. Asserts only on
/// mechanics (start/sample/stop never error); whether a player is found
/// depends on the environment, so the picked state is printed for manual
/// receipts and not asserted.
#[cfg(any(target_os = "linux", target_os = "windows"))]
#[test]
fn media_source_lifecycle_against_native_provider() {
    let mut source = MediaSource::new();
    let status_handle = source
        .source_status_handle()
        .expect("native media source exposes status");
    source.start().expect("media source should start");
    assert!(source.is_running());

    std::thread::sleep(std::time::Duration::from_millis(2500));

    let mut latest = None;
    for _ in 0..3 {
        match source.sample().expect("sample should succeed") {
            InputData::Media(state) => latest = Some(state),
            InputData::None => {}
            other => panic!("unexpected input data: {other:?}"),
        }
    }
    if let Some(state) = latest {
        println!(
            "live media state: available={} playing={} player={} track={} artist={} art={} position_ms={} duration_ms={}",
            state.available,
            state.playing,
            state.player,
            state.track,
            state.artist,
            state.art_data_url.as_ref().map_or(0, String::len),
            state.position_ms,
            state.duration_ms,
        );
    }

    let status = status_handle.snapshot();
    println!(
        "native media status: state={:?} issue={:?}",
        status.state,
        status.issue.as_ref().map(|issue| issue.code.as_ref())
    );
    assert_ne!(
        status.issue.as_ref().map(|issue| issue.code.as_ref()),
        Some("media_backend_unsupported")
    );

    source.stop();
    assert!(!source.is_running());
}
