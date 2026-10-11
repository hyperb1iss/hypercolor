//! The startup bind honors a credential authority end to end.
//!
//! One test per binary: `prepare` installs the process-wide tracing
//! subscriber, which can happen only once.

use std::sync::Arc;

use hypercolor_daemon::api::security::{CredentialAuthority, CredentialGrant, CredentialTier};
use hypercolor_daemon::daemon::{DaemonRunOptions, build_main_runtime, prepare};
use hypercolor_daemon::startup::default_config;
use hypercolor_types::config::NetworkAccessMode;

struct EmptyAuthority;

impl CredentialAuthority for EmptyAuthority {
    fn ceiling(&self) -> CredentialTier {
        CredentialTier::Control
    }

    fn authenticate(&self, _presented: &str) -> Option<CredentialGrant> {
        None
    }
}

#[test]
fn prepare_binds_the_network_for_an_authority_with_no_credentials_yet() {
    let directory = tempfile::tempdir().expect("config directory should be created");
    let config_path = directory.path().join("hypercolor.toml");
    let mut config = default_config();
    config.network.access_mode = NetworkAccessMode::LanProtected;
    config.daemon.listen_address = "0.0.0.0".to_owned();
    config.daemon.port = 0;
    std::fs::write(
        &config_path,
        toml::to_string(&config).expect("config should serialize"),
    )
    .expect("config should be written");

    let runtime = build_main_runtime().expect("runtime should build");
    let prepared = runtime
        .block_on(prepare(DaemonRunOptions {
            config: Some(config_path),
            credential_authority: Some(Arc::new(EmptyAuthority)),
            ..DaemonRunOptions::default()
        }))
        .expect("an authority that can grant control satisfies the network bind");

    assert!(
        prepared.advertised_bind().ip().is_unspecified(),
        "expected the all-interfaces bind, got {}",
        prepared.advertised_bind()
    );
}
