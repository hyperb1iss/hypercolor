#![allow(dead_code, unused_imports)]

#[path = "../src/components/page_header.rs"]
pub mod page_header_mod;

mod components {
    pub use super::page_header_mod as page_header;
}

#[path = "../src/api/mod.rs"]
mod api;
#[path = "../src/pages/displays.rs"]
mod displays;
#[path = "../src/icons.rs"]
mod icons;
#[path = "../src/toasts.rs"]
mod toasts;

use api::{DisplaySummary, UpdateSimulatedDisplayRequest};
use displays::{is_simulator_display, parse_simulator_dimension};

fn display_summary(family: &str) -> DisplaySummary {
    DisplaySummary {
        id: "display-1".to_owned(),
        name: "Preview LCD".to_owned(),
        vendor: "Hypercolor".to_owned(),
        family: family.to_owned(),
        width: 480,
        height: 480,
        circular: true,
        overlay_count: 2,
        enabled_overlay_count: 1,
    }
}

#[test]
fn simulator_detection_is_case_insensitive() {
    assert!(is_simulator_display(&display_summary("simulator")));
    assert!(is_simulator_display(&display_summary("Simulator")));
    assert!(is_simulator_display(&display_summary("SIMULATOR")));
}

#[test]
fn simulator_detection_rejects_other_families() {
    assert!(!is_simulator_display(&display_summary("corsair")));
    assert!(!is_simulator_display(&display_summary("custom")));
}

#[test]
fn parse_simulator_dimension_accepts_trimmed_positive_values() {
    assert_eq!(parse_simulator_dimension(" 480 ", "Width"), Ok(480));
    assert_eq!(parse_simulator_dimension("1", "Height"), Ok(1));
}

#[test]
fn parse_simulator_dimension_rejects_invalid_values() {
    assert_eq!(
        parse_simulator_dimension("0", "Width"),
        Err("Width must be a positive number.".to_owned())
    );
    assert_eq!(
        parse_simulator_dimension("abc", "Height"),
        Err("Height must be a positive number.".to_owned())
    );
}

#[test]
fn update_simulated_display_request_skips_absent_fields() {
    let payload = serde_json::to_value(UpdateSimulatedDisplayRequest::default())
        .expect("default simulator update request should serialize");
    assert_eq!(payload, serde_json::json!({}));
}

#[test]
fn update_simulated_display_request_serializes_only_present_fields() {
    let payload = serde_json::to_value(UpdateSimulatedDisplayRequest {
        name: Some("Desk LCD".to_owned()),
        width: Some(600),
        height: None,
        circular: Some(false),
        enabled: None,
    })
    .expect("partial simulator update request should serialize");

    assert_eq!(
        payload,
        serde_json::json!({
            "name": "Desk LCD",
            "width": 600,
            "circular": false
        })
    );
}
