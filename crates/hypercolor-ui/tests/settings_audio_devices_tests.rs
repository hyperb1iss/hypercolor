#[path = "../src/settings_audio_devices.rs"]
mod settings_audio_devices;

use settings_audio_devices::{
    AudioDeviceChoice, AudioDeviceDropdownState, AudioDeviceLoadState,
    resolve_audio_device_dropdown,
};

fn choice(id: &str, name: &str, description: &str) -> AudioDeviceChoice {
    AudioDeviceChoice {
        id: id.to_string(),
        name: name.to_string(),
        description: description.to_string(),
    }
}

#[test]
fn loading_without_configured_device_shows_placeholder_only() {
    assert_eq!(
        resolve_audio_device_dropdown(None, AudioDeviceLoadState::Loading),
        AudioDeviceDropdownState {
            options: Vec::new(),
            placeholder: "Loading devices...".to_string(),
            disabled: true,
        }
    );
}

#[test]
fn error_keeps_configured_device_visible_without_faking_discovery() {
    assert_eq!(
        resolve_audio_device_dropdown(Some("default"), AudioDeviceLoadState::Error),
        AudioDeviceDropdownState {
            options: vec![(
                "default".to_string(),
                "System default (Configured, unavailable)".to_string(),
            )],
            placeholder: "Couldn't load devices".to_string(),
            disabled: true,
        }
    );
}

#[test]
fn ready_devices_prefer_real_descriptions_and_mark_unavailable_entries() {
    let devices = vec![
        choice("default", "Default", "Default"),
        choice("usb", "USB DAC", "Unavailable in exclusive mode"),
    ];

    assert_eq!(
        resolve_audio_device_dropdown(Some("default"), AudioDeviceLoadState::Ready(&devices)),
        AudioDeviceDropdownState {
            options: vec![
                ("default".to_string(), "Default".to_string()),
                ("usb".to_string(), "USB DAC (Unavailable)".to_string()),
            ],
            placeholder: "Select audio device".to_string(),
            disabled: false,
        }
    );
}

#[test]
fn ready_devices_insert_missing_configured_device_without_enabling_selection() {
    let devices = Vec::new();

    assert_eq!(
        resolve_audio_device_dropdown(
            Some("loopback.monitor"),
            AudioDeviceLoadState::Ready(&devices)
        ),
        AudioDeviceDropdownState {
            options: vec![(
                "loopback.monitor".to_string(),
                "loopback.monitor (Configured, unavailable)".to_string(),
            )],
            placeholder: "No audio devices detected".to_string(),
            disabled: true,
        }
    );
}
