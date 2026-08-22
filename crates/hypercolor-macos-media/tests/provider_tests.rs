use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use hypercolor_macos_media::{
    AdapterFailure, Artwork, AutomationBackend, Capability, DeferredArtworkLoader,
    DeferredArtworkSource, LoadedArtwork, MediaAdapter, MediaError, MediaErrorKind,
    MediaPlayerSnapshot, MediaPoll, MediaProvider, PlaybackStatus,
};

struct ScriptedBackend {
    capability: Capability,
    connects: VecDeque<Result<(), MediaError>>,
    authorizations: VecDeque<Result<(), MediaError>>,
    polls: VecDeque<Result<MediaPoll, MediaError>>,
    lifecycle: Arc<Mutex<Vec<&'static str>>>,
}

impl AutomationBackend for ScriptedBackend {
    fn capability(&self) -> Capability {
        self.capability.clone()
    }

    fn request_authorization(&mut self, _adapter: MediaAdapter) -> Result<(), MediaError> {
        self.lifecycle
            .lock()
            .expect("lifecycle lock is not poisoned")
            .push("authorize");
        self.authorizations.pop_front().unwrap_or(Ok(()))
    }

    fn connect(&mut self) -> Result<(), MediaError> {
        self.lifecycle
            .lock()
            .expect("lifecycle lock is not poisoned")
            .push("connect");
        self.connects.pop_front().unwrap_or(Ok(()))
    }

    fn poll(&mut self) -> Result<MediaPoll, MediaError> {
        self.lifecycle
            .lock()
            .expect("lifecycle lock is not poisoned")
            .push("poll");
        self.polls
            .pop_front()
            .expect("fixture provides every expected poll")
    }

    fn disconnect(&mut self) {
        self.lifecycle
            .lock()
            .expect("lifecycle lock is not poisoned")
            .push("disconnect");
    }
}

fn player(track_id: &str, track: &str, artwork: Artwork) -> MediaPlayerSnapshot {
    MediaPlayerSnapshot {
        player_id: MediaAdapter::Music.bundle_id().to_owned(),
        track_id: track_id.to_owned(),
        status: PlaybackStatus::Playing,
        track: track.to_owned(),
        artist: "Artist".to_owned(),
        album: "Album".to_owned(),
        artwork: Some(artwork),
        position_ms: 1_000,
        duration_ms: 180_000,
    }
}

fn provider(
    capability: Capability,
    connects: impl IntoIterator<Item = Result<(), MediaError>>,
    authorizations: impl IntoIterator<Item = Result<(), MediaError>>,
    polls: impl IntoIterator<Item = Result<MediaPoll, MediaError>>,
) -> (MediaProvider, Arc<Mutex<Vec<&'static str>>>) {
    let lifecycle = Arc::new(Mutex::new(Vec::new()));
    let backend = ScriptedBackend {
        capability,
        connects: connects.into_iter().collect(),
        authorizations: authorizations.into_iter().collect(),
        polls: polls.into_iter().collect(),
        lifecycle: Arc::clone(&lifecycle),
    };
    (MediaProvider::with_backend(Box::new(backend)), lifecycle)
}

struct FixtureArtworkLoader;

impl DeferredArtworkLoader for FixtureArtworkLoader {
    fn load(&self, max_bytes: usize) -> Result<Option<LoadedArtwork>, MediaError> {
        if max_bytes < 3 {
            return Err(MediaError::new(
                MediaErrorKind::AdapterFailure,
                Some(MediaAdapter::Music),
                "fixture artwork exceeds policy",
            ));
        }
        Ok(Some(LoadedArtwork::Bytes {
            identity: "fixture-art".to_owned(),
            data: Arc::from([1_u8, 2, 3]),
        }))
    }
}

#[test]
fn deferred_artwork_has_stable_identity_and_loads_on_demand() {
    let source =
        DeferredArtworkSource::with_loader("music\u{1f}track", Arc::new(FixtureArtworkLoader));

    assert_eq!(source.identity(), "music\u{1f}track");
    assert!(matches!(
        source.load(3).expect("fixture artwork loads"),
        Some(LoadedArtwork::Bytes { data, .. }) if data.as_ref() == [1, 2, 3]
    ));
    assert_eq!(Artwork::Deferred(source.clone()), Artwork::Deferred(source));
}

#[test]
fn connected_provider_replaces_metadata_and_artwork() {
    let first = player(
        "track-a",
        "First",
        Artwork::Bytes {
            identity: "art-a".to_owned(),
            data: Arc::from([1_u8, 2, 3]),
        },
    );
    let second = player(
        "track-b",
        "Second",
        Artwork::Url("https://example.invalid/second.jpg".to_owned()),
    );
    let (mut provider, lifecycle) = provider(
        Capability::Available,
        [Ok(())],
        [],
        [
            Ok(MediaPoll::Players {
                players: vec![first.clone()],
                failures: Vec::new(),
            }),
            Ok(MediaPoll::Players {
                players: vec![second.clone()],
                failures: Vec::new(),
            }),
        ],
    );

    provider.connect().expect("fixture connects");
    let MediaPoll::Players { players, .. } = provider.poll_players().expect("first poll succeeds")
    else {
        panic!("fixture returns players");
    };
    assert_eq!(players, vec![first]);
    let MediaPoll::Players { players, .. } = provider.poll_players().expect("second poll succeeds")
    else {
        panic!("fixture returns players");
    };
    assert_eq!(players, vec![second]);
    provider.disconnect();
    assert_eq!(
        *lifecycle.lock().expect("lifecycle lock is not poisoned"),
        ["connect", "poll", "poll", "disconnect"]
    );
}

