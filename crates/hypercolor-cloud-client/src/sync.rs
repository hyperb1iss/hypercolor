use hypercolor_cloud_api::{
    ApiEnvelope, ChangesResponse, Etag, SyncConflictResponse, SyncEntity, SyncEntityKind,
    SyncPutRequest,
};
use reqwest::StatusCode;
use reqwest::header::IF_MATCH;

use crate::{CloudClient, CloudClientError};

pub const SYNC_PATH: &str = "/v1/sync";

impl CloudClient {
    pub async fn list_sync_entities(
        &self,
        access_token: &str,
        kind: SyncEntityKind,
    ) -> Result<Vec<SyncEntity>, CloudClientError> {
        let response = self
            .http_client()
            .get(
                self.config()
                    .api_url(&format!("{SYNC_PATH}/{}", kind.path_segment()))?,
            )
            .bearer_auth(access_token)
            .send()
            .await?
            .error_for_status()?
            .json::<ApiEnvelope<Vec<SyncEntity>>>()
            .await?;

        Ok(response.data)
    }

    pub async fn fetch_sync_changes(
        &self,
        access_token: &str,
        since: i64,
    ) -> Result<ChangesResponse, CloudClientError> {
        let response = self
            .http_client()
            .get(self.config().api_url(&format!("{SYNC_PATH}/changes"))?)
            .bearer_auth(access_token)
            .query(&[("since", since)])
            .send()
            .await?
            .error_for_status()?
            .json::<ChangesResponse>()
            .await?;

        Ok(response)
    }

    pub async fn put_sync_entity(
        &self,
        access_token: &str,
        kind: SyncEntityKind,
        entity_id: &str,
        if_match: Etag,
        request: &SyncPutRequest,
    ) -> Result<SyncEntity, CloudClientError> {
        let response = self
            .http_client()
            .put(self.sync_entity_url(kind, entity_id)?)
            .bearer_auth(access_token)
            .header(IF_MATCH, if_match.to_string())
            .json(request)
            .send()
            .await?;

        sync_entity_response(response).await
    }

    pub async fn delete_sync_entity(
        &self,
        access_token: &str,
        kind: SyncEntityKind,
        entity_id: &str,
        if_match: Etag,
    ) -> Result<SyncEntity, CloudClientError> {
        let response = self
            .http_client()
            .delete(self.sync_entity_url(kind, entity_id)?)
            .bearer_auth(access_token)
            .header(IF_MATCH, if_match.to_string())
            .send()
            .await?;

        sync_entity_response(response).await
    }

    fn sync_entity_url(
        &self,
        kind: SyncEntityKind,
        entity_id: &str,
    ) -> Result<reqwest::Url, CloudClientError> {
        let mut url = self.config().api_url(SYNC_PATH)?;
        let invalid_url = url.to_string();
        url.path_segments_mut()
            .map_err(|()| CloudClientError::InvalidBaseUrl(invalid_url))?
            .push(kind.path_segment())
            .push(entity_id);

        Ok(url)
    }
}

async fn sync_entity_response(response: reqwest::Response) -> Result<SyncEntity, CloudClientError> {
    if response.status() == StatusCode::PRECONDITION_FAILED {
        let conflict = response.json::<SyncConflictResponse>().await?;
        return Err(CloudClientError::SyncConflict {
            current_etag: conflict.current_etag,
            current: conflict.current.map(Box::new),
        });
    }

    Ok(response.error_for_status()?.json::<SyncEntity>().await?)
}
