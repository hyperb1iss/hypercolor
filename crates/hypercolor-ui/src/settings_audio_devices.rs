#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioDeviceChoice {
    pub id: String,
    pub name: String,
    pub description: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioDeviceDropdownState {
    pub options: Vec<(String, String)>,
    pub placeholder: String,
    pub disabled: bool,
}

pub enum AudioDeviceLoadState<'a> {
    Loading,
    Error,
    Ready(&'a [AudioDeviceChoice]),
}

#[must_use]
pub fn resolve_audio_device_dropdown(
    configured_device: Option<&str>,
    state: AudioDeviceLoadState<'_>,
) -> AudioDeviceDropdownState {
    let configured_device = configured_device
        .map(str::trim)
        .filter(|device| !device.is_empty());

    match state {
        AudioDeviceLoadState::Loading => AudioDeviceDropdownState {
            options: configured_device
                .map(|device| {
                    (
                        device.to_owned(),
                        configured_audio_device_label(device, false),
                    )
                })
                .into_iter()
                .collect(),
            placeholder: "Loading devices...".to_string(),
            disabled: true,
        },
        AudioDeviceLoadState::Error => AudioDeviceDropdownState {
            options: configured_device
                .map(|device| {
                    (
                        device.to_owned(),
                        configured_audio_device_label(device, true),
                    )
                })
                .into_iter()
                .collect(),
            placeholder: "Couldn't load devices".to_string(),
            disabled: true,
        },
        AudioDeviceLoadState::Ready(devices) => {
            let mut options = devices
                .iter()
                .map(|device| (device.id.clone(), discovered_audio_device_label(device)))
                .collect::<Vec<_>>();

            if let Some(configured_device) = configured_device
                && !options.iter().any(|(id, _)| id == configured_device)
            {
                options.insert(
                    0,
                    (
                        configured_device.to_owned(),
                        configured_audio_device_label(configured_device, true),
                    ),
                );
            }

            AudioDeviceDropdownState {
                options,
                placeholder: if devices.is_empty() {
                    "No audio devices detected".to_string()
                } else {
                    "Select audio device".to_string()
                },
                disabled: devices.is_empty(),
            }
        }
    }
}

fn discovered_audio_device_label(device: &AudioDeviceChoice) -> String {
    if device.description.is_empty() || device.description == device.name {
        device.name.clone()
    } else if device
        .description
        .to_ascii_lowercase()
        .contains("unavailable")
    {
        format!("{} (Unavailable)", device.name)
    } else {
        device.name.clone()
    }
}

fn configured_audio_device_label(device_id: &str, unavailable: bool) -> String {
    let base_label = if device_id == "default" {
        "System default".to_string()
    } else {
        device_id.to_owned()
    };
    if unavailable {
        format!("{base_label} (Configured, unavailable)")
    } else {
        format!("{base_label} (Configured)")
    }
}
