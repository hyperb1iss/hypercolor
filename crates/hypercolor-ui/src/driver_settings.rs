use hypercolor_types::device::DriverTransportKind;

use crate::api::DriverSummary;
use crate::label_utils::humanize_identifier_label;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryDriverSetting {
    pub id: String,
    pub label: String,
    pub key: String,
    pub transport_labels: Vec<String>,
    pub supports_pairing: bool,
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
        label: label.clone(),
        key: format!("{}.enabled", driver.config_key),
        transport_labels: transport_labels(&descriptor.transports),
        supports_pairing: descriptor.capabilities.pairing,
    }
}

fn transport_labels(transports: &[DriverTransportKind]) -> Vec<String> {
    let labels = transports.iter().map(transport_label).collect::<Vec<_>>();
    if labels.is_empty() {
        vec!["Configured".to_owned()]
    } else {
        labels
    }
}

fn transport_label(transport: &DriverTransportKind) -> String {
    match transport {
        DriverTransportKind::Network => "Network".to_owned(),
        DriverTransportKind::Usb => "USB".to_owned(),
        DriverTransportKind::Smbus => "SMBus".to_owned(),
        DriverTransportKind::Midi => "MIDI".to_owned(),
        DriverTransportKind::Serial => "Serial".to_owned(),
        DriverTransportKind::Bridge => "Bridge".to_owned(),
        DriverTransportKind::Virtual => "Virtual".to_owned(),
        DriverTransportKind::Custom(label) => humanize_identifier_label(label),
    }
}
