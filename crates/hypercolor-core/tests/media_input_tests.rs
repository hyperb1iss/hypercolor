use hypercolor_core::input::media::{
    ArtCache, MediaSource, PlaybackStatus, PlayerSnapshot, media_state_from_player,
    pick_active_player,
};
use hypercolor_core::input::{InputData, InputSource};
use hypercolor_core::input::{SourceFreshness, SourceState};
use std::time::{Duration, Instant};

fn player(bus_name: &str, status: PlaybackStatus, track: &str) -> PlayerSnapshot {
    PlayerSnapshot {
        bus_name: bus_name.to_owned(),
        status,
        track: track.to_owned(),
        artist: "Artist".to_owned(),
        album: "Album".to_owned(),
        art_url: None,
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
    snapshot.art_url = Some("file:///art/a.jpg".to_owned());

    let mut fetches = 0;
    let art = cache.resolve(&snapshot, |url| {
        fetches += 1;
        Some(format!("data:{url}"))
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
    snapshot.art_url = Some("file:///art/b.jpg".to_owned());
    let art = cache.resolve(&snapshot, |url| {
        fetches += 1;
        Some(format!("data:{url}"))
    });
    assert_eq!(art.as_deref(), Some("data:file:///art/b.jpg"));
    assert_eq!(fetches, 2);
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

/// Lifecycle smoke test against the real session bus. Asserts only on
/// mechanics (start/sample/stop never error); whether a player is found
/// depends on the environment, so the picked state is printed for manual
/// receipts and not asserted.
#[cfg(target_os = "linux")]
#[test]
fn media_source_lifecycle_against_real_bus() {
    let mut source = MediaSource::new();
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

    source.stop();
    assert!(!source.is_running());
}
