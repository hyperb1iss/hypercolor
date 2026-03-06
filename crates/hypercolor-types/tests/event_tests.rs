use std::collections::HashMap;

use hypercolor_types::event::{
    ChangeTrigger, ContextType, DisconnectReason, EffectRef, EffectStopReason, EventCategory,
    EventControlValue, EventPriority, FrameData, FrameTiming, HypercolorEvent, Severity,
    TransitionRef, ZoneColors, ZoneRef,
};
use hypercolor_types::session::SessionEvent;

// ── Category Tests ──────────────────────────────────────────────────────

#[test]
fn device_events_have_device_category() {
    let events = vec![
        HypercolorEvent::DeviceDiscovered {
            device_id: "d1".into(),
            name: "Strip".into(),
            backend: "wled".into(),
            led_count: 60,
            address: Some("192.168.1.100".into()),
        },
        HypercolorEvent::DeviceConnected {
            device_id: "d1".into(),
            name: "Strip".into(),
            backend: "wled".into(),
            led_count: 60,
            zones: vec![],
        },
        HypercolorEvent::DeviceDisconnected {
            device_id: "d1".into(),
            reason: DisconnectReason::Timeout,
            will_retry: true,
        },
        HypercolorEvent::DeviceError {
            device_id: "d1".into(),
            error: "connection lost".into(),
            recoverable: true,
        },
        HypercolorEvent::DeviceFirmwareInfo {
            device_id: "d1".into(),
            firmware_version: Some("0.14.0".into()),
            hardware_version: None,
            manufacturer: None,
            model: None,
            extra: HashMap::new(),
        },
        HypercolorEvent::DeviceStateChanged {
            device_id: "d1".into(),
            changes: HashMap::new(),
        },
        HypercolorEvent::DeviceDiscoveryStarted {
            backends: vec!["wled".into()],
        },
        HypercolorEvent::DeviceDiscoveryCompleted {
            found: vec![],
            duration_ms: 1500,
        },
    ];

    for event in &events {
        assert_eq!(
            event.category(),
            EventCategory::Device,
            "Expected Device category for {event:?}"
        );
    }
}

#[test]
fn effect_events_have_effect_category() {
    let effect_ref = EffectRef {
        id: "rainbow".into(),
        name: "Rainbow Wave".into(),
        engine: "wgpu".into(),
    };

    let events = vec![
        HypercolorEvent::EffectStarted {
            effect: effect_ref.clone(),
            trigger: ChangeTrigger::User,
            previous: None,
            transition: None,
        },
        HypercolorEvent::EffectStopped {
            effect: effect_ref,
            reason: EffectStopReason::Replaced,
        },
        HypercolorEvent::EffectControlChanged {
            effect_id: "rainbow".into(),
            control_id: "speed".into(),
            old_value: EventControlValue::Number(0.5),
            new_value: EventControlValue::Number(0.8),
            trigger: ChangeTrigger::Api,
        },
        HypercolorEvent::EffectLayerAdded {
            layer_id: "l1".into(),
            effect: EffectRef {
                id: "fire".into(),
                name: "Fire".into(),
                engine: "wgpu".into(),
            },
            index: 0,
            blend_mode: "add".into(),
            opacity: 1.0,
        },
        HypercolorEvent::EffectLayerRemoved {
            layer_id: "l1".into(),
            effect_id: "fire".into(),
        },
        HypercolorEvent::EffectError {
            effect_id: "broken".into(),
            error: "shader compile failed".into(),
            fallback: Some("solid_black".into()),
        },
    ];

    for event in &events {
        assert_eq!(
            event.category(),
            EventCategory::Effect,
            "Expected Effect category for {event:?}"
        );
    }
}

#[test]
fn scene_events_have_scene_category() {
    let events = vec![
        HypercolorEvent::SceneActivated {
            scene_id: "s1".into(),
            scene_name: "Gaming".into(),
            trigger_type: "manual".into(),
            profile_id: "p1".into(),
        },
        HypercolorEvent::SceneTransitionStarted {
            scene_id: "s1".into(),
            from_profile: None,
            to_profile: "p1".into(),
            duration_ms: 500,
        },
        HypercolorEvent::SceneTransitionComplete {
            scene_id: "s1".into(),
            profile_id: "p1".into(),
        },
        HypercolorEvent::SceneEnabled {
            scene_id: "s1".into(),
            enabled: true,
        },
    ];

    for event in &events {
        assert_eq!(
            event.category(),
            EventCategory::Scene,
            "Expected Scene category for {event:?}"
        );
    }
}

