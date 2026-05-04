use hypercolor_cloud_api::EntitlementTokenResponse;

use crate::{CloudClient, CloudClientError};

pub const ENTITLEMENTS_PATH: &str = "/v1/me/entitlements";

impl CloudClient {
    pub async fn fetch_entitlement_token(
        &self,
        access_token: &str,
    ) -> Result<EntitlementTokenResponse, CloudClientError> {
        let response = self
            .http_client()
            .get(self.config().api_url(ENTITLEMENTS_PATH)?)
            .bearer_auth(access_token)
            .send()
            .await?
            .error_for_status()?
            .json::<EntitlementTokenResponse>()
            .await?;

        Ok(response)
    }
}
