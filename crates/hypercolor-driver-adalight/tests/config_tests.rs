//! Config parsing, validation, and line-budget plan tests (Spec 91 §7-8).

use std::time::Duration;

use hypercolor_driver_adalight::protocol::{AwaWhite, Framing};
use hypercolor_driver_adalight::{
    AdalightConfig, AdalightConfigError, AdalightDeviceConfig, AdalightProtocol, ColorOrder,
    WhiteCalibration, WhiteMode, resolve_config, resolve_device,
};
use serde_json::json;

fn device(value: serde_json::Value) -> AdalightDeviceConfig {
    serde_json::from_value(value).expect("device config should deserialize")
}

fn minimal() -> AdalightDeviceConfig {
    device(json!({ "id": "desk", "port": "/dev/ttyUSB0", "led_count": 60 }))
}

fn error_text(device: &AdalightDeviceConfig) -> String {
    resolve_device(device)
        .expect_err("config should be rejected")
        .to_string()
}

#[test]
fn toml_config_round_trips_with_defaults() {
    let config: AdalightConfig = toml::from_str(
        r#"
        [[devices]]
        id = "desk"
        port = "/dev/serial/by-id/usb-1a86_USB_Serial-if00-port0"
        led_count = 120

        [[devices]]
        id = "tv"
        name = "TV Backlight"
        port = "COM4"
        protocol = "awa"
        led_count = 250
        color_order = "grb"
        white = "firmware"
        white_calibration = { gain = 200, blue = 112 }
        startup_delay_ms = 1500
        latch_ms = 2
        max_fps = 50
        "#,
    )
    .expect("TOML should parse");

    let desk = &config.devices[0];
    assert_eq!(desk.protocol, AdalightProtocol::Ada);
    assert_eq!(desk.color_order, ColorOrder::Rgb);
    assert_eq!(desk.white, WhiteMode::Off);
    assert_eq!(desk.effective_baud_rate(), 115_200);
    assert_eq!(desk.display_name(), "desk");

    let tv = &config.devices[1];
    assert_eq!(
        tv.effective_baud_rate(),
        2_000_000,
        "AWA defaults to 2 Mbaud"
    );
    assert_eq!(tv.display_name(), "TV Backlight");
    assert_eq!(
        tv.white_calibration,
        Some(WhiteCalibration {
            gain: 200,
            red: 0xA0,
            green: 0xA0,
            blue: 112,
        }),
        "omitted calibration fields take the cold-white defaults"
    );

    let plans = resolve_config(&config).expect("config should validate");
    assert_eq!(
        plans[1].layout.framing(),
        Framing::Awa(AwaWhite::Firmware(WhiteCalibration {
            gain: 200,
            red: 0xA0,
            green: 0xA0,
            blue: 112,
        }))
    );
    assert_eq!(plans[1].startup_delay, Duration::from_millis(1_500));

    let reparsed: AdalightConfig =
        serde_json::from_value(serde_json::to_value(&config).expect("serialize"))
            .expect("round trip");
    assert_eq!(reparsed, config);
}

#[test]
fn enum_spellings_parse_case_insensitively() {
    let parsed = device(json!({
        "id": "x", "port": "/dev/ttyACM0", "led_count": 1,
        "protocol": "HyperSerial", "color_order": "BGR", "white": "Host"
    }));
    assert_eq!(parsed.protocol, AdalightProtocol::Awa);
    assert_eq!(parsed.color_order, ColorOrder::Bgr);
    assert_eq!(parsed.white, WhiteMode::Host);

    let error = serde_json::from_value::<AdalightDeviceConfig>(json!({
        "id": "x", "port": "/dev/ttyACM0", "led_count": 1, "protocol": "tpm2"
    }))
    .expect_err("unknown protocol");
    assert!(error.to_string().contains("expected ada or awa"));
}

#[test]
fn firmware_white_without_calibration_uses_cold_white() {
    let mut config = minimal();
    config.protocol = AdalightProtocol::Awa;
    config.white = WhiteMode::Firmware;
    let plan = resolve_device(&config).expect("valid");
    assert_eq!(
        plan.layout.framing(),
        Framing::Awa(AwaWhite::Firmware(WhiteCalibration::COLD_WHITE))
    );
}

#[test]
fn calibration_is_ignored_unless_firmware_derives_white() {
    let mut config = minimal();
    config.protocol = AdalightProtocol::Awa;
    config.white_calibration = Some(WhiteCalibration::COLD_WHITE);
    let plan = resolve_device(&config).expect("valid");
    assert_eq!(plan.layout.framing(), Framing::Awa(AwaWhite::Off));
}

#[test]
fn white_modes_need_awa() {
    for white in [WhiteMode::Firmware, WhiteMode::Host] {
        let mut config = minimal();
        config.white = white;
        assert!(error_text(&config).contains("need protocol = \"awa\""));
    }
}

#[test]
fn device_ids_are_validated() {
    for id in [
        "",
        "has space",
        "slash/",
        "dots.not.allowed",
        &"x".repeat(65),
    ] {
        let mut config = minimal();
        config.id = id.to_owned();
        assert!(error_text(&config).contains("id must be"), "id {id:?}");
    }
    let mut config = minimal();
    config.id = "Desk_Strip-2".to_owned();
    assert!(resolve_device(&config).is_ok());
}

#[test]
fn ports_are_trimmed_and_validated() {
    let mut config = minimal();
    config.port = "  COM3 ".to_owned();
    assert_eq!(resolve_device(&config).expect("valid").port, "COM3");

    config.port = "   ".to_owned();
    assert!(error_text(&config).contains("port must not be empty"));

    config.port = "/dev/tty\nUSB0".to_owned();
    assert!(error_text(&config).contains("control characters"));

    config.port = format!("/dev/{}", "x".repeat(300));
    assert!(error_text(&config).contains("longer than 255"));
}

