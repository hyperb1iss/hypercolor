use hypercolor_driver_hue::{
    HueChannel, HueChannelMember, HueEntertainmentConfig, HueEntertainmentType, HueLight,
    HuePosition, build_device_info,
};

#[test]
fn build_device_info_prefers_area_and_light_names_for_hue() {
    let info = build_device_info(
        "bridge-1",
        "Studio Bridge",
        Some("BSB002"),
        Some("1.0.0"),
        Some(&HueEntertainmentConfig {
            id: "area-1".to_owned(),
            name: "Movie Time".to_owned(),
            config_type: HueEntertainmentType::Screen,
            channels: vec![
                HueChannel {
                    id: 0,
                    name: "Channel 0".to_owned(),
                    position: HuePosition::default(),
                    segment_count: 1,
                    members: vec![HueChannelMember {
                        id: "member-1".to_owned(),
                        light_id: Some("light-left".to_owned()),
                    }],
                },
                HueChannel {
                    id: 1,
                    name: "Channel 1".to_owned(),
                    position: HuePosition::default(),
                    segment_count: 1,
                    members: vec![HueChannelMember {
                        id: "member-2".to_owned(),
                        light_id: Some("light-right".to_owned()),
                    }],
                },
            ],
        }),
        &[
            HueLight {
                id: "light-left".to_owned(),
                name: "Left Lamp".to_owned(),
                model_id: None,
                gamut_type: None,
                gamut: None,
            },
            HueLight {
                id: "light-right".to_owned(),
                name: "Right Lamp".to_owned(),
                model_id: None,
                gamut_type: None,
                gamut: None,
            },
        ],
    );

    assert_eq!(info.name, "Movie Time");
    assert_eq!(info.zones[0].name, "Left Lamp");
    assert_eq!(info.zones[1].name, "Right Lamp");
}
