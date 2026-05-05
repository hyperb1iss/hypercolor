use hypercolor_daemon_link::{
    IdentityKeypair, IdentityNonce, SignedUpgradeHeaders, UpgradeHeaderInput, UpgradeNonce,
};
use reqwest::Url;
use uuid::Uuid;

use crate::{CloudClient, CloudClientError, RefreshTokenOwner, SecretStore, load_identity};
use crate::{DeviceRegistrationInput, signed_device_registration};

#[derive(Clone)]
pub struct DaemonConnectInput<'a> {
    pub daemon_id: Uuid,
    pub keypair: &'a IdentityKeypair,
    pub bearer_token: &'a str,
    pub daemon_version: &'a str,
    pub timestamp: &'a str,
    pub nonce: UpgradeNonce,
}

#[derive(Clone, Debug)]
pub struct StoredDaemonConnectInput<'a> {
    pub token_owner: RefreshTokenOwner,
    pub install_name: &'a str,
    pub os: &'a str,
    pub arch: &'a str,
    pub daemon_version: &'a str,
    pub identity_nonce: IdentityNonce,
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoredDaemonConnect {
    MissingIdentity,
    MissingRefreshToken,
    Prepared(DaemonConnectRequest),
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

    pub async fn prepare_stored_daemon_connect(
        &self,
        store: &impl SecretStore,
        input: StoredDaemonConnectInput<'_>,
    ) -> Result<StoredDaemonConnect, CloudClientError> {
        let Some(identity) = load_identity(store)? else {
            return Ok(StoredDaemonConnect::MissingIdentity);
        };
        let Some(token) = self
            .refresh_stored_device_token(store, input.token_owner)
            .await?
        else {
            return Ok(StoredDaemonConnect::MissingRefreshToken);
        };
        let registration = signed_device_registration(
            DeviceRegistrationInput {
                daemon_id: identity.daemon_id(),
                install_name: input.install_name.to_owned(),
                os: input.os.to_owned(),
                arch: input.arch.to_owned(),
                daemon_version: input.daemon_version.to_owned(),
                nonce: input.identity_nonce,
            },
            identity.keypair(),
        );
        let registration = self
            .register_device(&token.access_token, &registration)
            .await?;
        let request = self.prepare_daemon_connect(DaemonConnectInput {
            daemon_id: identity.daemon_id(),
            keypair: identity.keypair(),
            bearer_token: &registration.registration_token,
            daemon_version: input.daemon_version,
            timestamp: input.timestamp,
            nonce: input.nonce,
        })?;

        Ok(StoredDaemonConnect::Prepared(request))
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