#[test]
fn numeric_limits_are_enforced() {
    let mut config = minimal();
    config.baud_rate = Some(300);
    assert!(error_text(&config).contains("baud_rate 300"));
    config.baud_rate = Some(12_000_001);
    assert!(error_text(&config).contains("baud_rate 12000001"));

    let mut config = minimal();
    config.led_count = 0;
    assert!(error_text(&config).contains("LED count 0"));

    let mut config = minimal();
    config.protocol = AdalightProtocol::Awa;
    config.led_count = 4_097;
    assert!(error_text(&config).contains("outside 1-4096"));

    let mut config = minimal();
    config.startup_delay_ms = Some(10_001);
    assert!(error_text(&config).contains("startup_delay_ms"));

    let mut config = minimal();
    config.latch_ms = Some(1_001);
    assert!(error_text(&config).contains("latch_ms"));

    let mut config = minimal();
    config.max_fps = Some(1_001);
    assert!(error_text(&config).contains("max_fps"));

    let mut config = minimal();
    config.name = Some("n".repeat(129));
    assert!(error_text(&config).contains("name is longer"));
}

#[test]
fn layouts_slower_than_one_frame_per_second_are_rejected() {
    // 4000 LEDs at 9600 baud: 12006 bytes, 12.5 s per frame.
    let mut config = minimal();
    config.led_count = 4_000;
    config.baud_rate = Some(9_600);
    let message = error_text(&config);
    assert!(message.contains("12006-byte frame"), "{message}");
    assert!(message.contains("cannot carry one frame per second"));
}

#[test]
fn duplicate_ids_and_ports_are_rejected() {
    let config = AdalightConfig {
        devices: vec![minimal(), minimal()],
    };
    assert_eq!(
        resolve_config(&config).expect_err("duplicate id"),
        AdalightConfigError::DuplicateId {
            id: "desk".to_owned()
        }
    );

    let mut second = minimal();
    second.id = "shelf".to_owned();
    second.port = " /dev/ttyUSB0".to_owned();
    let config = AdalightConfig {
        devices: vec![minimal(), second],
    };
    assert_eq!(
        resolve_config(&config).expect_err("duplicate port"),
        AdalightConfigError::DuplicatePort {
            first: "desk".to_owned(),
            second: "shelf".to_owned(),
            port: "/dev/ttyUSB0".to_owned(),
        }
    );
}

#[test]
fn device_count_is_capped() {
    let devices = (0..65)
        .map(|index| {
            device(json!({
                "id": format!("d{index}"), "port": format!("/dev/ttyUSB{index}"), "led_count": 1
            }))
        })
        .collect();
    assert_eq!(
        resolve_config(&AdalightConfig { devices }).expect_err("too many"),
        AdalightConfigError::TooManyDevices { count: 65 }
    );
}

// ── Line budget ─────────────────────────────────────────────────────────

#[test]
fn plan_reports_the_line_limit() {
    // 300 LEDs at 115200: 906 bytes take 78.646 ms, so 12 FPS.
    let mut config = minimal();
    config.led_count = 300;
    let plan = resolve_device(&config).expect("valid");
    assert_eq!(plan.frame_bytes(), 906);
    assert_eq!(plan.frame_time(), Duration::from_micros(78_646));
    assert_eq!(plan.line_interval(), Duration::from_micros(78_646));
    assert_eq!(plan.line_max_fps(), 12);
    assert_eq!(plan.effective_max_fps(), 12);
    assert_eq!(plan.frame_interval(), Duration::from_micros(78_646));
}

#[test]
fn fast_lines_leave_the_render_loop_unthrottled() {
    // 60 LEDs of AWA at 2 Mbaud: 189 bytes, 945 us, over a thousand FPS.
    let mut config = minimal();
    config.protocol = AdalightProtocol::Awa;
    let plan = resolve_device(&config).expect("valid");
    assert_eq!(plan.frame_bytes(), 189);
    assert_eq!(plan.frame_time(), Duration::from_micros(945));
    assert_eq!(plan.line_max_fps(), 1_058);
    assert!(plan.frame_interval() < Duration::from_secs(1) / 60);
}

#[test]
fn latch_gap_extends_the_line_interval() {
    let mut config = minimal();
    config.led_count = 100;
    config.latch_ms = Some(4);
    let plan = resolve_device(&config).expect("valid");
    // 306 bytes at 115200 is 26.563 ms, plus the 4 ms latch.
    assert_eq!(plan.frame_time(), Duration::from_micros(26_563));
    assert_eq!(plan.line_interval(), Duration::from_micros(30_563));
    assert_eq!(plan.line_max_fps(), 32);
}

#[test]
fn user_cap_only_ever_slows_the_device() {
    let mut config = minimal();
    config.protocol = AdalightProtocol::Awa;
    config.max_fps = Some(30);
    let plan = resolve_device(&config).expect("valid");
    assert_eq!(plan.effective_max_fps(), 30);
    assert_eq!(plan.frame_interval(), Duration::from_secs(1) / 30);

    // A cap above what the line carries changes nothing.
    let mut config = minimal();
    config.led_count = 300;
    config.max_fps = Some(60);
    let plan = resolve_device(&config).expect("valid");
    assert_eq!(plan.effective_max_fps(), 12);
    assert_eq!(plan.frame_interval(), plan.line_interval());
}
