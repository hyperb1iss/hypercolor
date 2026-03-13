#![cfg(target_os = "linux")]

use hypercolor_core::input::audio::linux::{
    PulseSourceSnapshot, build_named_audio_sources, default_monitor_source_name_from_snapshots,
};

#[test]
fn default_monitor_source_prefers_monitor_of_default_sink() {
    let sources = vec![
        PulseSourceSnapshot {
            name: "alsa_output.usb-Other-00.analog-stereo.monitor".into(),
            description: Some("Monitor of Other Output".into()),
            monitor_of_sink_name: Some("alsa_output.usb-Other-00.analog-stereo".into()),
        },
        PulseSourceSnapshot {
            name: "alsa_output.usb-Main-00.analog-stereo.monitor".into(),
            description: Some("Monitor of Main Output".into()),
            monitor_of_sink_name: Some("alsa_output.usb-Main-00.analog-stereo".into()),
        },
    ];

    let default_monitor = default_monitor_source_name_from_snapshots(
        &sources,
        Some("alsa_output.usb-Main-00.analog-stereo"),
    );

    assert_eq!(
        default_monitor.as_deref(),
        Some("alsa_output.usb-Main-00.analog-stereo.monitor")
    );
}

#[test]
fn build_named_audio_sources_ranks_default_monitor_first() {
    let sources = vec![
        PulseSourceSnapshot {
            name: "alsa_input.usb-Razer-00.analog-stereo".into(),
            description: Some("Razer Seiren V3 Chroma Analog Stereo".into()),
            monitor_of_sink_name: None,
        },
        PulseSourceSnapshot {
            name: "alsa_output.usb-Main-00.analog-stereo.monitor".into(),
            description: Some("Monitor of Main Output".into()),
            monitor_of_sink_name: Some("alsa_output.usb-Main-00.analog-stereo".into()),
        },
        PulseSourceSnapshot {
            name: "alsa_output.usb-Other-00.analog-stereo.monitor".into(),
            description: Some("Monitor of Other Output".into()),
            monitor_of_sink_name: Some("alsa_output.usb-Other-00.analog-stereo".into()),
        },
    ];

    let named_sources =
        build_named_audio_sources(&sources, Some("alsa_output.usb-Main-00.analog-stereo"));

    assert_eq!(
        named_sources[0].id,
        "alsa_output.usb-Main-00.analog-stereo.monitor"
    );
    assert!(named_sources[0].is_default_monitor);
    assert!(named_sources[0].is_monitor);
    assert_eq!(
        named_sources[1].id,
        "alsa_output.usb-Other-00.analog-stereo.monitor"
    );
    assert!(named_sources[1].is_monitor);
    assert_eq!(named_sources[2].id, "alsa_input.usb-Razer-00.analog-stereo");
    assert!(!named_sources[2].is_monitor);
}
