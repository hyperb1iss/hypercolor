use hypercolor_daemon_link::{
    IdentityKeypair, SignedUpgradeHeaders, UpgradeHeaderInput, UpgradeNonce,
};
use reqwest::Url;
use uuid::Uuid;

use crate::{CloudClient, CloudClientError};

#[derive(Clone)]
pub struct DaemonConnectInput<'a> {
    pub daemon_id: Uuid,
    pub keypair: &'a IdentityKeypair,
    pub bearer_token: &'a str,
    pub daemon_version: &'a str,
    pub timestamp: &'a str,
    pub nonce: UpgradeNonce,
}

impl std::fmt::Debug for DaemonConnectInput<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DaemonConnectInput")
            .field("daemon_id", &self.daemon_id)
            .field("keypair", &"<redacted>")
            .field("bearer_token", &"<redacted>")
            .field("daemon_version", &self.daemon_version)
            .field("timestamp", &self.timestamp)
            .field("nonce", &self.nonce)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct DaemonConnectRequest {
    pub url: Url,
    pub headers: SignedUpgradeHeaders,
}

impl std::fmt::Debug for DaemonConnectRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DaemonConnectRequest")
            .field("url", &self.url)
            .field("headers", &self.headers)
            .finish()
    }
}

impl CloudClient {
    pub fn prepare_daemon_connect(
        &self,
        input: DaemonConnectInput<'_>,
    ) -> Result<DaemonConnectRequest, CloudClientError> {
        let url = self.config().daemon_connect_url()?;
        let host = connect_authority(&url)?;
        let headers = UpgradeHeaderInput {
            host: &host,
            daemon_id: input.daemon_id,
            daemon_version: input.daemon_version,
            timestamp: input.timestamp,
            nonce: &input.nonce,
            authorization_jwt: input.bearer_token,
        }
        .signed_headers(input.keypair);

        Ok(DaemonConnectRequest { url, headers })
    }
}

pub fn connect_authority(url: &Url) -> Result<String, CloudClientError> {
    let host = url
        .host_str()
        .ok_or_else(|| CloudClientError::InvalidBaseUrl("missing daemon connect host".into()))?;
    let host = if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_owned()
    };

    Ok(url
        .port()
        .map_or_else(|| host.clone(), |port| format!("{host}:{port}")))
}
