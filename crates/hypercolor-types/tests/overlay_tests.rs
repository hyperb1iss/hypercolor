use hypercolor_types::overlay::{
    Anchor, ClockConfig, ClockStyle, DisplayOverlayConfig, HourFormat, HtmlOverlayConfig,
    OverlayBlendMode, OverlayPosition, OverlaySlot, OverlaySlotId, OverlaySource, TextAlign,
    TextOverlayConfig,
};
use uuid::Uuid;

#[test]
fn display_overlay_config_round_trips_through_json() {
    let config = DisplayOverlayConfig {
        overlays: vec![OverlaySlot {
            id: OverlaySlotId::from(Uuid::now_v7()),
            name: "Clock".to_owned(),
            source: OverlaySource::Clock(ClockConfig {
                style: ClockStyle::Digital,
                hour_format: HourFormat::TwentyFour,
                show_seconds: true,
                show_date: true,
                date_format: Some("%Y-%m-%d".to_owned()),
                font_family: Some("Orbitron".to_owned()),
                color: "#80ffea".to_owned(),
                secondary_color: Some("#ff6ac1".to_owned()),
                template: None,
            }),
            position: OverlayPosition::Anchored {
                anchor: Anchor::TopRight,
                offset_x: -12,
                offset_y: 8,
                width: 120,
                height: 60,
            },
            blend_mode: OverlayBlendMode::Screen,
            opacity: 0.75,
            enabled: true,
        }],
    };

    let json = serde_json::to_string(&config).expect("serialize overlay config");
    let restored: DisplayOverlayConfig =
        serde_json::from_str(&json).expect("deserialize overlay config");
    assert_eq!(restored, config);
}

#[test]
fn normalized_overlay_config_clamps_and_trims() {
    let config = DisplayOverlayConfig {
        overlays: vec![OverlaySlot {
            id: OverlaySlotId::from(Uuid::nil()),
            name: "   ".to_owned(),
            source: OverlaySource::Text(TextOverlayConfig {
                text: "   ".to_owned(),
                font_family: Some("  ".to_owned()),
                font_size: 0.0,
                color: "  ".to_owned(),
                align: TextAlign::Center,
                scroll: true,
                scroll_speed: -4.0,
            }),
            position: OverlayPosition::Anchored {
                anchor: Anchor::BottomLeft,
                offset_x: 0,
                offset_y: 0,
                width: 0,
                height: 0,
            },
            blend_mode: OverlayBlendMode::Normal,
            opacity: 5.0,
            enabled: true,
        }],
    }
    .normalized();

    let slot = &config.overlays[0];
    assert_eq!(slot.name, "Overlay");
    assert!((slot.opacity - 1.0).abs() < f32::EPSILON);
    assert!(matches!(
        slot.position,
        OverlayPosition::Anchored {
            width: 1,
            height: 1,
            ..
        }
    ));

    let OverlaySource::Text(text) = &slot.source else {
        panic!("expected text overlay");
    };
    assert_eq!(text.text, "Overlay");
    assert_eq!(text.font_family, None);
    assert_eq!(text.font_size, 1.0);
    assert_eq!(text.color, "#ffffff");
    assert_eq!(text.scroll_speed, 1.0);
}

#[test]
fn html_overlay_normalization_enforces_minimum_interval() {
    let source = OverlaySource::Html(HtmlOverlayConfig {
        path: "  /tmp/test.html  ".to_owned(),
        properties: Default::default(),
        render_interval_ms: 1,
    })
    .normalized();

    let OverlaySource::Html(config) = source else {
        panic!("expected html overlay");
    };
    assert_eq!(config.path, "/tmp/test.html");
    assert_eq!(config.render_interval_ms, 16);
}
