use hypercolor_hal::transport::midi::{
    classify_push2_port_for_testing, midi_output_path_requires_pacing_for_testing,
    midi_packet_spacing_for_testing, midi_usb_paths_match_for_testing,
    select_push2_port_identity_for_testing,
};

use std::time::Duration;

#[cfg(target_os = "linux")]
use hypercolor_hal::transport::midi::{
    midi_usb_path_from_sound_card_sysfs_for_testing,
    rawmidi_name_from_sound_card_and_seq_port_for_testing, rawmidi_open_retry_for_testing,
};

#[test]
fn classify_push2_port_recognizes_linux_and_macos_names() {
    assert_eq!(
        classify_push2_port_for_testing("Ableton Push 2 24:0"),
        Some("live")
    );
    assert_eq!(
        classify_push2_port_for_testing("Ableton Push 2 24:1"),
        Some("user")
    );
    assert_eq!(
        classify_push2_port_for_testing("Ableton Push 2 User Port"),
        Some("user")
    );
    assert_eq!(
        classify_push2_port_for_testing("Ableton Push 2 Live Port"),
        Some("live")
    );
    assert_eq!(
        classify_push2_port_for_testing("Ableton Push 2"),
        Some("live")
    );
    assert_eq!(
        classify_push2_port_for_testing("MIDIIN2 (Ableton Push 2)"),
        Some("user")
    );
    assert_eq!(
        classify_push2_port_for_testing("MIDIOUT2 (Ableton Push 2)"),
        Some("user")
    );
    assert_eq!(
        classify_push2_port_for_testing("Unrelated Controller"),
        None
    );
}

#[test]
fn push2_port_selection_prefers_requested_usb_path_when_multiple_match() {
    let selected = select_push2_port_identity_for_testing(
        &[
            ("Ableton Push 2 24:1", "24:1", Some("1-2")),
            ("Ableton Push 2 28:1", "28:1", Some("1-6.3")),
        ],
        "user",
        Some("01-6.3"),
    )
    .expect("USB path filtering should disambiguate the user port");

    assert_eq!(selected, "28:1");
}

#[test]
fn usb_path_matching_normalizes_bus_numbers() {
    assert!(midi_usb_paths_match_for_testing("01-6.3", "1-6.3"));
    assert!(midi_usb_paths_match_for_testing("1-6.3", "01-6.3"));
    assert!(!midi_usb_paths_match_for_testing("1-6.3", "1-6.4"));
}

#[test]
fn midi_packet_spacing_paces_sysex_more_than_short_led_updates() {
    assert_eq!(
        midi_packet_spacing_for_testing(3),
        Duration::from_micros(500)
    );
    assert_eq!(midi_packet_spacing_for_testing(9), Duration::from_millis(1));
    assert_eq!(
        midi_packet_spacing_for_testing(17),
        Duration::from_millis(1)
    );
}

#[test]
fn push2_midi_pacing_only_applies_to_sequencer_output() {
    assert_eq!(
        midi_output_path_requires_pacing_for_testing("sequencer"),
        Some(true)
    );
    #[cfg(target_os = "linux")]
    assert_eq!(
        midi_output_path_requires_pacing_for_testing("rawmidi"),
        Some(false)
    );
}

#[cfg(target_os = "linux")]
#[test]
fn sound_card_sysfs_path_extracts_usb_path() {
    let usb_path = midi_usb_path_from_sound_card_sysfs_for_testing(
        "/devices/pci0000:00/0000:00:14.0/usb1/1-12/1-12:1.1/sound/card4",
    );

    assert_eq!(usb_path.as_deref(), Some("1-12"));
}

#[cfg(target_os = "linux")]
#[test]
fn rawmidi_name_maps_alsa_seq_port_to_user_subdevice() {
    assert_eq!(
        rawmidi_name_from_sound_card_and_seq_port_for_testing(3, 1).as_deref(),
        Some("hw:3,0,1")
    );
    assert_eq!(
        rawmidi_name_from_sound_card_and_seq_port_for_testing(3, -1),
        None
    );
}

#[cfg(target_os = "linux")]
#[test]
fn rawmidi_open_retry_waits_for_hotplug_device_node() {
    let (attempts, elapsed) =
        rawmidi_open_retry_for_testing(2, Duration::from_secs(1), Duration::from_millis(50))
            .expect("rawmidi retry should eventually succeed");

    assert_eq!(attempts, 3);
    assert_eq!(elapsed, Duration::from_millis(100));
}
