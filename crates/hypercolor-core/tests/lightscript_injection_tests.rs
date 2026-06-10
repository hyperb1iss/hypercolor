//! Spec 69 W3.1 — media/net/lighting LightScript injection.
//!
//! Covers payload gating (include flags from metadata opt-ins), change
//! detection, album-art track gating, and the bootstrap defaults faces read
//! before the first payload arrives.

use std::collections::HashMap;

use hypercolor_core::effect::{
    FrameDataSources, FrameInput, LightScriptFrameUpdateOptions, LightscriptRuntime,
    parse_html_effect_metadata,
};
use hypercolor_core::input::InteractionData;
use hypercolor_types::audio::AudioData;
use hypercolor_types::lighting::LightingState;
use hypercolor_types::media::MediaState;
use hypercolor_types::net::NetStats;
use hypercolor_types::sensor::SystemSnapshot;

fn options_with(
    include_media: bool,
    include_net: bool,
    include_lighting: bool,
) -> LightScriptFrameUpdateOptions<'static> {
    LightScriptFrameUpdateOptions {
        include_audio: false,
        include_screen: false,
        include_sensors: false,
        include_interaction: false,
        include_media,
        include_net,
        include_lighting,
        render_host_frame: false,
        selected_sensor_labels: None,
    }
}

struct Fixture {
    audio: AudioData,
    interaction: InteractionData,
    sensors: SystemSnapshot,
}

impl Fixture {
    fn new() -> Self {
        Self {
            audio: AudioData::silence(),
            interaction: InteractionData::default(),
            sensors: SystemSnapshot::empty(),
        }
    }

    fn input<'a>(&'a self, sources: FrameDataSources<'a>) -> FrameInput<'a> {
        FrameInput {
            time_secs: 1.0,
            delta_secs: 1.0 / 30.0,
            frame_number: 7,
            audio: &self.audio,
            interaction: &self.interaction,
            screen: None,
            sensors: &self.sensors,
            sources,
            canvas_width: 480,
            canvas_height: 480,
        }
    }
}

fn playing_state() -> MediaState {
    MediaState {
        available: true,
        playing: true,
        track: "SANCTUARY".to_owned(),
        artist: "TIGEREYES".to_owned(),
        album: "SANCTUARY".to_owned(),
        art_data_url: Some("data:image/jpeg;base64,QQ==".to_owned()),
        position_ms: 1000,
        duration_ms: 200_000,
        player: "org.mpris.MediaPlayer2.spotify".to_owned(),
    }
}

fn payload_json(
    runtime: &mut LightscriptRuntime,
    fixture: &Fixture,
    sources: FrameDataSources<'_>,
    options: LightScriptFrameUpdateOptions<'_>,
) -> Option<serde_json::Value> {
    runtime
        .frame_payload_json(&fixture.input(sources), &HashMap::new(), options)
        .map(|json| serde_json::from_str(&json).expect("payload should be valid JSON"))
}

#[test]
fn media_payload_requires_opt_in() {
    let mut runtime = LightscriptRuntime::new(480, 480);
    let fixture = Fixture::new();
    let media = playing_state();
    let sources = FrameDataSources {
        media: Some(&media),
        ..FrameDataSources::default()
    };

    assert!(
        payload_json(
            &mut runtime,
            &fixture,
            sources,
            options_with(false, false, false)
        )
        .is_none()
    );

    let payload = payload_json(
        &mut runtime,
        &fixture,
        sources,
        options_with(true, false, false),
    )
    .expect("opted-in media should emit");
    assert_eq!(payload["media"]["track"], "SANCTUARY");
    assert_eq!(payload["media"]["playing"], true);
}

#[test]
fn media_art_rides_along_only_on_track_change() {
    let mut runtime = LightscriptRuntime::new(480, 480);
    let fixture = Fixture::new();
    let options = options_with(true, false, false);

    let first = playing_state();
    let sources = FrameDataSources {
        media: Some(&first),
        ..FrameDataSources::default()
    };
    let payload = payload_json(&mut runtime, &fixture, sources, options)
        .expect("first media state should emit");
    assert_eq!(
        payload["media"]["artDataUrl"],
        "data:image/jpeg;base64,QQ=="
    );

    // Same state again: no payload at all.
    assert!(payload_json(&mut runtime, &fixture, sources, options).is_none());

    // Position-only change: payload without the art key.
    let mut progressed = first.clone();
    progressed.position_ms = 2000;
    let sources = FrameDataSources {
        media: Some(&progressed),
        ..FrameDataSources::default()
    };
    let payload = payload_json(&mut runtime, &fixture, sources, options)
        .expect("position change should emit");
    assert_eq!(payload["media"]["positionMs"], 2000);
    assert!(
        payload["media"].get("artDataUrl").is_none(),
        "art must not be re-sent while the track is unchanged"
    );

    // Track change without artwork: art key present and null to clear stale art.
    let mut next_track = first.clone();
    next_track.track = "NEON RAIN".to_owned();
    next_track.art_data_url = None;
    let sources = FrameDataSources {
        media: Some(&next_track),
        ..FrameDataSources::default()
    };
    let payload =
        payload_json(&mut runtime, &fixture, sources, options).expect("track change should emit");
    assert_eq!(payload["media"]["track"], "NEON RAIN");
    assert!(payload["media"]["artDataUrl"].is_null());
}

