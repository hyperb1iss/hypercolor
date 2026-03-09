//! Tests for configuration types — defaults, serde roundtrips, partial deserialization.

use hypercolor_types::config::{
    AudioConfig, CaptureConfig, DaemonConfig, DbusConfig, DiscoveryConfig, EffectEngineConfig,
    FeatureFlags, HypercolorConfig, LogLevel, McpConfig, ShutdownBehavior, TuiConfig, WebConfig,
    WledConfig, WledProtocolConfig,
};
use hypercolor_types::session::SessionConfig;

// ─── Default Value Tests ─────────────────────────────────────────────────────

#[test]
fn daemon_defaults_match_spec() {
    let d = DaemonConfig::default();
    assert_eq!(d.listen_address, "127.0.0.1");
    assert_eq!(d.port, 9420);
    assert!(d.unix_socket);
    assert_eq!(d.target_fps, 30);
    assert_eq!(d.canvas_width, 320);
    assert_eq!(d.canvas_height, 200);
    assert_eq!(d.max_devices, 32);
    assert_eq!(d.log_level, LogLevel::Info);
    assert_eq!(d.log_file, "");
    assert_eq!(d.start_profile, "last");
    assert_eq!(d.shutdown_behavior, ShutdownBehavior::HardwareDefault);
    assert_eq!(d.shutdown_color, "#1a1a2e");
}

#[test]
fn web_defaults_match_spec() {
    let w = WebConfig::default();
    assert!(w.enabled);
    assert!(!w.open_browser);
    assert!(w.cors_origins.is_empty());
    assert_eq!(w.websocket_fps, 30);
    assert!(!w.auth_enabled);
}

#[test]
fn mcp_defaults_match_spec() {
    let m = McpConfig::default();
    assert!(!m.enabled);
    assert_eq!(m.base_path, "/mcp");
    assert!(m.stateful_mode);
    assert!(!m.json_response);
    assert_eq!(m.sse_keep_alive_secs, 15);
}

#[test]
fn effect_engine_defaults_match_spec() {
    let e = EffectEngineConfig::default();
    assert_eq!(e.preferred_renderer, "auto");
    assert!(e.servo_enabled);
    assert_eq!(e.wgpu_backend, "auto");
    assert!(e.extra_effect_dirs.is_empty());
    assert!(e.watch_effects);
    assert!(e.watch_config);
}

#[test]
fn audio_defaults_match_spec() {
    let a = AudioConfig::default();
    assert!(a.enabled);
    assert_eq!(a.device, "default");
    assert_eq!(a.fft_size, 1024);
    assert!((a.smoothing - 0.8).abs() < f32::EPSILON);
    assert!((a.noise_gate - 0.02).abs() < f32::EPSILON);
    assert!((a.beat_sensitivity - 0.6).abs() < f32::EPSILON);
}

#[test]
fn capture_defaults_match_spec() {
    let c = CaptureConfig::default();
    assert!(!c.enabled);
    assert_eq!(c.source, "auto");
    assert_eq!(c.capture_fps, 30);
    assert_eq!(c.monitor, 0);
}

#[test]
fn discovery_defaults_match_spec() {
    let d = DiscoveryConfig::default();
    assert!(d.mdns_enabled);
    assert_eq!(d.scan_interval_secs, 300);
    assert!(d.wled_scan);
    assert!(d.hue_scan);
}

#[test]
fn wled_defaults_match_spec() {
    let w = WledConfig::default();
    assert!(w.known_ips.is_empty());
    assert_eq!(w.default_protocol, WledProtocolConfig::Ddp);
    assert!(w.realtime_http_enabled);
    assert_eq!(w.dedup_threshold, 2);
}

#[test]
fn dbus_defaults_match_spec() {
    let d = DbusConfig::default();
    assert!(d.enabled);
    assert_eq!(d.bus_name, "tech.hyperbliss.hypercolor1");
}

