//! Config parsing, validation, and output planning for the pixel driver.

use std::collections::BTreeMap;
use std::net::IpAddr;

use hypercolor_driver_api::DriverConfigView;
use hypercolor_driver_pixel::plan::{ChannelRun, DdpSpanPlan, OutputPlan, UniversePlan};
use hypercolor_driver_pixel::{
    ColorOrder, PixelConfig, PixelConfigError, PixelProtocol, derived_cid, resolve_config,
};
use hypercolor_pixel_protocols::ddp::{DDP_DTYPE_RGB8, DDP_DTYPE_RGBW8, DDP_DTYPE_UNDEFINED};
use hypercolor_types::config::DriverConfigEntry;
use serde_json::json;

fn parse(settings: serde_json::Value) -> PixelConfig {
    let entry = DriverConfigEntry::enabled(
        settings
            .as_object()
            .expect("settings fixture should be an object")
            .clone()
            .into_iter()
            .collect(),
    );
    DriverConfigView {
        driver_id: "pixel",
        entry: &entry,
    }
    .parse_settings()
    .expect("settings fixture should parse")
}

fn one_device(device: serde_json::Value) -> PixelConfig {
    parse(json!({ "devices": [device] }))
}

fn resolve_error(config: &PixelConfig) -> String {
    resolve_config(config)
        .expect_err("config should be rejected")
        .to_string()
}

// ── Parsing ─────────────────────────────────────────────────────────────

#[test]
fn empty_settings_use_standard_defaults() {
    let config = parse(json!({}));
    assert!(config.devices.is_empty());
    assert_eq!(config.source_name, "Hypercolor");
    assert_eq!(config.priority, 100);
    assert!(
        resolve_config(&config)
            .expect("empty config is valid")
            .is_empty()
    );
}

#[test]
fn protocol_and_color_order_parse_case_insensitively() {
    let config = parse(json!({
        "devices": [
            { "id": "a", "host": "10.0.0.5", "protocol": "sACN", "pixel_count": 1, "color_order": "GRB" },
            { "id": "b", "host": "10.0.0.6", "protocol": "Art-Net", "pixel_count": 1 },
            { "id": "c", "host": "10.0.0.7", "protocol": "DDP", "pixel_count": 1, "color_order": "Grbw" }
        ]
    }));
    assert_eq!(config.devices[0].protocol, PixelProtocol::E131);
    assert_eq!(config.devices[0].color_order, ColorOrder::Grb);
    assert_eq!(config.devices[1].protocol, PixelProtocol::ArtNet);
    assert_eq!(config.devices[1].color_order, ColorOrder::Rgb);
    assert_eq!(config.devices[2].color_order, ColorOrder::Grbw);
}

#[test]
fn unknown_protocol_is_a_parse_error() {
    let entry = DriverConfigEntry::enabled(
        json!({ "devices": [{ "id": "a", "host": "10.0.0.5", "protocol": "dmx", "pixel_count": 1 }] })
            .as_object()
            .expect("object")
            .clone()
            .into_iter()
            .collect(),
    );
    let error = DriverConfigView {
        driver_id: "pixel",
        entry: &entry,
    }
    .parse_settings::<PixelConfig>()
    .expect_err("unknown protocol should not parse");
    assert!(error.to_string().contains("unknown protocol 'dmx'"));
}

#[test]
fn toml_example_from_docs_parses_and_resolves() {
    let toml_text = r#"
[drivers.pixel]
enabled = true
source_name = "Hypercolor"
priority = 100

[[drivers.pixel.devices]]
id = "roofline"
name = "Roofline Falcon"
host = "192.168.1.60"
protocol = "e131"
start_universe = 1
color_order = "rgb"

[[drivers.pixel.devices.segments]]
name = "Front"
pixel_count = 300

[[drivers.pixel.devices.segments]]
name = "Garage"
pixel_count = 150
start_universe = 4

[[drivers.pixel.devices]]
id = "tree"
host = "192.168.1.61"
protocol = "ddp"
pixel_count = 1000

[[drivers.pixel.devices]]
id = "stage-pars"
host = "192.168.1.70"
protocol = "artnet"
start_universe = 1
start_channel = 17
color_order = "rgbw"
pixel_count = 8
"#;
    // The `[drivers]` table as the daemon's config file nests it.
    #[derive(serde::Deserialize)]
    struct ConfigFile {
        drivers: BTreeMap<String, DriverConfigEntry>,
    }
    let config: ConfigFile = toml::from_str(toml_text).expect("docs example should parse");
    let entry = config.drivers.get("pixel").expect("pixel entry");
    assert!(entry.enabled);
    let pixel: PixelConfig = DriverConfigView {
        driver_id: "pixel",
        entry,
    }
    .parse_settings()
    .expect("pixel settings should parse");
    let plans = resolve_config(&pixel).expect("docs example should validate");
    assert_eq!(plans.len(), 3);
    assert_eq!(plans[0].universe_summary().as_deref(), Some("1-2, 4"));
    assert_eq!(plans[0].total_pixels(), 450);
    assert_eq!(plans[1].universe_summary(), None);
    assert_eq!(plans[2].universe_summary().as_deref(), Some("1"));
}

