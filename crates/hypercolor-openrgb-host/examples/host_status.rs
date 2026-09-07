//! Print what this crate sees on the current host as JSON.
//!
//! ```sh
//! cargo run -p hypercolor-openrgb-host --example host_status
//! ```

use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use hypercolor_openrgb_host::{
    DEFAULT_SERVER_PORT, detect_binary, install_hints, permission_checks, probe_server,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let binary = detect_binary().await;
    let probe = probe_server(
        SocketAddr::from((Ipv4Addr::LOCALHOST, DEFAULT_SERVER_PORT)),
        Duration::from_secs(1),
    )
    .await;
    let status = serde_json::json!({
        "binary": binary,
        "server": probe,
        "install_hints": install_hints(),
        "permission_checks": permission_checks(),
    });
    println!("{}", serde_json::to_string_pretty(&status)?);
    Ok(())
}