#[test]
fn audio_events_have_audio_category() {
    let events = vec![
        HypercolorEvent::AudioSourceChanged {
            previous: None,
            current: "default_sink".into(),
            sample_rate: 48000,
        },
        HypercolorEvent::BeatDetected {
            confidence: 0.95,
            bpm: Some(128.0),
            phase: 0.0,
        },
        HypercolorEvent::AudioLevelUpdate {
            level: 0.7,
            bass: 0.9,
            mid: 0.5,
            treble: 0.3,
            beat: true,
        },
        HypercolorEvent::AudioStarted {
            source_name: "pipewire".into(),
            sample_rate: 44100,
        },
        HypercolorEvent::AudioStopped {
            reason: "source removed".into(),
        },
    ];

    for event in &events {
        assert_eq!(
            event.category(),
            EventCategory::Audio,
            "Expected Audio category for {event:?}"
        );
    }
}

#[test]
fn system_events_have_system_category() {
    let events = vec![
        HypercolorEvent::FrameRendered {
            frame_number: 42,
            timing: FrameTiming {
                render_us: 1000,
                sample_us: 200,
                push_us: 500,
                total_us: 2000,
                budget_us: 16666,
            },
        },
        HypercolorEvent::FpsChanged {
            old_target: 60,
            new_target: 30,
            measured: 59.8,
        },
        HypercolorEvent::ProfileLoaded {
            profile_id: "p1".into(),
            profile_name: "Gaming".into(),
            trigger: ChangeTrigger::User,
        },
        HypercolorEvent::ProfileSaved {
            profile_id: "p1".into(),
            profile_name: "Gaming".into(),
            is_new: true,
        },
        HypercolorEvent::ProfileDeleted {
            profile_id: "p1".into(),
        },
        HypercolorEvent::ConfigChanged {
            key: "daemon.fps".into(),
            old_value: Some(serde_json::json!(60)),
            new_value: serde_json::json!(30),
        },
        HypercolorEvent::ShutdownRequested {
            source: "signal".into(),
            grace_period_secs: 5,
        },
        HypercolorEvent::DaemonStarted {
            version: "0.1.0".into(),
            pid: 1234,
            device_count: 3,
            effect_count: 12,
        },
        HypercolorEvent::DaemonShutdown {
            reason: "user".into(),
        },
        HypercolorEvent::BrightnessChanged {
            old: 100,
            new_value: 50,
        },
        HypercolorEvent::Paused,
        HypercolorEvent::Resumed,
        HypercolorEvent::SessionChanged(SessionEvent::ScreenLocked),
        HypercolorEvent::Error {
            code: "E001".into(),
            message: "out of memory".into(),
            severity: Severity::Error,
        },
    ];

    for event in &events {
        assert_eq!(
            event.category(),
            EventCategory::System,
            "Expected System category for {event:?}"
        );
    }
}

#[test]
fn automation_events_have_automation_category() {
    let events = vec![
        HypercolorEvent::TriggerFired {
            trigger_id: "t1".into(),
            scene_id: "s1".into(),
            trigger_type: "schedule".into(),
            payload: serde_json::json!({"cron": "0 20 * * *"}),
        },
        HypercolorEvent::ScheduleActivated {
            scene_id: "s1".into(),
            scene_name: "Evening".into(),
            schedule_expr: "0 20 * * *".into(),
            profile_id: "p2".into(),
        },
        HypercolorEvent::ContextChanged {
            context_type: ContextType::TimeOfDay,
            previous: Some("afternoon".into()),
            current: "evening".into(),
        },
    ];

    for event in &events {
        assert_eq!(
            event.category(),
            EventCategory::Automation,
            "Expected Automation category for {event:?}"
        );
    }
}

