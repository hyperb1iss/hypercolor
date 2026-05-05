use hypercolor_cloud_client::daemon_link::{
    DeniedChannel, IdentityNonce, UpgradeNonce, WelcomeFrame, frame::DenialReason,
};
use hypercolor_cloud_client::{
    CloudClient, CloudClientError, DaemonConnectRequest, RefreshTokenOwner, SecretStore,
    StoredDaemonConnect, StoredDaemonConnectInput,
};
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CloudConnectionRuntimeState {
    #[default]
    Idle,
    Connecting,
    Connected,
    Backoff,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, ToSchema)]
pub struct CloudConnectionSnapshot {
    pub runtime_state: CloudConnectionRuntimeState,
    pub connected: bool,
    pub session_id: Option<String>,
    pub available_channels: Vec<String>,
    pub denied_channels: Vec<CloudDeniedChannelStatus>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CloudConnectionPrepareInput<'a> {
    pub install_name: &'a str,
    pub os: &'a str,
    pub arch: &'a str,
    pub daemon_version: &'a str,
    pub identity_nonce: IdentityNonce,
    pub timestamp: &'a str,
    pub upgrade_nonce: UpgradeNonce,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloudConnectionPrepareResult {
    MissingIdentity,
    MissingRefreshToken,
    Prepared(DaemonConnectRequest),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct CloudDeniedChannelStatus {
    pub name: String,
    pub reason: String,
    pub feature: Option<String>,
}

#[derive(Debug, Default)]
pub struct CloudConnectionRuntime {
    snapshot: CloudConnectionSnapshot,
}

impl CloudConnectionRuntime {
    pub fn snapshot(&self) -> CloudConnectionSnapshot {
        self.snapshot.clone()
    }

    pub fn mark_idle(&mut self) {
        self.snapshot = CloudConnectionSnapshot::default();
    }

    pub fn mark_connecting(&mut self) {
        self.snapshot.runtime_state = CloudConnectionRuntimeState::Connecting;
        self.snapshot.connected = false;
        self.snapshot.session_id = None;
        self.snapshot.available_channels.clear();
        self.snapshot.denied_channels.clear();
        self.snapshot.last_error = None;
    }

    pub fn mark_connected(&mut self, welcome: &WelcomeFrame) {
        self.snapshot = CloudConnectionSnapshot {
            runtime_state: CloudConnectionRuntimeState::Connected,
            connected: true,
            session_id: Some(welcome.session_id.to_string()),
            available_channels: welcome
                .available_channels
                .iter()
                .map(|channel| channel.as_str().to_owned())
                .collect(),
            denied_channels: welcome
                .denied_channels
                .iter()
                .map(CloudDeniedChannelStatus::from_denied_channel)
                .collect(),
            last_error: None,
        };
    }

    pub fn mark_backoff(&mut self, error: impl Into<String>) {
        self.snapshot.runtime_state = CloudConnectionRuntimeState::Backoff;
        self.snapshot.connected = false;
        self.snapshot.session_id = None;
        self.snapshot.available_channels.clear();
        self.snapshot.denied_channels.clear();
        self.snapshot.last_error = Some(error.into());
    }

    pub async fn prepare_stored_daemon_connect(
        &mut self,
        client: &CloudClient,
        store: &impl SecretStore,
        input: CloudConnectionPrepareInput<'_>,
    ) -> Result<CloudConnectionPrepareResult, CloudClientError> {
        self.mark_connecting();
        let result = client
            .prepare_stored_daemon_connect(
                store,
                StoredDaemonConnectInput {
                    token_owner: RefreshTokenOwner::Daemon,
                    install_name: input.install_name,
                    os: input.os,
                    arch: input.arch,
                    daemon_version: input.daemon_version,
                    identity_nonce: input.identity_nonce,
                    timestamp: input.timestamp,
                    nonce: input.upgrade_nonce,
                },
            )
            .await;

        match result {
            Ok(StoredDaemonConnect::MissingIdentity) => {
                self.mark_backoff("missing cloud identity");
                Ok(CloudConnectionPrepareResult::MissingIdentity)
            }
            Ok(StoredDaemonConnect::MissingRefreshToken) => {
                self.mark_backoff("missing cloud refresh token");
                Ok(CloudConnectionPrepareResult::MissingRefreshToken)
            }
            Ok(StoredDaemonConnect::Prepared(request)) => {
                Ok(CloudConnectionPrepareResult::Prepared(request))
            }
            Err(error) => {
                self.mark_backoff(error.to_string());
                Err(error)
            }
        }
    }
}

impl CloudDeniedChannelStatus {
    fn from_denied_channel(denied: &DeniedChannel) -> Self {
        Self {
            name: denied.name.as_str().to_owned(),
            reason: denial_reason_code(denied.reason).to_owned(),
            feature: denied.feature.clone(),
        }
    }
}

const fn denial_reason_code(reason: DenialReason) -> &'static str {
    match reason {
        DenialReason::EntitlementMissing => "entitlement_missing",
        DenialReason::CapabilityMissing => "capability_missing",
        DenialReason::Disabled => "disabled",
    }
}
