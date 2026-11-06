//! Golden JSON for every request, reply decoding, module classes, color
//! conversion, and request signing (Spec 93 §2 to §7).

use hypercolor_driver_wiz::protocol::{
    DISCOVERY_REQUEST, GET_MODEL_CONFIG, GET_PILOT, GET_SYSTEM_CONFIG, ModelConfig, ModuleClass,
    Pilot, ProtocolError, RegistrationResult, SigningKey, SystemConfig, canonical_mac,
    decode_reply, encode_set_pilot, hmac_sha256, reply_result,
};

const KEY: &str = "00112233445566778899aabbccddeeff";

#[test]
fn query_requests_match_the_wire() {
    assert_eq!(
        DISCOVERY_REQUEST,
        r#"{"method":"registration","params":{"phoneMac":"AAAAAAAAAAAA","register":false,"phoneIp":"1.2.3.4","id":"1"}}"#
    );
    assert_eq!(
        GET_SYSTEM_CONFIG,
        r#"{"method":"getSystemConfig","params":{}}"#
    );
    assert_eq!(
        GET_MODEL_CONFIG,
        r#"{"method":"getModelConfig","params":{}}"#
    );
    assert_eq!(GET_PILOT, r#"{"method":"getPilot","params":{}}"#);
    for request in [
        DISCOVERY_REQUEST,
        GET_SYSTEM_CONFIG,
        GET_MODEL_CONFIG,
        GET_PILOT,
    ] {
        serde_json::from_str::<serde_json::Value>(request).expect("requests are valid JSON");
    }
}

#[test]
fn replies_decode_with_or_without_a_trailing_nul() {
    let reply = decode_reply(
        br#"{"method":"registration","env":"pro","result":{"mac":"a8bb50000001","success":true}}"#,
    )
    .expect("reply");
    let result: RegistrationResult = reply_result(&reply, "registration").expect("result");
    assert_eq!(result.mac, "a8bb50000001");
    assert_eq!(result.success, Some(true));

    let with_nul = decode_reply(b"{\"method\":\"getPilot\",\"result\":{}}\0").expect("reply");
    assert_eq!(with_nul.method, "getPilot");
    assert!(decode_reply(b"not json").is_err());
}

#[test]
fn device_errors_and_wrong_methods_are_errors() {
    let reply = decode_reply(
        br#"{"method":"getModelConfig","env":"pro","error":{"code":-32601,"message":"Method not found"}}"#,
    )
    .expect("reply");
    assert_eq!(
        reply_result::<ModelConfig>(&reply, "getModelConfig"),
        Err(ProtocolError::Device {
            method: "getModelConfig".to_owned(),
            code: -32601,
            message: "Method not found".to_owned(),
        })
    );
    let other = decode_reply(br#"{"method":"getPilot","result":{}}"#).expect("reply");
    assert!(matches!(
        reply_result::<serde_json::Value>(&other, "getSystemConfig"),
        Err(ProtocolError::UnexpectedMethod { .. })
    ));
    let empty = decode_reply(br#"{"method":"getSystemConfig"}"#).expect("reply");
    assert!(matches!(
        reply_result::<SystemConfig>(&empty, "getSystemConfig"),
        Err(ProtocolError::MissingResult(_))
    ));
}

#[test]
fn system_and_model_configs_decode() {
    let reply = decode_reply(
        br#"{"method":"getSystemConfig","env":"pro","result":{"mac":"a8bb5006033d","homeId":653906,"roomId":989983,"moduleName":"ESP01_SHRGB1C_31","fwVersion":"1.25.0","groupId":0,"drvConf":[20,2],"ping":0}}"#,
    )
    .expect("reply");
    let system: SystemConfig = reply_result(&reply, "getSystemConfig").expect("system");
    assert_eq!(system.mac, "a8bb5006033d");
    assert_eq!(system.module_name.as_deref(), Some("ESP01_SHRGB1C_31"));
    assert_eq!(system.fw_version.as_deref(), Some("1.25.0"));

    let reply = decode_reply(
        br#"{"method":"getModelConfig","env":"pro","result":{"ps":1,"pwmFreq":1000,"cctRange":[2200,2700,6500,6500],"nowc":1,"minDimLevel":1,"hasGradient":1}}"#,
    )
    .expect("reply");
    let model: ModelConfig = reply_result(&reply, "getModelConfig").expect("model");
    assert_eq!(model.min_dim_level, Some(1));
    assert_eq!(model.nowc, Some(1));
    assert_eq!(model.cct_range, Some(vec![2200, 2700, 6500, 6500]));
}

#[test]
fn module_names_classify_in_order() {
    let cases = [
        (Some("ESP01_SHRGB1C_31"), ModuleClass::Rgb),
        (Some("ESP03_SHRGBP_31"), ModuleClass::Rgb),
        (Some("ESP20_DHRGB_01"), ModuleClass::Rgb),
        (Some("ESP25_SHORGB_01"), ModuleClass::Rgb),
        (Some("ESP15_SHTW1_01I"), ModuleClass::TunableWhite),
        (Some("ESP56_SHTW3_01"), ModuleClass::TunableWhite),
        (Some("ESP05_SHDW_21"), ModuleClass::DimmableWhite),
        (Some("ESP01_DIMTRIACS_01"), ModuleClass::DimmableWhite),
        (Some("ESP03_FANDIMS_31"), ModuleClass::DimmableWhite),
        (Some("ESP10_SOCKET_06"), ModuleClass::Socket),
        (Some(""), ModuleClass::DimmableWhite),
        (None, ModuleClass::DimmableWhite),
    ];
    for (name, expected) in cases {
        assert_eq!(ModuleClass::from_module_name(name), expected, "{name:?}");
    }
    assert!(!ModuleClass::Socket.is_light());
    assert!(ModuleClass::Rgb.color_capable());
    assert!(!ModuleClass::TunableWhite.color_capable());
}

#[test]
fn colors_split_into_chroma_and_dimming() {
    assert_eq!(
        Pilot::from_color([255, 0, 0], ModuleClass::Rgb, 10),
        Pilot::Color {
            r: 255,
            g: 0,
            b: 0,
            dimming: 100
        }
    );
    assert_eq!(
        Pilot::from_color([128, 64, 0], ModuleClass::Rgb, 10),
        Pilot::Color {
            r: 255,
            g: 128,
            b: 0,
            dimming: 50
        }
    );
    // 2 / 255 rounds to 1 %, raised to the device floor.
    assert_eq!(
        Pilot::from_color([2, 1, 0], ModuleClass::Rgb, 10),
        Pilot::Color {
            r: 255,
            g: 128,
            b: 0,
            dimming: 10
        }
    );
    assert_eq!(
        Pilot::from_color([2, 1, 0], ModuleClass::Rgb, 1),
        Pilot::Color {
            r: 255,
            g: 128,
            b: 0,
            dimming: 1
        }
    );
    assert_eq!(
        Pilot::from_color([1, 1, 1], ModuleClass::Rgb, 10),
        Pilot::Off
    );
    assert_eq!(
        Pilot::from_color([0, 0, 0], ModuleClass::Rgb, 10),
        Pilot::Off
    );
}

#[test]
fn white_lights_follow_luma() {
    assert_eq!(
        Pilot::from_color([255, 255, 255], ModuleClass::TunableWhite, 10),
        Pilot::White { dimming: 100 }
    );
    // Pure green carries 71.52 % of luma.
    assert_eq!(
        Pilot::from_color([0, 255, 0], ModuleClass::DimmableWhite, 10),
        Pilot::White { dimming: 72 }
    );
    assert_eq!(
        Pilot::from_color([0, 0, 20], ModuleClass::DimmableWhite, 10),
        Pilot::White { dimming: 10 }
    );
    assert_eq!(
        Pilot::from_color([0, 0, 0], ModuleClass::TunableWhite, 10),
        Pilot::Off
    );
}

#[test]
fn unsigned_set_pilot_matches_the_wire() {
    let red = Pilot::Color {
        r: 255,
        g: 0,
        b: 0,
        dimming: 100,
    };
    assert_eq!(
        encode_set_pilot(red, true, None),
        r#"{"method":"setPilot","params":{"state":true,"r":255,"g":0,"b":0,"dimming":100}}"#
    );
    assert_eq!(
        encode_set_pilot(red, false, None),
        r#"{"method":"setPilot","params":{"r":255,"g":0,"b":0,"dimming":100}}"#
    );
    assert_eq!(
        encode_set_pilot(Pilot::White { dimming: 40 }, false, None),
        r#"{"method":"setPilot","params":{"dimming":40}}"#
    );
    assert_eq!(
        encode_set_pilot(Pilot::Off, true, None),
        r#"{"method":"setPilot","params":{"state":false}}"#
    );
}

#[test]
fn signed_set_pilot_signs_the_compact_params() {
    let key = SigningKey::parse(KEY).expect("key");
    let red = Pilot::Color {
        r: 255,
        g: 0,
        b: 0,
        dimming: 100,
    };
    let request = encode_set_pilot(red, true, Some((&key, 1_730_000_000)));
    assert_eq!(
        request,
        r#"{"method":"setPilot","env":"pro","params":{"state":true,"r":255,"g":0,"b":0,"dimming":100,"sigTs":1730000000},"hmac":"lv1P36lNJKDafo7cIJYXrTNw+8eqew5V624gc67o2Ak="}"#
    );
    let parsed: serde_json::Value = serde_json::from_str(&request).expect("valid JSON");
    assert_eq!(parsed["params"]["sigTs"], 1_730_000_000);
}

#[test]
fn hmac_matches_rfc_4231() {
    let hex = |bytes: [u8; 32]| {
        bytes.iter().fold(String::new(), |mut text, byte| {
            use std::fmt::Write as _;
            let _ = write!(text, "{byte:02x}");
            text
        })
    };
    assert_eq!(
        hex(hmac_sha256(b"Jefe", b"what do ya want for nothing?")),
        "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
    );
    assert_eq!(
        hex(hmac_sha256(
            &[0xaa; 131],
            b"Test Using Larger Than Block-Size Key - Hash Key First"
        )),
        "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
    );
}

#[test]
fn signing_keys_parse_strictly_and_never_print() {
    assert!(SigningKey::parse(KEY).is_ok());
    assert!(SigningKey::parse(&KEY.to_ascii_uppercase()).is_ok());
    assert_eq!(
        SigningKey::parse("0011").err(),
        Some(ProtocolError::SigningKey)
    );
    assert!(SigningKey::parse("zz112233445566778899aabbccddeeff").is_err());
    let key = SigningKey::parse(KEY).expect("key");
    assert_eq!(format!("{key:?}"), "SigningKey(redacted)");
}

#[test]
fn macs_canonicalize() {
    assert_eq!(
        canonical_mac("A8BB5006033D").as_deref(),
        Some("a8:bb:50:06:03:3d")
    );
    assert_eq!(canonical_mac("a8bb50"), None);
}