#[test]
fn layout_events_have_layout_category() {
    let events = vec![
        HypercolorEvent::LayoutChanged {
            previous: None,
            current: "desk_setup".into(),
        },
        HypercolorEvent::LayoutZoneAdded {
            layout_id: "desk_setup".into(),
            zone: ZoneRef {
                zone_id: "z1".into(),
                device_id: "d1".into(),
                topology: "linear".into(),
                led_count: 60,
            },
        },
        HypercolorEvent::LayoutZoneRemoved {
            layout_id: "desk_setup".into(),
            zone_id: "z1".into(),
            device_id: "d1".into(),
        },
        HypercolorEvent::LayoutUpdated {
            layout_id: "desk_setup".into(),
        },
    ];

    for event in &events {
        assert_eq!(
            event.category(),
            EventCategory::Layout,
            "Expected Layout category for {event:?}"
        );
    }
}

#[test]
fn input_events_have_input_category() {
    let events = vec![
        HypercolorEvent::CaptureStarted {
            source_name: "screen0".into(),
            resolution: (1920, 1080),
        },
        HypercolorEvent::CaptureStopped {
            reason: "user request".into(),
        },
        HypercolorEvent::InputSourceChanged {
            input_id: "mic1".into(),
            input_type: "audio".into(),
            enabled: true,
        },
    ];

    for event in &events {
        assert_eq!(
            event.category(),
            EventCategory::Input,
            "Expected Input category for {event:?}"
        );
    }
}

#[test]
fn integration_events_have_integration_category() {
    let event = HypercolorEvent::WebhookReceived {
        webhook_id: "wh1".into(),
        source: "home_assistant".into(),
    };
    assert_eq!(event.category(), EventCategory::Integration);
}

// ── Priority Tests ──────────────────────────────────────────────────────

#[test]
fn critical_priority_events() {
    let events = vec![
        HypercolorEvent::DaemonShutdown {
            reason: "signal".into(),
        },
        HypercolorEvent::ShutdownRequested {
            source: "user".into(),
            grace_period_secs: 0,
        },
        HypercolorEvent::Error {
            code: "E999".into(),
            message: "fatal".into(),
            severity: Severity::Critical,
        },
    ];

    for event in &events {
        assert_eq!(
            event.priority(),
            EventPriority::Critical,
            "Expected Critical priority for {event:?}"
        );
    }
}

#[test]
fn high_priority_events() {
    let events = vec![
        HypercolorEvent::DeviceConnected {
            device_id: "d1".into(),
            name: "Strip".into(),
            backend: "wled".into(),
            led_count: 60,
            zones: vec![],
        },
        HypercolorEvent::DeviceDisconnected {
            device_id: "d1".into(),
            reason: DisconnectReason::Error,
            will_retry: true,
        },
        HypercolorEvent::DeviceError {
            device_id: "d1".into(),
            error: "timeout".into(),
            recoverable: false,
        },
    ];

    for event in &events {
        assert_eq!(
            event.priority(),
            EventPriority::High,
            "Expected High priority for {event:?}"
        );
    }
}

#[test]
fn low_priority_events() {
    let events = vec![
        HypercolorEvent::BeatDetected {
            confidence: 0.8,
            bpm: Some(120.0),
            phase: 0.5,
        },
        HypercolorEvent::AudioLevelUpdate {
            level: 0.5,
            bass: 0.6,
            mid: 0.4,
            treble: 0.2,
            beat: false,
        },
        HypercolorEvent::FrameRendered {
            frame_number: 1,
            timing: FrameTiming {
                render_us: 800,
                sample_us: 100,
                push_us: 300,
                total_us: 1500,
                budget_us: 16666,
            },
        },
        HypercolorEvent::DeviceDiscoveryCompleted {
            found: vec![],
            duration_ms: 500,
        },
        HypercolorEvent::LayoutUpdated {
            layout_id: "main".into(),
        },
        HypercolorEvent::WebhookReceived {
            webhook_id: "wh1".into(),
            source: "ha".into(),
        },
    ];

    for event in &events {
        assert_eq!(
            event.priority(),
            EventPriority::Low,
            "Expected Low priority for {event:?}"
        );
    }
}