// ── DMX planning ────────────────────────────────────────────────────────

#[test]
fn rgb_device_fills_510_channel_universes() {
    let config = one_device(json!({
        "id": "strip", "host": "10.0.0.5", "protocol": "e131", "pixel_count": 400
    }));
    let plans = resolve_config(&config).expect("valid");
    let OutputPlan::E131 { dmx, source } = &plans[0].output else {
        panic!("expected an E1.31 plan");
    };
    assert_eq!(
        dmx.universes,
        vec![
            UniversePlan {
                universe: 1,
                channel_count: 510
            },
            UniversePlan {
                universe: 2,
                channel_count: 510
            },
            UniversePlan {
                universe: 3,
                channel_count: 180
            },
        ]
    );
    assert_eq!(dmx.runs[2].source, 1020..1200);
    assert_eq!(source.priority, 100);
    assert_eq!(source.source_name, "Hypercolor");
    assert_eq!(source.cid, derived_cid("strip"));
    assert_eq!(plans[0].target.port(), 5568);
}

#[test]
fn rgbw_device_packs_128_pixels_per_universe() {
    let config = one_device(json!({
        "id": "rgbw", "host": "10.0.0.5", "protocol": "artnet", "pixel_count": 129,
        "color_order": "rgbw"
    }));
    let plans = resolve_config(&config).expect("valid");
    assert_eq!(plans[0].universes(), vec![1, 2]);
    assert_eq!(plans[0].channels_per_pixel, 4);
    assert_eq!(plans[0].target.port(), 6454);
}

#[test]
fn start_channel_zero_fills_lower_slots() {
    let config = one_device(json!({
        "id": "par", "host": "10.0.0.5", "protocol": "artnet", "pixel_count": 2,
        "start_universe": 3, "start_channel": 17
    }));
    let plans = resolve_config(&config).expect("valid");
    let OutputPlan::ArtNet { dmx } = &plans[0].output else {
        panic!("expected an Art-Net plan");
    };
    assert_eq!(
        dmx.universes,
        vec![UniversePlan {
            universe: 3,
            channel_count: 22
        }]
    );
    assert_eq!(
        dmx.runs,
        vec![ChannelRun {
            universe_index: 0,
            first_slot: 16,
            source: 0..6
        }]
    );
}

#[test]
fn full_512_channel_universes_split_pixels_across_boundaries() {
    let config = one_device(json!({
        "id": "esps", "host": "10.0.0.5", "protocol": "e131", "pixel_count": 200,
        "channels_per_universe": 512
    }));
    let plans = resolve_config(&config).expect("valid");
    let OutputPlan::E131 { dmx, .. } = &plans[0].output else {
        panic!("expected an E1.31 plan");
    };
    assert_eq!(dmx.runs[0].source, 0..512);
    assert_eq!(dmx.runs[1].source, 512..600);
}

#[test]
fn segments_continue_or_jump_to_their_universe() {
    let config = one_device(json!({
        "id": "falcon", "host": "10.0.0.5", "protocol": "e131", "start_universe": 10,
        "segments": [
            { "name": "Port 1", "pixel_count": 100 },
            { "pixel_count": 100 },
            { "name": "Port 3", "pixel_count": 10, "start_universe": 20, "start_channel": 4 }
        ]
    }));
    let plans = resolve_config(&config).expect("valid");
    let plan = &plans[0];
    assert_eq!(plan.segments[1].name, "Output 2");
    let OutputPlan::E131 { dmx, .. } = &plan.output else {
        panic!("expected an E1.31 plan");
    };
    // Port 1 fills 300 channels of universe 10; port 2 continues at slot
    // 300, wraps after 210 channels into universe 11.
    assert_eq!(
        dmx.runs,
        vec![
            ChannelRun {
                universe_index: 0,
                first_slot: 0,
                source: 0..300
            },
            ChannelRun {
                universe_index: 0,
                first_slot: 300,
                source: 300..510
            },
            ChannelRun {
                universe_index: 1,
                first_slot: 0,
                source: 510..600
            },
            ChannelRun {
                universe_index: 2,
                first_slot: 3,
                source: 600..630
            },
        ]
    );
    assert_eq!(plan.universe_summary().as_deref(), Some("10-11, 20"));
}