#[test]
fn disconnected_provider_rejects_polling() {
    let (mut provider, _) = provider(Capability::Available, [], [], []);
    let error = provider
        .poll_players()
        .expect_err("disconnected provider rejects a poll");
    assert_eq!(error.kind(), MediaErrorKind::Disconnected);
}

#[test]
fn unsupported_capability_remains_distinct() {
    let unsupported = MediaError::new(
        MediaErrorKind::UnsupportedCapability,
        None,
        "responsible process is not an eligible app sidecar",
    );
    let (mut provider, _) = provider(
        Capability::IneligibleResponsibleBundle,
        [Err(unsupported)],
        [],
        [],
    );
    assert_eq!(
        provider.capability(),
        Capability::IneligibleResponsibleBundle
    );
    assert_eq!(
        provider
            .connect()
            .expect_err("fixture is unsupported")
            .kind(),
        MediaErrorKind::UnsupportedCapability
    );
}

#[test]
fn stale_target_recovers_after_session_reconnect() {
    let stale = MediaError::new(
        MediaErrorKind::StaleTarget,
        Some(MediaAdapter::Music),
        "Music terminated",
    );
    let recovered = player(
        "track-recovered",
        "Recovered",
        Artwork::Url("https://example.invalid/recovered.jpg".to_owned()),
    );
    let (mut provider, lifecycle) = provider(
        Capability::Available,
        [Ok(()), Ok(())],
        [],
        [
            Err(stale),
            Ok(MediaPoll::Players {
                players: vec![recovered.clone()],
                failures: Vec::new(),
            }),
        ],
    );

    provider.connect().expect("initial connection succeeds");
    assert_eq!(
        provider
            .poll_players()
            .expect_err("target goes stale")
            .kind(),
        MediaErrorKind::StaleTarget
    );
    provider.disconnect();
    provider.connect().expect("replacement connection succeeds");
    let MediaPoll::Players { players, .. } = provider.poll_players().expect("provider recovers")
    else {
        panic!("fixture returns players");
    };
    assert_eq!(players, vec![recovered]);
    assert_eq!(
        *lifecycle.lock().expect("lifecycle lock is not poisoned"),
        ["connect", "poll", "disconnect", "connect", "poll"]
    );
}

#[test]
fn no_running_player_and_adapter_failures_are_not_collapsed() {
    let spotify_failure = MediaError::new(
        MediaErrorKind::AuthorizationDenied,
        Some(MediaAdapter::Spotify),
        "Spotify permission denied",
    );
    let music_failure = MediaError::new(
        MediaErrorKind::AuthorizationRequired,
        Some(MediaAdapter::Music),
        "Music permission requires consent",
    );
    let (mut provider, _) = provider(
        Capability::Available,
        [Ok(())],
        [],
        [
            Ok(MediaPoll::NoRunningCapablePlayer),
            Ok(MediaPoll::Players {
                players: Vec::new(),
                failures: vec![
                    AdapterFailure {
                        adapter: MediaAdapter::Music,
                        error: music_failure,
                    },
                    AdapterFailure {
                        adapter: MediaAdapter::Spotify,
                        error: spotify_failure,
                    },
                ],
            }),
        ],
    );

    provider.connect().expect("fixture connects");
    assert_eq!(
        provider
            .poll_players()
            .expect("absence is a successful poll"),
        MediaPoll::NoRunningCapablePlayer
    );
    let MediaPoll::Players { failures, .. } =
        provider.poll_players().expect("failures are retained")
    else {
        panic!("fixture returns adapter failures");
    };
    assert_eq!(failures.len(), 2);
    assert_eq!(
        failures[0].error.kind(),
        MediaErrorKind::AuthorizationRequired
    );
    assert_eq!(
        failures[1].error.kind(),
        MediaErrorKind::AuthorizationDenied
    );
}

#[test]
fn explicit_authorization_action_is_separate_from_polling() {
    let (mut provider, lifecycle) = provider(Capability::Available, [], [Ok(())], []);
    provider
        .request_authorization(MediaAdapter::Music)
        .expect("explicit authorization succeeds");
    assert_eq!(
        *lifecycle.lock().expect("lifecycle lock is not poisoned"),
        ["authorize"]
    );
}

#[cfg(not(target_os = "macos"))]
#[test]
fn native_stub_reports_unsupported_without_connecting() {
    let mut provider = MediaProvider::new();
    assert_eq!(provider.capability(), Capability::UnsupportedPlatform);
    assert_eq!(
        provider.connect().expect_err("stub cannot connect").kind(),
        MediaErrorKind::UnsupportedCapability
    );
}

#[cfg(target_os = "macos")]
#[test]
fn unbundled_test_process_is_not_automation_eligible() {
    let mut provider = MediaProvider::new();
    assert_eq!(
        provider.capability(),
        Capability::IneligibleResponsibleBundle
    );
    assert_eq!(
        provider
            .connect()
            .expect_err("test binary is not an app sidecar")
            .kind(),
        MediaErrorKind::UnsupportedCapability
    );
}
