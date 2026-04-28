#![allow(dead_code, unused_imports)]

#[path = "../src/api/mod.rs"]
mod api;

use api::{
    ControlSurfaceListQuery, control_surface_action_url, control_surface_list_url,
    control_surface_values_url,
};

#[test]
fn list_url_selects_device_and_driver_surfaces() {
    assert_eq!(
        control_surface_list_url(ControlSurfaceListQuery {
            device_id: Some("Desk Strip"),
            driver_id: None,
            include_driver: true,
        }),
        "/api/v1/control-surfaces?device_id=Desk%20Strip&include_driver=true"
    );
}

#[test]
fn list_url_selects_driver_surface() {
    assert_eq!(
        control_surface_list_url(ControlSurfaceListQuery {
            device_id: None,
            driver_id: Some("wled"),
            include_driver: false,
        }),
        "/api/v1/control-surfaces?driver_id=wled"
    );
}

#[test]
fn mutation_urls_encode_surface_and_action_ids() {
    assert_eq!(
        control_surface_values_url("driver:wled"),
        "/api/v1/control-surfaces/driver%3Awled/values"
    );
    assert_eq!(
        control_surface_action_url("driver:wled:device:Desk Strip", "refresh topology"),
        "/api/v1/control-surfaces/driver%3Awled%3Adevice%3ADesk%20Strip/actions/refresh%20topology"
    );
}