#[test]
fn normal_priority_is_default() {
    let events = vec![
        HypercolorEvent::EffectStarted {
            effect: EffectRef {
                id: "e1".into(),
                name: "Test".into(),
                engine: "wgpu".into(),
            },
            trigger: ChangeTrigger::User,
            previous: None,
            transition: None,
        },
        HypercolorEvent::SceneActivated {
            scene_id: "s1".into(),
            scene_name: "Test".into(),
            trigger_type: "manual".into(),
            profile_id: "p1".into(),
        },
        HypercolorEvent::Paused,
        HypercolorEvent::Resumed,
        HypercolorEvent::ConfigChanged {
            key: "daemon.fps".into(),
            old_value: None,
            new_value: serde_json::json!(60),
        },
    ];

    for event in &events {
        assert_eq!(
            event.priority(),
            EventPriority::Normal,
            "Expected Normal priority for {event:?}"
        );
    }
}

#[test]
fn non_critical_error_has_normal_priority() {
    let warning = HypercolorEvent::Error {
        code: "W001".into(),
        message: "slow frame".into(),
        severity: Severity::Warning,
    };
    assert_eq!(warning.priority(), EventPriority::Normal);

    let error = HypercolorEvent::Error {
        code: "E001".into(),
        message: "device timeout".into(),
        severity: Severity::Error,
    };
    assert_eq!(error.priority(), EventPriority::Normal);
}

// ── Priority Ordering ───────────────────────────────────────────────────

#[test]
fn priority_ordering() {
    assert!(EventPriority::Low < EventPriority::Normal);
    assert!(EventPriority::Normal < EventPriority::High);
    assert!(EventPriority::High < EventPriority::Critical);
}

// ── Serialization Tests ─────────────────────────────────────────────────

#[test]
fn serialize_unit_variant_roundtrip() {
    let event = HypercolorEvent::Paused;
    let json = serde_json::to_string(&event).expect("serialize Paused");
    let deserialized: HypercolorEvent = serde_json::from_str(&json).expect("deserialize Paused");

    assert!(matches!(deserialized, HypercolorEvent::Paused));
}

#[test]
fn serialize_resumed_roundtrip() {
    let event = HypercolorEvent::Resumed;
    let json = serde_json::to_string(&event).expect("serialize Resumed");
    let deserialized: HypercolorEvent = serde_json::from_str(&json).expect("deserialize Resumed");

    assert!(matches!(deserialized, HypercolorEvent::Resumed));
}

#[test]
fn serialize_session_changed_roundtrip() {
    let event = HypercolorEvent::SessionChanged(SessionEvent::IdleEntered {
        idle_duration: std::time::Duration::from_secs(120),
    });
    let json = serde_json::to_string(&event).expect("serialize SessionChanged");
    let deserialized: HypercolorEvent =
        serde_json::from_str(&json).expect("deserialize SessionChanged");

    assert!(matches!(
        deserialized,
        HypercolorEvent::SessionChanged(SessionEvent::IdleEntered { .. })
    ));
}

#[test]
fn serialize_device_discovered_roundtrip() {
    let event = HypercolorEvent::DeviceDiscovered {
        device_id: "wled_001".into(),
        name: "Desk Strip".into(),
        backend: "wled".into(),
        led_count: 144,
        address: Some("192.168.1.42".into()),
    };

    let json = serde_json::to_string_pretty(&event).expect("serialize");
    let deserialized: HypercolorEvent = serde_json::from_str(&json).expect("deserialize");

    if let HypercolorEvent::DeviceDiscovered {
        device_id,
        name,
        backend,
        led_count,
        address,
    } = deserialized
    {
        assert_eq!(device_id, "wled_001");
        assert_eq!(name, "Desk Strip");
        assert_eq!(backend, "wled");
        assert_eq!(led_count, 144);
        assert_eq!(address, Some("192.168.1.42".into()));
    } else {
        panic!("Expected DeviceDiscovered variant");
    }
}

