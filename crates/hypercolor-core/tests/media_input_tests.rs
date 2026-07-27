use hypercolor_core::input::media::{
    ArtCache, ArtworkError, ArtworkFetcher, ArtworkPolicy, ArtworkSource, MediaMetadataProvider,
    MediaProviderError, MediaProviderFailure, MediaProviderSession, MediaSource, PlaybackStatus,
    PlayerSnapshot, media_state_from_player, pick_active_player,
};
use hypercolor_core::input::{InputData, InputSource};
use hypercolor_core::input::{SourceFreshness, SourceState};
use std::collections::VecDeque;
use std::io::{Cursor, Read as _, Write as _};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

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
    publisher.publish_completed(state.clone(), first_completed_at);
    std::thread::sleep(Duration::from_millis(20));
    assert!(matches!(
        source.sample().expect("first completed poll samples"),
        InputData::Media(_)
    ));
    assert_eq!(handle.snapshot().last_sample_at, Some(first_completed_at));

    let second_completed_at = Instant::now();
    publisher.publish_completed(state, second_completed_at);
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
                        publisher.publish_completed(
                            media_state_from_player(Some(&snapshot), None),
                            Instant::now(),
                        );
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