#[test]
fn overlapping_segments_are_rejected() {
    let config = one_device(json!({
        "id": "overlap", "host": "10.0.0.5", "protocol": "e131",
        "segments": [
            { "pixel_count": 10 },
            { "pixel_count": 10, "start_universe": 1, "start_channel": 10 }
        ]
    }));
    assert!(resolve_error(&config).contains("overlaps another segment in universe 1"));
}

#[test]
fn universe_ranges_follow_each_standard() {
    let e131_zero = one_device(json!({
        "id": "a", "host": "10.0.0.5", "protocol": "e131", "pixel_count": 1, "start_universe": 0
    }));
    assert!(resolve_error(&e131_zero).contains("outside the E1.31 (sACN) range 1-63999"));

    let artnet_zero = one_device(json!({
        "id": "a", "host": "10.0.0.5", "protocol": "artnet", "pixel_count": 1, "start_universe": 0
    }));
    assert_eq!(
        resolve_config(&artnet_zero).expect("Port-Address 0 stays addressable")[0].universes(),
        vec![0]
    );

    let artnet_high = one_device(json!({
        "id": "a", "host": "10.0.0.5", "protocol": "artnet", "pixel_count": 1,
        "start_universe": 32_768
    }));
    assert!(resolve_error(&artnet_high).contains("Art-Net range 0-32767"));

    let runs_past = one_device(json!({
        "id": "a", "host": "10.0.0.5", "protocol": "e131", "pixel_count": 400,
        "start_universe": 63_998
    }));
    assert!(resolve_error(&runs_past).contains("runs past E1.31 (sACN) universe 63999"));
}

#[test]
fn start_channel_must_fit_the_universe() {
    let config = one_device(json!({
        "id": "a", "host": "10.0.0.5", "protocol": "e131", "pixel_count": 1, "start_channel": 511
    }));
    assert!(resolve_error(&config).contains("start_channel 511 must be 1-510"));
}

#[test]
fn segment_start_channel_requires_start_universe() {
    let config = one_device(json!({
        "id": "a", "host": "10.0.0.5", "protocol": "e131",
        "segments": [{ "pixel_count": 1 }, { "pixel_count": 1, "start_channel": 100 }]
    }));
    assert!(resolve_error(&config).contains("segment 2 start_channel requires start_universe"));
}

// ── DDP planning ────────────────────────────────────────────────────────

#[test]
fn ddp_spans_honor_offsets() {
    let config = one_device(json!({
        "id": "fpp", "host": "10.0.0.5", "protocol": "ddp", "ddp_offset": 300,
        "segments": [
            { "pixel_count": 10 },
            { "pixel_count": 5, "ddp_offset": 3000 },
            { "pixel_count": 2 }
        ]
    }));
    let plans = resolve_config(&config).expect("valid");
    let OutputPlan::Ddp(ddp) = &plans[0].output else {
        panic!("expected a DDP plan");
    };
    assert_eq!(
        ddp.spans,
        vec![
            DdpSpanPlan {
                offset: 300,
                source: 0..30
            },
            DdpSpanPlan {
                offset: 3000,
                source: 30..45
            },
            DdpSpanPlan {
                offset: 3015,
                source: 45..51
            },
        ]
    );
    assert_eq!(ddp.data_type, DDP_DTYPE_RGB8);
    assert_eq!(ddp.chunk_len, 1440);
    assert_eq!(plans[0].target.port(), 4048);
}

#[test]
fn ddp_data_type_tracks_pixel_layout() {
    let rgbw = one_device(json!({
        "id": "a", "host": "10.0.0.5", "protocol": "ddp", "pixel_count": 1, "color_order": "grbw"
    }));
    let OutputPlan::Ddp(ddp) = &resolve_config(&rgbw).expect("valid")[0].output else {
        panic!("expected a DDP plan");
    };
    assert_eq!(ddp.data_type, DDP_DTYPE_RGBW8);

    let padded = one_device(json!({
        "id": "a", "host": "10.0.0.5", "protocol": "ddp", "pixel_count": 1,
        "channels_per_pixel": 5
    }));
    let OutputPlan::Ddp(ddp) = &resolve_config(&padded).expect("valid")[0].output else {
        panic!("expected a DDP plan");
    };
    assert_eq!(ddp.data_type, DDP_DTYPE_UNDEFINED);
    assert_eq!(ddp.chunk_len, 1440);
}

