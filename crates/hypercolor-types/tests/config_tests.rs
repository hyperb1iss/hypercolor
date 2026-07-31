//! Tests for configuration types — defaults, serde roundtrips, partial deserialization.

use hypercolor_types::config::{
    AudioConfig, CaptureConfig, CaptureConfigValidationError, CapturePlatform, DaemonConfig,
    DbusConfig, DiscoveryConfig, DisplayConfig, EffectEngineConfig, EffectErrorFallbackPolicy,
    FeatureFlags, GoveeConfig, HypercolorConfig, InputConfig, InteractionRoutePolicy, LogLevel,
    McpConfig, MediaConfig, NetworkAccessMode, NetworkClientScope, NetworkConfig,
    RenderAccelerationMode, RenderingConfig, ServoGpuImportConfig, ServoGpuImportMode,
    ShutdownBehavior, TuiConfig, WebConfig, default_driver_configs,
};
use hypercolor_types::session::{OffOutputBehavior, SessionConfig};

// ─── Default Value Tests ─────────────────────────────────────────────────────

#[test]
fn daemon_defaults_match_spec() {
    let d = DaemonConfig::default();
    assert_eq!(d.listen_address, "127.0.0.1");
    assert_eq!(d.port, 9420);
    assert!(d.unix_socket);
    assert_eq!(d.target_fps, 30);
    assert_eq!(d.canvas_width, 640);
    assert_eq!(d.canvas_height, 480);
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
    assert_eq!(w.interactive_preview_resource_bytes, 1024 * 1024 * 1024);
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
    assert_eq!(e.compositor_acceleration_mode, RenderAccelerationMode::Auto);
    assert_eq!(e.effect_error_fallback, EffectErrorFallbackPolicy::None);
    assert!(e.extra_effect_dirs.is_empty());
    assert!(e.watch_effects);
    assert!(e.watch_config);
}

#[test]
fn rendering_defaults_match_spec() {
    let rendering = RenderingConfig::default();
    assert_eq!(rendering.servo_gpu_import.mode, ServoGpuImportMode::Auto);
}

