use hypercolor_cloud_api::{DeviceRegistrationRequest, DeviceRegistrationResponse};
use hypercolor_daemon_link::{IdentityKeypair, IdentityNonce, registration_proof_message};
use uuid::Uuid;

use crate::{CloudClient, CloudClientError};

pub const DEVICE_REGISTRATION_PATH: &str = "/v1/me/devices";

#[derive(Debug, Clone)]
pub struct DeviceRegistrationInput {
    pub daemon_id: Uuid,
    pub install_name: String,
    pub os: String,
    pub arch: String,
    pub daemon_version: String,
    pub nonce: IdentityNonce,
}

pub fn signed_device_registration(
    input: DeviceRegistrationInput,
    keypair: &IdentityKeypair,
) -> DeviceRegistrationRequest {
    let identity_pubkey = keypair.public_key();
    let identity_proof = keypair.sign(&registration_proof_message(
        input.daemon_id,
        &identity_pubkey,
        &input.nonce,
    ));

    DeviceRegistrationRequest {
        daemon_id: input.daemon_id,
        install_name: input.install_name,
        os: input.os,
        arch: input.arch,
        daemon_version: input.daemon_version,
        identity_pubkey: identity_pubkey.as_str().to_owned(),
        identity_proof: identity_proof.as_str().to_owned(),
        nonce: input.nonce.as_str().to_owned(),
    }
}

impl CloudClient {
    pub async fn register_device(
        &self,
        access_token: &str,
        request: &DeviceRegistrationRequest,
    ) -> Result<DeviceRegistrationResponse, CloudClientError> {
        let response = self
            .http_client()
            .post(self.config().api_url(DEVICE_REGISTRATION_PATH)?)
            .bearer_auth(access_token)
            .json(request)
            .send()
            .await?
            .error_for_status()?
            .json::<DeviceRegistrationResponse>()
            .await?;

        Ok(response)
    }
}
