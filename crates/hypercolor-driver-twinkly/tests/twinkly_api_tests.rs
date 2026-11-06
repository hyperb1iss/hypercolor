//! Golden JSON for REST requests, reply decoding, and layout mapping
//! (Spec 92 §4, §5, and §7).

use std::time::Duration;

use hypercolor_driver_twinkly::api::{
    ApiError, AuthToken, Gestalt, LayoutCoordinate, LayoutReply, LoginReply, decode_reply,
    login_request_body, mode_request_body, verify_request_body,
};
use hypercolor_driver_twinkly::layout::{LayoutSource, normalized_positions, segments_for};
use hypercolor_types::device::DeviceTopologyHint;
use hypercolor_types::spatial::LedTopology;

#[test]
fn login_body_carries_base64_challenge() {
    let challenge: [u8; 32] = std::array::from_fn(|index| index as u8);
    assert_eq!(
        login_request_body(&challenge),
        serde_json::json!({ "challenge": "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=" })
    );
}

#[test]
fn verify_and_mode_bodies_match_the_wire() {
    assert_eq!(
        verify_request_body("8d87f080947e343180da3f411df3997e3e9ae0cc").to_string(),
        r#"{"challenge-response":"8d87f080947e343180da3f411df3997e3e9ae0cc"}"#
    );
    assert_eq!(mode_request_body("rt").to_string(), r#"{"mode":"rt"}"#);
}

#[test]
fn login_reply_decodes_hyphenated_field() {
    let reply: LoginReply = decode_reply(
        "login",
        br#"{"authentication_token":"5jPe+ONhwUY=","authentication_token_expires_in":14400,"challenge-response":"abc","code":1000}"#,
    )
    .expect("login reply");
    assert_eq!(reply.authentication_token, "5jPe+ONhwUY=");
    assert_eq!(reply.authentication_token_expires_in, Some(14_400));
    assert_eq!(reply.challenge_response, "abc");

    let token = AuthToken::decode(
        &reply.authentication_token,
        Some(Duration::from_secs(14_400)),
    )
    .expect("token decodes");
    assert_eq!(
        token.bytes,
        [0xe6, 0x33, 0xde, 0xf8, 0xe3, 0x61, 0xc1, 0x46]
    );
    assert!(AuthToken::decode("AAEC", None).is_err());
    assert!(AuthToken::decode("not base64!", None).is_err());
}

#[test]
fn error_codes_surface_as_errors() {
    let error = decode_reply::<serde_json::Value>("led/mode", br#"{"code":1104}"#)
        .expect_err("non-1000 code fails");
    assert!(matches!(error, ApiError::Code { code: 1104, .. }));
    assert!(decode_reply::<serde_json::Value>("x", b"Invalid Token.").is_err());
    assert!(decode_reply::<serde_json::Value>("x", br#"{"mode":"rt"}"#).is_ok());
}

#[test]
fn gestalt_decodes_generation_one_and_two_shapes() {
    let generation_one: Gestalt = decode_reply(
        "gestalt",
        br#"{"product_name":"Twinkly","product_version":"2","hardware_version":"6","flash_size":16,"led_type":6,"led_version":"1","product_code":"TW105SEUP06","device_name":"Twinkly_33AAFF","uptime":"60","hw_id":"0033aaff","mac":"5c:cf:7f:33:aa:ff","max_supported_led":224,"base_leds_number":105,"number_of_led":105,"led_profile":"RGB","frame_rate":25,"movie_capacity":719,"copyright":"LEDWORKS 2017","code":1000}"#,
    )
    .expect("generation I gestalt");
    assert_eq!(generation_one.fw_family, None);
    assert_eq!(generation_one.number_of_led, 105);
    assert_eq!(generation_one.frame_rate, Some(25.0));

    let generation_two: Gestalt = decode_reply(
        "gestalt",
        br#"{"product_name":"Twinkly","hardware_version":"100","bytes_per_led":4,"product_code":"TWI190SPP","fw_family":"G","device_name":"Twinkly_1CC190","mac":"98:f4:ab:1c:c1:90","number_of_led":190,"led_profile":"RGBW","frame_rate":28.57,"code":1000}"#,
    )
    .expect("generation II gestalt");
    assert_eq!(generation_two.fw_family.as_deref(), Some("G"));
    assert_eq!(generation_two.bytes_per_led, Some(4));
    assert_eq!(generation_two.led_profile.as_deref(), Some("RGBW"));
}

fn coordinate(x: f64, y: f64, z: f64) -> LayoutCoordinate {
    LayoutCoordinate { x, y, z }
}

#[test]
fn positions_fill_the_zone_with_y_pointing_down() {
    let positions = normalized_positions(&[
        coordinate(-1.0, 0.0, 0.3),
        coordinate(1.0, 1.0, -0.2),
        coordinate(0.0, 0.5, 0.0),
    ]);
    let pairs: Vec<(f32, f32)> = positions.iter().map(|p| (p.x, p.y)).collect();
    assert_eq!(pairs, [(0.0, 1.0), (1.0, 0.0), (0.5, 0.5)]);
}

#[test]
fn flat_axis_centers_instead_of_dividing_by_zero() {
    let positions = normalized_positions(&[coordinate(0.2, 0.0, 0.0), coordinate(0.2, 1.0, 0.0)]);
    assert!(positions.iter().all(|p| p.x == 0.5));
    assert_eq!(positions[0].y, 1.0);
    assert_eq!(positions[1].y, 0.0);
}

fn layout(source: &str, coordinates: Vec<LayoutCoordinate>) -> LayoutReply {
    LayoutReply {
        source: Some(source.to_owned()),
        synthesized: Some(false),
        coordinates,
        code: Some(1000),
    }
}

#[test]
fn mapped_layout_becomes_custom_positions() {
    let reply = layout(
        "3d",
        vec![coordinate(0.0, 0.0, 1.0), coordinate(1.0, 1.0, -1.0)],
    );
    let (segments, source) = segments_for(2, Some(&reply));
    assert_eq!(source, LayoutSource::ThreeD);
    assert_eq!(segments.len(), 1);
    assert_eq!(segments[0].led_count, 2);
    assert_eq!(segments[0].topology, DeviceTopologyHint::Custom);
    let Some(LedTopology::Custom { positions }) = segments[0]
        .layout_hint
        .as_ref()
        .and_then(|hint| hint.topology.clone())
    else {
        panic!("expected custom positions");
    };
    assert_eq!(positions.len(), 2);
}

#[test]
fn linear_or_mismatched_layouts_stay_strips() {
    let linear = layout("linear", vec![coordinate(0.0, 0.0, 0.0); 3]);
    let (segments, source) = segments_for(3, Some(&linear));
    assert_eq!(source, LayoutSource::Linear);
    assert_eq!(segments[0].topology, DeviceTopologyHint::Strip);
    assert!(segments[0].layout_hint.is_none());

    let short = layout("2d", vec![coordinate(0.0, 0.0, 0.0); 2]);
    let (segments, source) = segments_for(3, Some(&short));
    assert_eq!(source, LayoutSource::TwoD);
    assert_eq!(segments[0].topology, DeviceTopologyHint::Strip);
    assert_eq!(segments[0].led_count, 3);

    let (segments, source) = segments_for(250, None);
    assert_eq!(source, LayoutSource::Unavailable);
    assert_eq!(segments[0].led_count, 250);
}
