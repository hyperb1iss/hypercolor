use std::fmt::Write as _;

/// Query parameters for `GET /api/v1/control-surfaces`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ControlSurfaceListQuery<'a> {
    pub device_id: Option<&'a str>,
    pub driver_id: Option<&'a str>,
    pub include_driver: bool,
}

pub fn control_surface_list_url(query: ControlSurfaceListQuery<'_>) -> String {
    let mut parts = Vec::new();
    if let Some(device_id) = query.device_id {
        parts.push(format!("device_id={}", query_value(device_id)));
    }
    if let Some(driver_id) = query.driver_id {
        parts.push(format!("driver_id={}", query_value(driver_id)));
    }
    if query.include_driver {
        parts.push("include_driver=true".to_string());
    }

    if parts.is_empty() {
        "/api/v1/control-surfaces".to_string()
    } else {
        format!("/api/v1/control-surfaces?{}", parts.join("&"))
    }
}

pub fn control_surface_values_url(surface_id: &str) -> String {
    format!(
        "/api/v1/control-surfaces/{}/values",
        path_segment(surface_id)
    )
}

pub fn control_surface_action_url(surface_id: &str, action_id: &str) -> String {
    format!(
        "/api/v1/control-surfaces/{}/actions/{}",
        path_segment(surface_id),
        path_segment(action_id)
    )
}

pub fn path_segment(input: &str) -> String {
    percent_encode(input)
}

fn query_value(input: &str) -> String {
    percent_encode(input)
}

fn percent_encode(input: &str) -> String {
    let mut encoded = String::with_capacity(input.len());
    for byte in input.bytes() {
        let unreserved = byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~');
        if unreserved {
            encoded.push(char::from(byte));
        } else {
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::{
        ControlSurfaceListQuery, control_surface_action_url, control_surface_list_url,
        control_surface_values_url,
    };

    #[test]
    fn control_surface_list_url_encodes_device_queries() {
        let url = control_surface_list_url(ControlSurfaceListQuery {
            device_id: Some("usb:driver:desk strip"),
            driver_id: None,
            include_driver: true,
        });

        assert_eq!(
            url,
            "/api/v1/control-surfaces?device_id=usb%3Adriver%3Adesk%20strip&include_driver=true"
        );
    }

    #[test]
    fn control_surface_mutation_urls_encode_surface_and_action_ids() {
        assert_eq!(
            control_surface_values_url("driver:alpha:device:abc/123"),
            "/api/v1/control-surfaces/driver%3Aalpha%3Adevice%3Aabc%2F123/values"
        );
        assert_eq!(
            control_surface_action_url("driver:beta:device:panel 1", "refresh/topology"),
            "/api/v1/control-surfaces/driver%3Abeta%3Adevice%3Apanel%201/actions/refresh%2Ftopology"
        );
    }
}