#[test]
fn ddp_segments_cannot_overlap() {
    let config = one_device(json!({
        "id": "a", "host": "10.0.0.5", "protocol": "ddp",
        "segments": [{ "pixel_count": 10 }, { "pixel_count": 10, "ddp_offset": 15 }]
    }));
    assert!(resolve_error(&config).contains("overlaps another segment's DDP bytes"));
}

// ── Validation ──────────────────────────────────────────────────────────

#[test]
fn device_id_must_be_a_slug() {
    let config = one_device(json!({
        "id": "has space", "host": "10.0.0.5", "protocol": "ddp", "pixel_count": 1
    }));
    assert!(resolve_error(&config).contains("id must be 1-64 ASCII letters"));
}

#[test]
fn duplicate_ids_are_rejected_case_insensitively() {
    let config = parse(json!({ "devices": [
        { "id": "Tree", "host": "10.0.0.5", "protocol": "ddp", "pixel_count": 1 },
        { "id": "tree", "host": "10.0.0.6", "protocol": "ddp", "pixel_count": 1 }
    ]}));
    assert_eq!(
        resolve_config(&config),
        Err(PixelConfigError::DuplicateId {
            id: "tree".to_owned()
        })
    );
}

#[test]
fn multicast_and_broadcast_hosts_are_rejected() {
    for host in ["239.255.0.1", "255.255.255.255", "0.0.0.0", "ff02::1"] {
        let config = one_device(json!({
            "id": "a", "host": host, "protocol": "e131", "pixel_count": 1
        }));
        assert!(
            resolve_error(&config).contains("is not a unicast address"),
            "{host} should be rejected"
        );
    }
}

#[test]
fn loopback_and_link_local_hosts_are_allowed() {
    for host in ["127.0.0.1", "169.254.10.20", "::1"] {
        let config = one_device(json!({
            "id": "a", "host": host, "protocol": "ddp", "pixel_count": 1
        }));
        let plans = resolve_config(&config).expect("unicast host should be accepted");
        assert_eq!(
            plans[0].target.ip(),
            host.parse::<IpAddr>().expect("valid IP")
        );
    }
}

#[test]
fn privileged_ports_are_rejected() {
    let config = one_device(json!({
        "id": "a", "host": "10.0.0.5", "protocol": "ddp", "pixel_count": 1, "port": 80
    }));
    assert!(resolve_error(&config).contains("privileged port 80"));
}

#[test]
fn pixel_count_and_segments_are_exclusive_and_required() {
    let both = one_device(json!({
        "id": "a", "host": "10.0.0.5", "protocol": "ddp", "pixel_count": 1,
        "segments": [{ "pixel_count": 1 }]
    }));
    assert!(resolve_error(&both).contains("either pixel_count or segments"));

    let neither = one_device(json!({ "id": "a", "host": "10.0.0.5", "protocol": "ddp" }));
    assert!(resolve_error(&neither).contains("set pixel_count or at least one segment"));

    let zero = one_device(json!({
        "id": "a", "host": "10.0.0.5", "protocol": "ddp", "pixel_count": 0
    }));
    assert!(resolve_error(&zero).contains("pixel_count must be at least 1"));
}

#[test]
fn protocol_specific_fields_are_rejected_elsewhere() {
    let priority_on_ddp = one_device(json!({
        "id": "a", "host": "10.0.0.5", "protocol": "ddp", "pixel_count": 1, "priority": 120
    }));
    assert!(resolve_error(&priority_on_ddp).contains("priority only applies to e131"));

    let offset_on_e131 = one_device(json!({
        "id": "a", "host": "10.0.0.5", "protocol": "e131", "pixel_count": 1, "ddp_offset": 3
    }));
    assert!(resolve_error(&offset_on_e131).contains("ddp_offset only applies to ddp"));

    let universe_on_ddp = one_device(json!({
        "id": "a", "host": "10.0.0.5", "protocol": "ddp", "pixel_count": 1, "start_universe": 2
    }));
    assert!(resolve_error(&universe_on_ddp).contains("universe and channel addressing"));

    let sync_on_artnet = one_device(json!({
        "id": "a", "host": "10.0.0.5", "protocol": "artnet", "pixel_count": 1, "sync_universe": 9
    }));
    assert!(resolve_error(&sync_on_artnet).contains("sync_universe only applies to e131"));
}

#[test]
fn channels_per_pixel_must_cover_the_color_order() {
    let config = one_device(json!({
        "id": "a", "host": "10.0.0.5", "protocol": "e131", "pixel_count": 1,
        "color_order": "rgbw", "channels_per_pixel": 3
    }));
    assert!(resolve_error(&config).contains("channels_per_pixel must be 4-512"));
}