#[test]
fn serialize_effect_started_with_transition() {
    let event = HypercolorEvent::EffectStarted {
        effect: EffectRef {
            id: "rainbow_wave".into(),
            name: "Rainbow Wave".into(),
            engine: "wgpu".into(),
        },
        trigger: ChangeTrigger::Scene,
        previous: Some(EffectRef {
            id: "solid_blue".into(),
            name: "Solid Blue".into(),
            engine: "wgpu".into(),
        }),
        transition: Some(TransitionRef {
            transition_type: "crossfade".into(),
            duration_ms: 1000,
        }),
    };

    let json = serde_json::to_string(&event).expect("serialize");
    let deserialized: HypercolorEvent = serde_json::from_str(&json).expect("deserialize");

    if let HypercolorEvent::EffectStarted {
        effect,
        trigger,
        previous,
        transition,
    } = deserialized
    {
        assert_eq!(effect.id, "rainbow_wave");
        assert_eq!(trigger, ChangeTrigger::Scene);
        assert!(previous.is_some());
        assert!(transition.is_some());
        let t = transition.expect("transition present");
        assert_eq!(t.transition_type, "crossfade");
        assert_eq!(t.duration_ms, 1000);
    } else {
        panic!("Expected EffectStarted variant");
    }
}

#[test]
fn serialize_beat_detected_roundtrip() {
    let event = HypercolorEvent::BeatDetected {
        confidence: 0.92,
        bpm: Some(140.0),
        phase: 0.25,
    };

    let json = serde_json::to_string(&event).expect("serialize");
    let deserialized: HypercolorEvent = serde_json::from_str(&json).expect("deserialize");

    if let HypercolorEvent::BeatDetected {
        confidence,
        bpm,
        phase,
    } = deserialized
    {
        assert!((confidence - 0.92).abs() < f32::EPSILON);
        assert_eq!(bpm, Some(140.0));
        assert!((phase - 0.25).abs() < f32::EPSILON);
    } else {
        panic!("Expected BeatDetected variant");
    }
}

#[test]
fn serialize_config_changed_with_json_values() {
    let event = HypercolorEvent::ConfigChanged {
        key: "audio.gain".into(),
        old_value: Some(serde_json::json!(1.0)),
        new_value: serde_json::json!(1.5),
    };

    let json = serde_json::to_string(&event).expect("serialize");
    let deserialized: HypercolorEvent = serde_json::from_str(&json).expect("deserialize");

    if let HypercolorEvent::ConfigChanged {
        key,
        old_value,
        new_value,
    } = deserialized
    {
        assert_eq!(key, "audio.gain");
        assert!(old_value.is_some());
        assert_eq!(new_value, serde_json::json!(1.5));
    } else {
        panic!("Expected ConfigChanged variant");
    }
}

#[test]
fn serde_tagged_format() {
    let event = HypercolorEvent::DaemonStarted {
        version: "0.1.0".into(),
        pid: 42,
        device_count: 2,
        effect_count: 10,
    };

    let value: serde_json::Value = serde_json::to_value(&event).expect("to_value");
    assert_eq!(value["type"], "DaemonStarted");
    assert_eq!(value["data"]["version"], "0.1.0");
    assert_eq!(value["data"]["pid"], 42);
}

// ── ControlValue Tests ──────────────────────────────────────────────────

#[test]
fn control_value_number_roundtrip() {
    let val = EventControlValue::Number(0.75);
    let json = serde_json::to_string(&val).expect("serialize");
    let deserialized: EventControlValue = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(deserialized, EventControlValue::Number(0.75));
}

#[test]
fn control_value_boolean_roundtrip() {
    let val = EventControlValue::Boolean(true);
    let json = serde_json::to_string(&val).expect("serialize");
    let deserialized: EventControlValue = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(deserialized, EventControlValue::Boolean(true));
}

#[test]
fn control_value_string_roundtrip() {
    let val = EventControlValue::String("rainbow".into());
    let json = serde_json::to_string(&val).expect("serialize");
    let deserialized: EventControlValue = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(deserialized, EventControlValue::String("rainbow".into()));
}

// ── FrameData Tests ─────────────────────────────────────────────────────

#[test]
fn frame_data_empty() {
    let frame = FrameData::empty();
    assert_eq!(frame.frame_number, 0);
    assert_eq!(frame.timestamp_ms, 0);
    assert!(frame.zones.is_empty());
    assert_eq!(frame.total_leds(), 0);
}