#[test]
fn tui_defaults_match_spec() {
    let t = TuiConfig::default();
    assert_eq!(t.theme, "silkcircuit");
    assert_eq!(t.preview_fps, 15);
    assert_eq!(t.keybindings, "default");
}

#[test]
fn feature_flags_all_false_by_default() {
    let f = FeatureFlags::default();
    assert!(!f.wasm_plugins);
    assert!(!f.hue_entertainment);
    assert!(!f.midi_input);
}

// ─── TOML Roundtrip Tests ────────────────────────────────────────────────────

#[test]
fn daemon_config_toml_roundtrip() {
    let original = DaemonConfig::default();
    let toml_str = toml::to_string(&original).expect("serialize DaemonConfig");
    let restored: DaemonConfig = toml::from_str(&toml_str).expect("deserialize DaemonConfig");
    assert_eq!(restored.port, original.port);
    assert_eq!(restored.target_fps, original.target_fps);
    assert_eq!(restored.canvas_width, original.canvas_width);
    assert_eq!(restored.log_level, original.log_level);
    assert_eq!(restored.shutdown_behavior, original.shutdown_behavior);
}

#[test]
fn web_config_toml_roundtrip() {
    let original = WebConfig::default();
    let toml_str = toml::to_string(&original).expect("serialize WebConfig");
    let restored: WebConfig = toml::from_str(&toml_str).expect("deserialize WebConfig");
    assert_eq!(restored.enabled, original.enabled);
    assert_eq!(restored.websocket_fps, original.websocket_fps);
}

#[test]
fn audio_config_toml_roundtrip() {
    let original = AudioConfig::default();
    let toml_str = toml::to_string(&original).expect("serialize AudioConfig");
    let restored: AudioConfig = toml::from_str(&toml_str).expect("deserialize AudioConfig");
    assert_eq!(restored.fft_size, original.fft_size);
    assert!((restored.smoothing - original.smoothing).abs() < f32::EPSILON);
    assert!((restored.beat_sensitivity - original.beat_sensitivity).abs() < f32::EPSILON);
}

#[test]
fn full_config_toml_roundtrip() {
    let original = HypercolorConfig {
        schema_version: 2,
        include: vec!["local.toml".into()],
        daemon: DaemonConfig::default(),
        web: WebConfig::default(),
        mcp: McpConfig::default(),
        effect_engine: EffectEngineConfig::default(),
        audio: AudioConfig::default(),
        capture: CaptureConfig::default(),
        discovery: DiscoveryConfig::default(),
        wled: WledConfig::default(),
        dbus: DbusConfig::default(),
        tui: TuiConfig::default(),
        features: FeatureFlags::default(),
        session: SessionConfig::default(),
    };
    let toml_str = toml::to_string(&original).expect("serialize HypercolorConfig");
    let restored: HypercolorConfig =
        toml::from_str(&toml_str).expect("deserialize HypercolorConfig");
    assert_eq!(restored.schema_version, 2);
    assert_eq!(restored.include, vec!["local.toml"]);
    assert_eq!(restored.daemon.port, 9420);
    assert!(restored.web.enabled);
    assert_eq!(restored.mcp.base_path, "/mcp");
    assert_eq!(restored.audio.fft_size, 1024);
    assert!(!restored.capture.enabled);
    assert_eq!(restored.discovery.scan_interval_secs, 300);
    assert!(restored.dbus.enabled);
    assert_eq!(restored.tui.theme, "silkcircuit");
    assert!(!restored.features.wasm_plugins);
}

// ─── Partial Deserialization (forward compatibility) ─────────────────────────

#[test]
fn minimal_toml_fills_defaults() {
    let minimal = "schema_version = 2\n";
    let config: HypercolorConfig = toml::from_str(minimal).expect("deserialize minimal config");
    assert_eq!(config.schema_version, 2);
    assert_eq!(config.daemon.port, 9420);
    assert!(config.web.enabled);
    assert_eq!(config.mcp.base_path, "/mcp");
    assert_eq!(config.audio.device, "default");
    assert!(!config.capture.enabled);
    assert_eq!(config.tui.theme, "silkcircuit");
    assert_eq!(config.wled.default_protocol, WledProtocolConfig::Ddp);
    assert!(config.wled.realtime_http_enabled);
    assert_eq!(config.wled.dedup_threshold, 2);
}

