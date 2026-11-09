//! Spec 89 stability labels in the `drivers list` table.

use hypercolor_cli::commands::drivers::tier_label;
use hypercolor_types::api::drivers::DriverSummary;
use serde_json::json;

fn summary(stability: &str, protocol_stabilities: &[&str]) -> DriverSummary {
    let protocols = protocol_stabilities
        .iter()
        .enumerate()
        .map(|(index, stability)| {
            json!({
                "driver_id": "fixture",
                "protocol_id": format!("fixture/p{index}"),
                "display_name": "Fixture protocol",
                "family_id": "fixture",
                "transport": "usb",
                "route_backend_id": "usb",
                "stability": stability
            })
        })
        .collect::<Vec<_>>();
    serde_json::from_value(json!({
        "descriptor": {
            "id": "fixture",
            "display_name": "Fixture",
            "module_kind": "hal",
            "transports": [],
            "capabilities": {
                "config": false,
                "discovery": false,
                "pairing": false,
                "output_backend": false,
                "protocol_catalog": true,
                "runtime_cache": false,
                "credentials": false,
                "presentation": false,
                "controls": false
            },
            "api_schema_version": 3,
            "config_version": 1,
            "default_enabled": true,
            "stability": stability
        },
        "presentation": { "label": "Fixture" },
        "enabled": true,
        "config_key": "drivers.fixture",
        "protocols": protocols
    }))
    .expect("driver summary fixture")
}

#[test]
fn tier_reads_stable_for_a_fully_stable_module() {
    assert_eq!(
        tier_label(&summary("stable", &["stable", "stable"])),
        "stable"
    );
}

#[test]
fn tier_reads_experimental_for_an_experimental_module() {
    assert_eq!(
        tier_label(&summary("experimental", &["experimental"])),
        "experimental"
    );
}

#[test]
fn tier_counts_experimental_protocols_inside_a_stable_module() {
    assert_eq!(
        tier_label(&summary(
            "stable",
            &["stable", "experimental", "experimental"]
        )),
        "stable +2 experimental"
    );
}
