use hypercolor_types::device::DriverTransportKind;

use crate::api::DriverSummary;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryDriverSetting {
    pub id: String,
    pub label: String,
    pub description: String,
    pub key: String,
}

pub fn discovery_driver_settings(drivers: &[DriverSummary]) -> Vec<DiscoveryDriverSetting> {
    drivers
        .iter()
        .filter(|driver| driver.descriptor.capabilities.discovery)
        .map(discovery_driver_setting)
        .collect()
}

fn discovery_driver_setting(driver: &DriverSummary) -> DiscoveryDriverSetting {
    let descriptor = &driver.descriptor;
    let label = &driver.presentation.label;

    DiscoveryDriverSetting {
        id: descriptor.id.clone(),
        label: format!("{label} Scan"),
        description: discovery_description(
            label,
            descriptor.capabilities.pairing,
            &descriptor.transports,
        ),
        key: format!("{}.enabled", driver.config_key),
    }
}

fn discovery_description(
    label: &str,
    supports_pairing: bool,
    transports: &[DriverTransportKind],
) -> String {
    let transports = transport_summary(transports);

    if supports_pairing {
        format!("Discover and pair {label} devices over {transports}")
    } else {
        format!("Discover {label} devices over {transports}")
    }
}

fn transport_summary(transports: &[DriverTransportKind]) -> String {
    let labels = transports.iter().map(transport_label).collect::<Vec<_>>();

    match labels.as_slice() {
        [] => "configured transports".to_owned(),
        [one] => one.clone(),
        [first, second] => format!("{first} and {second}"),
        _ => {
            let last = labels.last().cloned().unwrap_or_default();
            let head = labels[..labels.len() - 1].join(", ");
            format!("{head}, and {last}")
        }
    }
}

fn transport_label(transport: &DriverTransportKind) -> String {
    match transport {
        DriverTransportKind::Network => "the network".to_owned(),
        DriverTransportKind::Usb => "USB".to_owned(),
        DriverTransportKind::Smbus => "SMBus".to_owned(),
        DriverTransportKind::Midi => "MIDI".to_owned(),
        DriverTransportKind::Serial => "serial".to_owned(),
        DriverTransportKind::Bridge => "bridge services".to_owned(),
        DriverTransportKind::Virtual => "virtual transport".to_owned(),
        DriverTransportKind::Custom(label) => label.clone(),
    }
}
