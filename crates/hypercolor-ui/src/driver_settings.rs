use hypercolor_types::device::{DriverModuleDescriptor, DriverTransportKind};

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
        .map(|driver| discovery_driver_setting(&driver.descriptor, &driver.config_key))
        .collect()
}

fn discovery_driver_setting(
    descriptor: &DriverModuleDescriptor,
    config_key: &str,
) -> DiscoveryDriverSetting {
    DiscoveryDriverSetting {
        id: descriptor.id.clone(),
        label: format!("{} Scan", descriptor.display_name),
        description: discovery_description(descriptor),
        key: format!("{config_key}.enabled"),
    }
}

fn discovery_description(descriptor: &DriverModuleDescriptor) -> String {
    let transports = transport_summary(&descriptor.transports);

    if descriptor.capabilities.pairing {
        format!(
            "Discover and pair {} devices over {transports}",
            descriptor.display_name
        )
    } else {
        format!(
            "Discover {} devices over {transports}",
            descriptor.display_name
        )
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
