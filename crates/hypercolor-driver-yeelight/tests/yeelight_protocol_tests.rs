//! Golden bytes for the search and every command line, reply decoding, and
//! color to command conversion (Spec 94 §3 to §6).

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use hypercolor_driver_yeelight::protocol::{
    Capabilities, Command, DeviceError, LightState, ProtocolError, SEARCH_MULTICAST,
    SEARCH_REQUEST, SearchReply, commands_for, decode_message, decode_search_reply,
    encode_search_reply, refresh_commands, worst_case_commands,
};

fn support(methods: &str) -> Vec<String> {
    methods.split_whitespace().map(ToOwned::to_owned).collect()
}

fn color() -> Capabilities {
    Capabilities::from_support(&support(
        "get_prop set_power set_bright set_rgb set_scene set_music",
    ))
}

#[test]
fn search_request_matches_the_spec() {
    assert_eq!(
        SEARCH_REQUEST,
        "M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1982\r\nMAN: \"ssdp:discover\"\r\nST: wifi_bulb\r\n"
    );
    assert_eq!(
        SEARCH_MULTICAST,
        SocketAddr::from((Ipv4Addr::new(239, 255, 255, 250), 1982))
    );
}

#[test]
fn search_reply_decodes_headers_case_insensitively() {
    let datagram = "HTTP/1.1 200 OK\r\nCache-Control: max-age=3600\r\nDate:\r\nExt:\r\nLocation: yeelight://192.168.1.239:55443\r\nServer: POSIX UPnP/1.0 YGLC/1\r\nid: 0x000000000015243F\r\nmodel: color\r\nfw_ver: 18\r\nsupport: get_prop set_default set_power toggle set_bright start_cf stop_cf set_scene cron_add cron_get cron_del set_ct_abx set_rgb\r\npower: on\r\nbright: 100\r\ncolor_mode: 2\r\nct: 4000\r\nrgb: 16711680\r\nhue: 100\r\nsat: 35\r\nname: my_bulb\r\n";
    let reply = decode_search_reply(datagram.as_bytes()).expect("reply decodes");
    assert_eq!(
        reply.location,
        SocketAddr::from((Ipv4Addr::new(192, 168, 1, 239), 55443))
    );
    assert_eq!(reply.id, "0x000000000015243f");
    assert_eq!(reply.model.as_deref(), Some("color"));
    assert_eq!(reply.fw_ver.as_deref(), Some("18"));
    assert_eq!(reply.support.len(), 13);
    assert_eq!(reply.power.as_deref(), Some("on"));
    assert_eq!(reply.bright, Some(100));
    assert_eq!(reply.name.as_deref(), Some("my_bulb"));

    let shouting = datagram
        .replace("Location:", "LOCATION:")
        .replace("id:", "ID:");
    assert_eq!(
        decode_search_reply(shouting.as_bytes())
            .expect("reply decodes")
            .id,
        "0x000000000015243f"
    );
}

#[test]
fn notify_advertisements_decode_too() {
    let datagram = "NOTIFY * HTTP/1.1\r\nHost: 239.255.255.250:1982\r\nCache-Control: max-age=3600\r\nLocation: yeelight://10.0.0.5:55443\r\nNTS: ssdp:alive\r\nServer: POSIX, UPnP/1.0 YGLC/1\r\nid: 0x0000000002dfb19a\r\nmodel: stripe\r\nsupport: set_power set_bright\r\n";
    let reply = decode_search_reply(datagram.as_bytes()).expect("advertisement decodes");
    assert_eq!(reply.model.as_deref(), Some("stripe"));
    assert_eq!(reply.name, None);
}

#[test]
fn malformed_search_replies_are_rejected() {
    assert_eq!(decode_search_reply(b""), Err(ProtocolError::Empty));
    assert!(matches!(
        decode_search_reply(b"M-SEARCH * HTTP/1.1\r\nST: wifi_bulb\r\n"),
        Err(ProtocolError::Status(_))
    ));
    assert_eq!(
        decode_search_reply(b"HTTP/1.1 200 OK\r\nid: 0x1\r\n"),
        Err(ProtocolError::MissingHeader("Location"))
    );
    assert_eq!(
        decode_search_reply(b"HTTP/1.1 200 OK\r\nLocation: yeelight://10.0.0.5:55443\r\n"),
        Err(ProtocolError::MissingHeader("id"))
    );
    assert!(matches!(
        decode_search_reply(b"HTTP/1.1 200 OK\r\nLocation: http://10.0.0.5\r\nid: 0x1\r\n"),
        Err(ProtocolError::Location(_))
    ));
}

