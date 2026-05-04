use hypercolor_cloud_api::{
    DeviceCodeRequest, DeviceCodeResponse, DeviceTokenError, DeviceTokenRequest,
    DeviceTokenResponse,
};

use crate::config::{DEVICE_CODE_PATH, DEVICE_TOKEN_PATH};
use crate::{CloudClient, CloudClientError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceTokenPoll {
    Authorized(DeviceTokenResponse),
    Waiting(DeviceTokenError),
}

impl DeviceTokenPoll {
    #[must_use]
    pub const fn is_retryable(&self) -> bool {
        match self {
            Self::Authorized(_) => false,
            Self::Waiting(error) => error.error.is_retryable(),
        }
    }
}

impl CloudClient {
    pub async fn start_device_authorization(&self) -> Result<DeviceCodeResponse, CloudClientError> {
        let request = DeviceCodeRequest::new(
            self.config().device_client_id(),
            self.config().device_scope(),
        );
        let response = self
            .http_client()
            .post(self.config().auth_url(DEVICE_CODE_PATH)?)
            .json(&request)
            .send()
            .await?
            .error_for_status()?
            .json::<DeviceCodeResponse>()
            .await?;

        Ok(response)
    }

    pub async fn poll_device_token(
        &self,
        device_code: impl Into<String>,
    ) -> Result<DeviceTokenPoll, CloudClientError> {
        let request = DeviceTokenRequest::new(device_code, self.config().device_client_id());
        let response = self
            .http_client()
            .post(self.config().auth_url(DEVICE_TOKEN_PATH)?)
            .json(&request)
            .send()
            .await?;

        if response.status().is_success() {
            return Ok(DeviceTokenPoll::Authorized(
                response.json::<DeviceTokenResponse>().await?,
            ));
        }

        Ok(DeviceTokenPoll::Waiting(
            response.json::<DeviceTokenError>().await?,
        ))
    }
}