#[test]
fn media_defaults_match_spec() {
    let media = MediaConfig::default();
    assert_eq!(media.max_video_producers, 2);
    assert_eq!(media.max_livestream_producers, 1);
    assert!(media.stream_private_network_allowlist.is_empty());
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

/// Screen capture is allowed by default only where turning it on cannot
/// ambush the user. Windows Desktop Duplication has no permission prompt and
/// no picker; the XDG portal and macOS TCC both do, so those stay opt-in.
#[test]
fn capture_is_enabled_by_default_only_where_it_needs_no_consent() {
    let c = CaptureConfig::default();
    assert_eq!(c.enabled, cfg!(target_os = "windows"));
}

#[test]
fn capture_defaults_match_spec() {
    let c = CaptureConfig::default();
    assert_eq!(c.source, "auto");
    assert_eq!(c.capture_fps, 30);
    assert_eq!(c.grid_cols, 8);
    assert_eq!(c.grid_rows, 6);
    assert_eq!(c.publication_memory_bytes, None);
    assert!((c.smoothing - 0.3).abs() < f32::EPSILON);
    assert!((c.scene_cut_threshold - 100.0).abs() < f32::EPSILON);
    // Off by default: desktops are not letterboxed, and dark desktop content
    // trips the detector into cropping real picture away.
    assert!(!c.letterbox);
    assert!((c.letterbox_threshold - 0.02).abs() < f32::EPSILON);
    assert!((c.saturation - 1.0).abs() < f32::EPSILON);
    assert!((c.brightness - 1.0).abs() < f32::EPSILON);
    assert!((c.gamma - 1.0).abs() < f32::EPSILON);
    assert_eq!(c.restore_token, None);
}

#[test]
fn capture_config_tolerates_legacy_monitor_key() {
    let parsed: CaptureConfig =
        toml::from_str("enabled = true\nmonitor = 2\n").expect("legacy capture config parses");
    assert!(parsed.enabled);
    assert_eq!(parsed.grid_cols, 8);
}

#[test]
fn capture_config_accepts_any_nonzero_backend_rate() {
    let mut config = CaptureConfig::default();
    for platform in [
        CapturePlatform::WindowsDesktopDuplication,
        CapturePlatform::LinuxPipeWire,
    ] {
        config.source = "auto".to_owned();
        config.capture_fps = 1;
        config
            .validate_for_platform(platform)
            .expect("minimum nonzero cadence should validate");
        config.capture_fps = u32::MAX;
        config
            .validate_for_platform(platform)
            .expect("configuration should not impose an arbitrary cadence ceiling");
    }
}

#[test]
fn capture_config_rejects_zero_cadence() {
    let config = CaptureConfig {
        capture_fps: 0,
        ..CaptureConfig::default()
    };
    assert!(matches!(
        config.validate_for_platform(CapturePlatform::WindowsDesktopDuplication),
        Err(CaptureConfigValidationError::CaptureFps { value: 0 })
    ));
}

#[test]
fn capture_config_accepts_arbitrary_nonzero_grid_dimensions() {
    let platform = CapturePlatform::WindowsDesktopDuplication;
    let config = CaptureConfig {
        grid_cols: u32::MAX,
        grid_rows: 256,
        ..CaptureConfig::default()
    };
    config
        .validate_for_platform(platform)
        .expect("grid dimensions are governed by byte admission, not axis caps");
}

#[test]
fn capture_config_rejects_empty_grid_and_invalid_float_values() {
    let platform = CapturePlatform::WindowsDesktopDuplication;
    let mut config = CaptureConfig {
        grid_cols: 0,
        ..CaptureConfig::default()
    };
    assert!(matches!(
        config.validate_for_platform(platform),
        Err(CaptureConfigValidationError::GridDimension {
            field: "grid_cols",
            value: 0
        })
    ));

    config.grid_cols = 8;
    config.smoothing = f32::NAN;
    assert!(matches!(
        config.validate_for_platform(platform),
        Err(CaptureConfigValidationError::FloatRange {
            field: "smoothing",
            ..
        })
    ));

    config.smoothing = 0.3;
    config.gamma = 0.19;
    assert!(matches!(
        config.validate_for_platform(platform),
        Err(CaptureConfigValidationError::FloatRange { field: "gamma", .. })
    ));
}

#[test]
fn capture_config_accepts_optional_nonzero_publication_memory_budget() {
    let platform = CapturePlatform::WindowsDesktopDuplication;
    let mut config = CaptureConfig {
        publication_memory_bytes: Some(1),
        ..CaptureConfig::default()
    };
    config
        .validate_for_platform(platform)
        .expect("one-byte explicit budget is semantically valid");

    config.publication_memory_bytes = Some(0);
    assert_eq!(
        config.validate_for_platform(platform),
        Err(CaptureConfigValidationError::PublicationMemoryBudget { value: 0 })
    );
}

#[test]
fn capture_config_validates_source_by_backend() {
    let mut config = CaptureConfig {
        source: r"monitor:\\?\DISPLAY#DEL40A9#stable".to_owned(),
        ..CaptureConfig::default()
    };
    config
        .validate_for_platform(CapturePlatform::WindowsDesktopDuplication)
        .expect("stable Windows display identities should validate");
    assert!(matches!(
        config.validate_for_platform(CapturePlatform::LinuxPipeWire),
        Err(CaptureConfigValidationError::Source { .. })
    ));
    config.enabled = false;
    config
        .validate_for_platform(CapturePlatform::LinuxPipeWire)
        .expect("disabled cross-platform source identities should remain portable");

    config.enabled = true;
    config.source = "auto\0hidden".to_owned();
    assert!(matches!(
        config.validate_for_platform(CapturePlatform::WindowsDesktopDuplication),
        Err(CaptureConfigValidationError::Source { .. })
    ));
}

#[test]
fn unsupported_capture_platform_only_accepts_disabled_config() {
    let mut config = CaptureConfig {
        enabled: false,
        ..CaptureConfig::default()
    };
    config
        .validate_for_platform(CapturePlatform::Unsupported)
        .expect("dormant portable capture config should remain loadable");
    config.grid_rows = 0;
    assert!(matches!(
        config.validate_for_platform(CapturePlatform::Unsupported),
        Err(CaptureConfigValidationError::GridDimension { .. })
    ));
    config.grid_rows = 6;
    config.enabled = true;
    assert_eq!(
        config.validate_for_platform(CapturePlatform::Unsupported),
        Err(CaptureConfigValidationError::UnsupportedPlatform)
    );
}

#[test]
fn discovery_defaults_match_spec() {
    let d = DiscoveryConfig::default();
    assert!(d.background_enabled);
    assert!(d.mdns_enabled);
    assert_eq!(d.scan_interval_secs, 300);
    assert!(d.blocks_scan);
}

#[test]
fn network_defaults_match_spec() {
    let n = NetworkConfig::default();
    assert_eq!(n.access_mode, NetworkAccessMode::LocalOnly);
    assert_eq!(n.client_scope, NetworkClientScope::LocalSubnets);
    assert!(n.mdns_publish);
    assert!(!n.remote_access);
    assert!(!n.allow_unauthenticated_remote_access);
    assert!(n.allowed_clients.is_empty());
    assert_eq!(n.instance_name, None);
}

#[test]
fn driver_registry_defaults_are_driver_agnostic() {
    let drivers = default_driver_configs();
    assert!(drivers.is_empty());
}

#[test]
fn govee_defaults_match_spec() {
    let g = GoveeConfig::default();
    assert!(g.known_ips.is_empty());
    assert!(!g.power_off_on_disconnect);
    assert_eq!(g.lan_state_fps, 10);
    assert_eq!(g.razer_fps, 25);
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

#[test]
fn session_defaults_match_spec() {
    let session = SessionConfig::default();
    assert!(session.enabled);
    assert!(session.idle_enabled);
    assert_eq!(session.idle_dim_timeout_secs, 120);
    assert_eq!(session.idle_off_timeout_secs, 600);
    assert_eq!(session.off_output_behavior, OffOutputBehavior::Static);
    assert_eq!(session.off_output_color, "#000000");
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
fn display_config_defaults_and_clamps_face_fps_cap() {
    let config = DisplayConfig::default();
    assert_eq!(config.face_fps_cap, 30);
    assert_eq!(config.effective_face_fps_cap(), 30);

    let low = DisplayConfig { face_fps_cap: 5 };
    assert_eq!(low.effective_face_fps_cap(), 15);

    let high = DisplayConfig { face_fps_cap: 240 };
    assert_eq!(high.effective_face_fps_cap(), 60);
}

#[test]
fn full_config_toml_roundtrip() {
    let original = HypercolorConfig {
        schema_version: 4,
        include: vec!["local.toml".into()],
        daemon: DaemonConfig::default(),
        web: WebConfig::default(),
        mcp: McpConfig::default(),
        effect_engine: EffectEngineConfig::default(),
        rendering: RenderingConfig::default(),
        media: MediaConfig::default(),
        audio: AudioConfig::default(),
        capture: CaptureConfig::default(),
        input: InputConfig::default(),
        display: DisplayConfig::default(),
        discovery: DiscoveryConfig::default(),
        network: NetworkConfig::default(),
        drivers: default_driver_configs(),
        dbus: DbusConfig::default(),
        tui: TuiConfig::default(),
        features: FeatureFlags::default(),
        session: SessionConfig::default(),
    };
    let toml_str = toml::to_string(&original).expect("serialize HypercolorConfig");
    let restored: HypercolorConfig =
        toml::from_str(&toml_str).expect("deserialize HypercolorConfig");
    assert_eq!(restored.schema_version, 4);
    assert_eq!(restored.include, vec!["local.toml"]);
    assert_eq!(restored.daemon.port, 9420);
    assert!(restored.web.enabled);
    assert_eq!(restored.mcp.base_path, "/mcp");
    assert_eq!(restored.audio.fft_size, 1024);
    assert_eq!(restored.capture.enabled, cfg!(target_os = "windows"));
    assert_eq!(
        restored.effect_engine.compositor_acceleration_mode,
        RenderAccelerationMode::Auto
    );
    assert_eq!(
        restored.rendering.servo_gpu_import.mode,
        ServoGpuImportMode::Auto
    );
    assert_eq!(restored.media.max_video_producers, 2);
    assert_eq!(restored.discovery.scan_interval_secs, 300);
    assert!(restored.network.mdns_publish);
    assert!(!restored.network.remote_access);
    assert!(!restored.network.allow_unauthenticated_remote_access);
    assert!(restored.network.allowed_clients.is_empty());
    assert!(restored.drivers.is_empty());
    assert!(restored.dbus.enabled);
    assert_eq!(restored.tui.theme, "silkcircuit");
    assert!(!restored.features.wasm_plugins);
}

// ─── Partial Deserialization (forward compatibility) ─────────────────────────

#[test]
fn minimal_toml_fills_defaults() {
    let minimal = "schema_version = 4\n";
    let config: HypercolorConfig = toml::from_str(minimal).expect("deserialize minimal config");
    assert_eq!(config.schema_version, 4);
    assert_eq!(config.daemon.port, 9420);
    assert!(config.web.enabled);
    assert_eq!(config.mcp.base_path, "/mcp");
    assert_eq!(config.audio.device, "default");
    assert_eq!(config.capture.enabled, cfg!(target_os = "windows"));
    assert_eq!(
        config.effect_engine.compositor_acceleration_mode,
        RenderAccelerationMode::Auto
    );
    assert_eq!(
        config.rendering.servo_gpu_import.mode,
        ServoGpuImportMode::Auto
    );
    assert_eq!(config.media.max_livestream_producers, 1);
    assert_eq!(config.tui.theme, "silkcircuit");
    assert!(config.network.mdns_publish);
    assert!(!config.network.remote_access);
    assert!(!config.network.allow_unauthenticated_remote_access);
    assert!(config.network.allowed_clients.is_empty());
    assert!(config.drivers.is_empty());
}

#[test]
fn servo_gpu_import_mode_toml_roundtrip() {
    let original = RenderingConfig {
        servo_gpu_import: ServoGpuImportConfig {
            mode: ServoGpuImportMode::Auto,
        },
    };
    let toml_str = toml::to_string(&original).expect("serialize RenderingConfig");
    let restored: RenderingConfig = toml::from_str(&toml_str).expect("deserialize RenderingConfig");
    assert_eq!(restored.servo_gpu_import.mode, ServoGpuImportMode::Auto);
}

#[test]
fn nested_servo_gpu_import_mode_deserializes() {
    let config: HypercolorConfig = toml::from_str(
        r#"
schema_version = 4

[rendering.servo_gpu_import]
mode = "on"
"#,
    )
    .expect("deserialize Servo GPU import mode");

    assert_eq!(
        config.rendering.servo_gpu_import.mode,
        ServoGpuImportMode::On
    );
}

#[test]
fn media_config_toml_deserializes_stream_policy() {
    let config: HypercolorConfig = toml::from_str(
        r#"
schema_version = 4

[media]
max_video_producers = 3
max_livestream_producers = 2
stream_private_network_allowlist = ["192.168.50.0/24", "fd00::/8"]
"#,
    )
    .expect("deserialize media config");

    assert_eq!(config.media.max_video_producers, 3);
    assert_eq!(config.media.max_livestream_producers, 2);
    assert_eq!(
        config.media.stream_private_network_allowlist,
        vec!["192.168.50.0/24".to_owned(), "fd00::/8".to_owned()]
    );
}

#[test]
fn driver_registry_toml_deserializes_unknown_driver_settings() {
    let config: HypercolorConfig = toml::from_str(
        r#"
schema_version = 4

[drivers.openrgb]
enabled = false
socket = "/run/openrgb.sock"
zones = ["keyboard", "mouse"]
"#,
    )
    .expect("deserialize driver registry config");

    let openrgb = &config.drivers["openrgb"];
    assert!(!openrgb.enabled);
    assert_eq!(openrgb.settings["socket"], "/run/openrgb.sock");
    assert_eq!(
        openrgb.settings["zones"],
        serde_json::json!(["keyboard", "mouse"])
    );
}

#[test]
fn effect_engine_compositor_acceleration_mode_toml_roundtrip() {
    let original = EffectEngineConfig {
        compositor_acceleration_mode: RenderAccelerationMode::Auto,
        effect_error_fallback: EffectErrorFallbackPolicy::ClearGroups,
        ..EffectEngineConfig::default()
    };
    let toml_str = toml::to_string(&original).expect("serialize EffectEngineConfig");
    let restored: EffectEngineConfig =
        toml::from_str(&toml_str).expect("deserialize EffectEngineConfig");
    assert_eq!(
        restored.compositor_acceleration_mode,
        RenderAccelerationMode::Auto
    );
    assert_eq!(
        restored.effect_error_fallback,
        EffectErrorFallbackPolicy::ClearGroups
    );
}

#[test]
fn legacy_render_acceleration_mode_deserializes_as_compositor_acceleration_mode() {
    let toml = r#"
preferred_renderer = "auto"
render_acceleration_mode = "gpu"
"#;
    let restored: EffectEngineConfig =
        toml::from_str(toml).expect("legacy acceleration key should deserialize");

    assert_eq!(
        restored.compositor_acceleration_mode,
        RenderAccelerationMode::Gpu
    );
}

#[test]
fn unknown_fields_ignored() {
    let toml_with_future_field = r#"
schema_version = 4

[daemon]
port = 9420
some_future_field = "hello from the future"
"#;
    let config: HypercolorConfig =
        toml::from_str(toml_with_future_field).expect("deserialize with unknown fields");
    assert_eq!(config.schema_version, 4);
    assert_eq!(config.daemon.port, 9420);
}

#[test]
fn override_specific_defaults() {
    let partial = r#"
schema_version = 4

[daemon]
port = 8080
target_fps = 120

[audio]
enabled = false
fft_size = 2048

[network]
access_mode = "lan_trusted"
client_scope = "private_ranges"
mdns_publish = false
remote_access = true
allow_unauthenticated_remote_access = true
allowed_clients = ["192.168.1.0/24", "fd00::/8"]
instance_name = "desk-pc"

[drivers.fixture-driver]
default_protocol = "e131"
known_ips = ["192.168.1.50"]
realtime_http_enabled = false
dedup_threshold = 0
"#;
    let config: HypercolorConfig = toml::from_str(partial).expect("deserialize partial config");
    assert_eq!(config.daemon.port, 8080);
    assert_eq!(config.daemon.target_fps, 120);
    // Non-overridden fields keep defaults
    assert_eq!(config.daemon.canvas_width, 640);
    assert_eq!(config.daemon.listen_address, "127.0.0.1");
    assert!(!config.audio.enabled);
    assert_eq!(config.audio.fft_size, 2048);
    assert_eq!(config.network.access_mode, NetworkAccessMode::LanTrusted);
    assert_eq!(
        config.network.client_scope,
        NetworkClientScope::PrivateRanges
    );
    assert!(!config.network.mdns_publish);
    assert!(config.network.remote_access);
    assert!(config.network.allow_unauthenticated_remote_access);
    assert_eq!(
        config.network.allowed_clients,
        vec!["192.168.1.0/24".to_owned(), "fd00::/8".to_owned()]
    );
    assert_eq!(config.network.instance_name.as_deref(), Some("desk-pc"));
    // Audio fields not overridden keep defaults
    assert!((config.audio.smoothing - 0.8).abs() < f32::EPSILON);
    assert_eq!(
        config.drivers["fixture-driver"].settings["default_protocol"],
        "e131"
    );
    assert_eq!(
        config.drivers["fixture-driver"].settings["known_ips"],
        serde_json::json!(["192.168.1.50"])
    );
    assert_eq!(
        config.drivers["fixture-driver"].settings["realtime_http_enabled"],
        false
    );
    assert_eq!(
        config.drivers["fixture-driver"].settings["dedup_threshold"],
        0
    );
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

#[test]
fn input_config_defaults_to_disabled_with_both_kinds_on() {
    let config = InputConfig::default();
    assert!(!config.enabled, "input capture must be opt-in");
    assert!(config.keyboard);
    assert!(config.mouse);
    assert_eq!(config.daemon_route, InteractionRoutePolicy::Host);
    assert_eq!(config.preview_route, InteractionRoutePolicy::Browser);

    let parsed: InputConfig = toml::from_str("").expect("empty input config parses");
    assert!(!parsed.enabled);
    assert!(parsed.keyboard);
    assert!(parsed.mouse);
    assert_eq!(parsed.daemon_route, InteractionRoutePolicy::Host);
    assert_eq!(parsed.preview_route, InteractionRoutePolicy::Browser);

    let full: HypercolorConfig = toml::from_str("schema_version = 4").expect("minimal config");
    assert!(!full.input.enabled);
    assert_eq!(full.input.daemon_route, InteractionRoutePolicy::Host);
    assert_eq!(full.input.preview_route, InteractionRoutePolicy::Browser);
}

#[test]
fn interaction_route_policies_use_stable_toml_spellings() {
    for (policy, spelling) in [
        (InteractionRoutePolicy::Host, "host"),
        (InteractionRoutePolicy::Browser, "browser"),
        (InteractionRoutePolicy::Merge, "merge"),
    ] {
        let input: InputConfig = toml::from_str(&format!("daemon_route = \"{spelling}\""))
            .expect("route spelling parses");
        assert_eq!(input.daemon_route, policy);

        let encoded = toml::to_string(&input).expect("route policy serializes");
        assert!(encoded.contains(&format!("daemon_route = \"{spelling}\"")));
    }
}

#[test]
fn invalid_interaction_route_policy_is_rejected() {
    let error = toml::from_str::<InputConfig>("daemon_route = \"all_browsers\"")
        .expect_err("unknown route policy must fail");
    assert!(error.to_string().contains("unknown variant"));
}