#[test]
fn frame_data_total_leds() {
    let frame = FrameData::new(
        vec![
            ZoneColors {
                zone_id: "strip_1:zone_0".into(),
                colors: vec![[255, 0, 0]; 30],
            },
            ZoneColors {
                zone_id: "strip_1:zone_1".into(),
                colors: vec![[0, 255, 0]; 60],
            },
            ZoneColors {
                zone_id: "strip_2:zone_0".into(),
                colors: vec![[0, 0, 255]; 10],
            },
        ],
        1,
        16,
    );

    assert_eq!(frame.total_leds(), 100);
    assert_eq!(frame.frame_number, 1);
    assert_eq!(frame.timestamp_ms, 16);
    assert_eq!(frame.zones.len(), 3);
}

// ── Supporting Type Serde Tests ─────────────────────────────────────────

#[test]
fn disconnect_reason_serde() {
    let reasons = vec![
        DisconnectReason::Removed,
        DisconnectReason::Error,
        DisconnectReason::Timeout,
        DisconnectReason::Shutdown,
        DisconnectReason::User,
    ];

    for reason in &reasons {
        let json = serde_json::to_string(reason).expect("serialize");
        let deserialized: DisconnectReason = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(&deserialized, reason);
    }
}

#[test]
fn effect_stop_reason_serde() {
    let reasons = vec![
        EffectStopReason::Replaced,
        EffectStopReason::Stopped,
        EffectStopReason::Error,
        EffectStopReason::Paused,
        EffectStopReason::Shutdown,
    ];

    for reason in &reasons {
        let json = serde_json::to_string(reason).expect("serialize");
        let deserialized: EffectStopReason = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(&deserialized, reason);
    }
}

#[test]
fn change_trigger_serde() {
    let triggers = vec![
        ChangeTrigger::User,
        ChangeTrigger::Profile,
        ChangeTrigger::Scene,
        ChangeTrigger::Api,
        ChangeTrigger::Cli,
        ChangeTrigger::Mcp,
        ChangeTrigger::Dbus,
        ChangeTrigger::Webhook,
        ChangeTrigger::System,
    ];

    for trigger in &triggers {
        let json = serde_json::to_string(trigger).expect("serialize");
        let deserialized: ChangeTrigger = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(&deserialized, trigger);
    }
}

#[test]
fn context_type_serde() {
    let types = vec![
        ContextType::TimeOfDay,
        ContextType::ActiveWindow,
        ContextType::IdleState,
        ContextType::Presence,
        ContextType::Custom,
    ];

    for ct in &types {
        let json = serde_json::to_string(ct).expect("serialize");
        let deserialized: ContextType = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(&deserialized, ct);
    }
}

#[test]
fn severity_serde() {
    let severities = vec![Severity::Warning, Severity::Error, Severity::Critical];

    for sev in &severities {
        let json = serde_json::to_string(sev).expect("serialize");
        let deserialized: Severity = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(&deserialized, sev);
    }
}

#[test]
fn event_category_serde() {
    let categories = vec![
        EventCategory::Device,
        EventCategory::Effect,
        EventCategory::Scene,
        EventCategory::Audio,
        EventCategory::System,
        EventCategory::Automation,
        EventCategory::Layout,
        EventCategory::Input,
        EventCategory::Integration,
    ];

    for cat in &categories {
        let json = serde_json::to_string(cat).expect("serialize");
        let deserialized: EventCategory = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(&deserialized, cat);
    }
}

#[test]
fn frame_timing_serde() {
    let timing = FrameTiming {
        render_us: 1200,
        sample_us: 300,
        push_us: 800,
        total_us: 2500,
        budget_us: 16666,
    };

    let json = serde_json::to_string(&timing).expect("serialize");
    let deserialized: FrameTiming = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(deserialized, timing);
}

// ── Clone / Debug Smoke Tests ───────────────────────────────────────────

#[test]
fn event_is_clone_and_debug() {
    let event = HypercolorEvent::BrightnessChanged {
        old: 100,
        new_value: 80,
    };
    let cloned = event.clone();
    let debug_str = format!("{cloned:?}");
    assert!(debug_str.contains("BrightnessChanged"));
}

#[test]
fn frame_data_is_clone_and_debug() {
    let frame = FrameData::new(
        vec![ZoneColors {
            zone_id: "z1".into(),
            colors: vec![[128, 64, 32]],
        }],
        42,
        700,
    );
    let cloned = frame.clone();
    let debug_str = format!("{cloned:?}");
    assert!(debug_str.contains("42"));
}