#[test]
fn net_payload_emits_only_on_change() {
    let mut runtime = LightscriptRuntime::new(480, 480);
    let fixture = Fixture::new();
    let options = options_with(false, true, false);

    let stats = NetStats {
        rx_bps: 1_250_000,
        tx_bps: 64_000,
        iface: "enp5s0".to_owned(),
    };
    let sources = FrameDataSources {
        net: Some(&stats),
        ..FrameDataSources::default()
    };
    let payload =
        payload_json(&mut runtime, &fixture, sources, options).expect("first net should emit");
    assert_eq!(payload["net"]["rxBps"], 1_250_000);
    assert_eq!(payload["net"]["iface"], "enp5s0");

    assert!(payload_json(&mut runtime, &fixture, sources, options).is_none());

    let refreshed = NetStats {
        rx_bps: 900_000,
        ..stats.clone()
    };
    let sources = FrameDataSources {
        net: Some(&refreshed),
        ..FrameDataSources::default()
    };
    let payload =
        payload_json(&mut runtime, &fixture, sources, options).expect("refreshed net should emit");
    assert_eq!(payload["net"]["rxBps"], 900_000);
}

#[test]
fn lighting_payload_serializes_hex_colors_and_gates_on_change() {
    let mut runtime = LightscriptRuntime::new(480, 480);
    let fixture = Fixture::new();
    let options = options_with(false, false, true);

    let lighting = LightingState {
        scene_name: Some("Studio".to_owned()),
        effect_names: vec!["Neon Clock".to_owned()],
        dominant_colors: vec![[225, 53, 255], [128, 255, 234]],
    };
    let sources = FrameDataSources {
        lighting: Some(&lighting),
        ..FrameDataSources::default()
    };
    let payload =
        payload_json(&mut runtime, &fixture, sources, options).expect("lighting should emit");
    assert_eq!(payload["lighting"]["sceneName"], "Studio");
    assert_eq!(
        payload["lighting"]["dominantColors"],
        serde_json::json!(["#e135ff", "#80ffea"])
    );
    assert_eq!(
        payload["lighting"]["effectNames"],
        serde_json::json!(["Neon Clock"])
    );

    assert!(payload_json(&mut runtime, &fixture, sources, options).is_none());
}

#[test]
fn absent_sources_emit_nothing_even_when_opted_in() {
    let mut runtime = LightscriptRuntime::new(480, 480);
    let fixture = Fixture::new();

    assert!(
        payload_json(
            &mut runtime,
            &fixture,
            FrameDataSources::default(),
            options_with(true, true, true),
        )
        .is_none()
    );
}

#[test]
fn bootstrap_script_seeds_data_source_defaults() {
    let runtime = LightscriptRuntime::new(480, 480);
    let script = runtime.bootstrap_script();

    assert!(script.contains("window.engine.media = { available: false"));
    assert!(script.contains("window.engine.net = { rxBps: 0"));
    assert!(script.contains("window.engine.lighting = { sceneName: null"));
}

#[test]
fn data_sources_meta_tag_maps_to_tags() {
    let html = r#"<html><head>
        <title>Now Playing</title>
        <meta data-sources="media, net" />
    </head><body></body></html>"#;

    let metadata = parse_html_effect_metadata(html);
    assert!(metadata.tags.iter().any(|tag| tag == "media"));
    assert!(metadata.tags.iter().any(|tag| tag == "net"));
    assert!(!metadata.tags.iter().any(|tag| tag == "lighting"));
}

#[test]
fn data_sources_meta_tag_drops_unknown_tokens() {
    let html = r#"<html><head>
        <title>Face</title>
        <meta data-sources="media, telemetry, LIGHTING" />
    </head><body></body></html>"#;

    let metadata = parse_html_effect_metadata(html);
    assert!(metadata.tags.iter().any(|tag| tag == "media"));
    assert!(metadata.tags.iter().any(|tag| tag == "lighting"));
    assert!(!metadata.tags.iter().any(|tag| tag == "telemetry"));
}