#[test]
fn unknown_fields_ignored() {
    let toml_with_future_field = r#"
schema_version = 3

[daemon]
port = 9420
some_future_field = "hello from the future"
"#;
    let config: HypercolorConfig =
        toml::from_str(toml_with_future_field).expect("deserialize with unknown fields");
    assert_eq!(config.schema_version, 3);
    assert_eq!(config.daemon.port, 9420);
}

#[test]
fn override_specific_defaults() {
    let partial = r#"
schema_version = 2

[daemon]
port = 8080
target_fps = 120

[audio]
enabled = false
fft_size = 2048

[wled]
default_protocol = "e131"
known_ips = ["192.168.1.50"]
realtime_http_enabled = false
dedup_threshold = 0
"#;
    let config: HypercolorConfig = toml::from_str(partial).expect("deserialize partial config");
    assert_eq!(config.daemon.port, 8080);
    assert_eq!(config.daemon.target_fps, 120);
    // Non-overridden fields keep defaults
    assert_eq!(config.daemon.canvas_width, 320);
    assert_eq!(config.daemon.listen_address, "127.0.0.1");
    assert!(!config.audio.enabled);
    assert_eq!(config.audio.fft_size, 2048);
    // Audio fields not overridden keep defaults
    assert!((config.audio.smoothing - 0.8).abs() < f32::EPSILON);
    assert_eq!(config.wled.default_protocol, WledProtocolConfig::E131);
    assert_eq!(config.wled.known_ips.len(), 1);
    assert!(!config.wled.realtime_http_enabled);
    assert_eq!(config.wled.dedup_threshold, 0);
}

// ─── Enum Serialization ─────────────────────────────────────────────────────

#[test]
fn log_level_serializes_snake_case() {
    // TOML can't serialize bare enum values; use JSON to verify snake_case naming.
    let json = serde_json::to_string(&LogLevel::Info).expect("serialize LogLevel");
    assert_eq!(json, "\"info\"");

    let json = serde_json::to_string(&LogLevel::Trace).expect("serialize LogLevel::Trace");
    assert_eq!(json, "\"trace\"");
}

#[test]
fn shutdown_behavior_roundtrip() {
    // Roundtrip through JSON since TOML requires a table at the top level.
    for (variant, expected_str) in [
        (ShutdownBehavior::HardwareDefault, "\"hardware_default\""),
        (ShutdownBehavior::Off, "\"off\""),
        (ShutdownBehavior::Static, "\"static\""),
    ] {
        let json = serde_json::to_string(&variant).expect("serialize ShutdownBehavior");
        assert_eq!(json, expected_str);
        let restored: ShutdownBehavior =
            serde_json::from_str(&json).expect("deserialize ShutdownBehavior");
        assert_eq!(restored, variant);
    }
}

#[test]
fn log_level_in_daemon_config_toml_roundtrip() {
    // Verify enums survive a TOML roundtrip inside their parent struct.
    let config = r#"
log_level = "warn"
shutdown_behavior = "off"
"#;
    let daemon: DaemonConfig = toml::from_str(config).expect("deserialize DaemonConfig");
    assert_eq!(daemon.log_level, LogLevel::Warn);
    assert_eq!(daemon.shutdown_behavior, ShutdownBehavior::Off);

    let reserialized = toml::to_string(&daemon).expect("reserialize DaemonConfig");
    let restored: DaemonConfig = toml::from_str(&reserialized).expect("re-deserialize");
    assert_eq!(restored.log_level, LogLevel::Warn);
    assert_eq!(restored.shutdown_behavior, ShutdownBehavior::Off);
}