#[test]
fn search_reply_round_trips() {
    let reply = SearchReply {
        location: SocketAddr::from((Ipv4Addr::new(10, 1, 2, 3), 55443)),
        id: "0x00000000000000aa".to_owned(),
        model: Some("color4".to_owned()),
        fw_ver: Some("26".to_owned()),
        support: support("set_power set_scene set_rgb set_bright"),
        power: Some("off".to_owned()),
        bright: Some(42),
        name: Some("desk".to_owned()),
    };
    assert_eq!(
        decode_search_reply(encode_search_reply(&reply).as_bytes()).expect("round trip"),
        reply
    );
}

#[test]
fn commands_match_the_wire() {
    let host = IpAddr::V4(Ipv4Addr::new(192, 168, 0, 2));
    let cases = [
        (
            Command::PowerOn,
            r#"{"id":1,"method":"set_power","params":["on","sudden",30]}"#,
        ),
        (
            Command::PowerOff,
            r#"{"id":1,"method":"set_power","params":["off","sudden",30]}"#,
        ),
        (
            Command::SceneColor {
                rgb: 65_280,
                bright: 70,
            },
            r#"{"id":1,"method":"set_scene","params":["color",65280,70]}"#,
        ),
        (
            Command::SetRgb { rgb: 16_711_680 },
            r#"{"id":1,"method":"set_rgb","params":[16711680,"sudden",30]}"#,
        ),
        (
            Command::SetBright { bright: 1 },
            r#"{"id":1,"method":"set_bright","params":[1,"sudden",30]}"#,
        ),
        (
            Command::MusicStart { host, port: 54_321 },
            r#"{"id":1,"method":"set_music","params":[1,"192.168.0.2",54321]}"#,
        ),
        (
            Command::MusicStop,
            r#"{"id":1,"method":"set_music","params":[0]}"#,
        ),
    ];
    for (command, json) in cases {
        let line = command.encode(1);
        assert_eq!(line, format!("{json}\r\n"));
        serde_json::from_str::<serde_json::Value>(line.trim_end()).expect("valid JSON");
    }
    assert!(Command::PowerOn.encode(42).starts_with(r#"{"id":42,"#));
}

#[test]
fn replies_and_notifications_decode() {
    let ok = decode_message(br#"{"id":1,"result":["ok"]}"#).expect("result");
    assert_eq!(ok.id, Some(1));
    assert!(ok.error.is_none());

    let quota =
        decode_message(br#"{"id":7,"error":{"code":-1,"message":"client quota exceeded"}}"#)
            .expect("error");
    assert!(
        quota
            .error
            .as_ref()
            .is_some_and(DeviceError::is_quota_exceeded)
    );
    let unsupported =
        decode_message(br#"{"id":8,"error":{"code":-1,"message":"unsupported method"}}"#)
            .expect("error");
    assert!(
        !unsupported
            .error
            .as_ref()
            .is_some_and(DeviceError::is_quota_exceeded)
    );

    let props = decode_message(br#"{"method":"props","params":{"power":"on","bright":"10"}}"#)
        .expect("props");
    assert_eq!(props.method.as_deref(), Some("props"));
    assert!(decode_message(b"garbage").is_err());
}

#[test]
fn capabilities_come_from_the_support_list() {
    let caps = color();
    assert!(caps.color && caps.scene && caps.brightness && caps.music && caps.drivable());
    assert_eq!(caps.class_name(), "color");
    let mono = Capabilities::from_support(&support("get_prop set_power set_bright"));
    assert!(!mono.color && mono.drivable());
    assert_eq!(mono.class_name(), "white");
    assert!(!Capabilities::from_support(&support("get_prop set_power")).drivable());
}

#[test]
fn colors_split_into_chroma_and_brightness() {
    assert_eq!(
        LightState::from_color([128, 64, 0], color()),
        LightState::Color {
            rgb: 0x00ff_8000,
            bright: 50
        }
    );
    assert_eq!(
        LightState::from_color([0, 0, 255], color()),
        LightState::Color {
            rgb: 0x0000_00ff,
            bright: 100
        }
    );
    assert_eq!(LightState::from_color([1, 1, 1], color()), LightState::Off);
    let mono = Capabilities::from_support(&support("set_power set_bright"));
    assert_eq!(
        LightState::from_color([255, 255, 255], mono),
        LightState::White { bright: 100 }
    );
    assert_eq!(
        LightState::from_color([0, 255, 0], mono),
        LightState::White { bright: 72 }
    );
}

#[test]
fn scene_lights_take_one_command_per_frame() {
    let red = LightState::Color {
        rgb: 0x00ff_0000,
        bright: 100,
    };
    assert_eq!(
        commands_for(None, red, color()),
        [Command::SceneColor {
            rgb: 0x00ff_0000,
            bright: 100
        }]
    );
    assert!(commands_for(Some(red), red, color()).is_empty());
    assert_eq!(
        commands_for(Some(red), LightState::Off, color()),
        [Command::PowerOff]
    );
    assert_eq!(worst_case_commands(color()), 1);
    assert_eq!(
        refresh_commands(red, color()),
        [Command::SceneColor {
            rgb: 0x00ff_0000,
            bright: 100
        }]
    );
}

#[test]
fn lights_without_scenes_power_on_then_set_what_changed() {
    let caps = Capabilities::from_support(&support("set_power set_rgb set_bright"));
    let red = LightState::Color {
        rgb: 0x00ff_0000,
        bright: 100,
    };
    assert_eq!(
        commands_for(Some(LightState::Off), red, caps),
        [
            Command::PowerOn,
            Command::SetRgb { rgb: 0x00ff_0000 },
            Command::SetBright { bright: 100 }
        ]
    );
    let dimmer = LightState::Color {
        rgb: 0x00ff_0000,
        bright: 40,
    };
    assert_eq!(
        commands_for(Some(red), dimmer, caps),
        [Command::SetBright { bright: 40 }]
    );
    assert_eq!(worst_case_commands(caps), 3);

    let mono = Capabilities::from_support(&support("set_power set_bright"));
    assert_eq!(
        commands_for(None, LightState::White { bright: 30 }, mono),
        [Command::PowerOn, Command::SetBright { bright: 30 }]
    );
    assert_eq!(
        commands_for(
            Some(LightState::White { bright: 30 }),
            LightState::White { bright: 31 },
            mono
        ),
        [Command::SetBright { bright: 31 }]
    );
    assert_eq!(worst_case_commands(mono), 2);
}

#[test]
fn ids_must_be_the_specified_hex_identity() {
    use hypercolor_driver_yeelight::protocol::canonical_id;
    assert_eq!(
        canonical_id("0x000000000015243F").as_deref(),
        Ok("0x000000000015243f")
    );
    assert_eq!(canonical_id("0XABC").as_deref(), Ok("0xabc"));
    for bad in [
        "€abcd",
        "0x",
        "0x€",
        "15243f",
        "0x00000000000000001",
        "0xzz",
    ] {
        assert!(canonical_id(bad).is_err(), "{bad} must be refused");
    }
    let datagram = "HTTP/1.1 200 OK\r\nLocation: yeelight://10.0.0.5:55443\r\nid: \u{20ac}abcd\r\nsupport: set_bright\r\n";
    assert!(matches!(
        decode_search_reply(datagram.as_bytes()),
        Err(ProtocolError::InvalidId(_))
    ));
}

#[test]
fn display_names_never_slice_inside_a_character() {
    use hypercolor_driver_yeelight::YeelightDeviceProfile;
    let profile = |id: &str| YeelightDeviceProfile {
        reply: SearchReply {
            location: SocketAddr::from((Ipv4Addr::new(10, 0, 0, 5), 55443)),
            id: id.to_owned(),
            model: Some("mono".to_owned()),
            fw_ver: None,
            support: support("set_bright"),
            power: None,
            bright: None,
            name: None,
        },
        search_port: 1982,
    };
    assert_eq!(
        profile("0x000000000015243f").display_name(),
        "Yeelight mono 15243F"
    );
    // Not reachable through the decoder, but must not panic either.
    assert_eq!(
        profile("\u{20ac}abcd").display_name(),
        "Yeelight mono \u{20ac}ABCD"
    );
}

#[test]
fn long_names_and_models_are_truncated() {
    let long = "n".repeat(500);
    let datagram = format!(
        "HTTP/1.1 200 OK\r\nLocation: yeelight://10.0.0.5:55443\r\nid: 0x1\r\nmodel: {long}\r\nname: {long}\r\n"
    );
    let reply = decode_search_reply(datagram.as_bytes()).expect("reply decodes");
    assert_eq!(reply.name.map(|name| name.chars().count()), Some(64));
    assert_eq!(reply.model.map(|model| model.chars().count()), Some(64));
}