#[test]
fn e131_priority_and_source_name_are_bounded() {
    let config = one_device(json!({
        "id": "a", "host": "10.0.0.5", "protocol": "e131", "pixel_count": 1, "priority": 201
    }));
    assert!(resolve_error(&config).contains("priority 201 is outside the E1.31 range 0-200"));

    let long_name = parse(json!({ "source_name": "x".repeat(64) }));
    assert!(resolve_error(&long_name).contains("E1.31 allows at most 63"));
}

#[test]
fn driver_defaults_flow_into_e131_devices_only() {
    let config = parse(json!({
        "source_name": "Garage Rig",
        "priority": 140,
        "devices": [
            { "id": "a", "host": "10.0.0.5", "protocol": "e131", "pixel_count": 1 },
            { "id": "b", "host": "10.0.0.6", "protocol": "e131", "pixel_count": 1, "priority": 90,
              "source_name": "Override" },
            { "id": "c", "host": "10.0.0.7", "protocol": "ddp", "pixel_count": 1 }
        ]
    }));
    let plans = resolve_config(&config).expect("valid");
    let source = |index: usize| match &plans[index].output {
        OutputPlan::E131 { source, .. } => (source.priority, source.source_name.clone()),
        _ => panic!("expected an E1.31 plan"),
    };
    assert_eq!(source(0), (140, "Garage Rig".to_owned()));
    assert_eq!(source(1), (90, "Override".to_owned()));
    assert!(matches!(plans[2].output, OutputPlan::Ddp(_)));
}

#[test]
fn explicit_cid_overrides_the_derived_one() {
    let cid = "6f1c7a2e-3b4d-4e5f-8a9b-0c1d2e3f4a5b";
    let config = one_device(json!({
        "id": "a", "host": "10.0.0.5", "protocol": "e131", "pixel_count": 1, "cid": cid
    }));
    let OutputPlan::E131 { source, .. } = &resolve_config(&config).expect("valid")[0].output else {
        panic!("expected an E1.31 plan");
    };
    assert_eq!(source.cid.to_string(), cid);
    assert_ne!(derived_cid("a"), derived_cid("b"));
}

#[test]
fn two_devices_cannot_share_a_universe_on_one_receiver() {
    let config = parse(json!({ "devices": [
        { "id": "left", "host": "10.0.0.5", "protocol": "artnet", "pixel_count": 2 },
        { "id": "right", "host": "10.0.0.5", "protocol": "artnet", "pixel_count": 2,
          "start_channel": 100 }
    ]}));
    let error = resolve_error(&config);
    assert!(error.contains("'left' and 'right' both send Art-Net universe 1 to 10.0.0.5:6454"));
    assert!(error.contains("make them segments of one device"));
}

#[test]
fn same_universe_on_other_receivers_or_protocols_is_fine() {
    let config = parse(json!({ "devices": [
        { "id": "a", "host": "10.0.0.5", "protocol": "e131", "pixel_count": 2 },
        { "id": "b", "host": "10.0.0.6", "protocol": "e131", "pixel_count": 2 },
        { "id": "c", "host": "10.0.0.5", "protocol": "artnet", "pixel_count": 2 }
    ]}));
    assert_eq!(resolve_config(&config).expect("valid").len(), 3);
}

#[test]
fn ddp_devices_may_share_a_receiver_without_overlapping() {
    let ok = parse(json!({ "devices": [
        { "id": "a", "host": "10.0.0.5", "protocol": "ddp", "pixel_count": 10 },
        { "id": "b", "host": "10.0.0.5", "protocol": "ddp", "pixel_count": 10, "ddp_offset": 30 }
    ]}));
    assert!(resolve_config(&ok).is_ok());

    let overlap = parse(json!({ "devices": [
        { "id": "a", "host": "10.0.0.5", "protocol": "ddp", "pixel_count": 10 },
        { "id": "b", "host": "10.0.0.5", "protocol": "ddp", "pixel_count": 10, "ddp_offset": 29 }
    ]}));
    assert!(resolve_error(&overlap).contains("overlap DDP bytes 29-29 at 10.0.0.5:4048"));
}

#[test]
fn max_fps_is_bounded() {
    let config = one_device(json!({
        "id": "a", "host": "10.0.0.5", "protocol": "ddp", "pixel_count": 1, "max_fps": 1001
    }));
    assert!(resolve_error(&config).contains("max_fps must be 0-1000"));
}
