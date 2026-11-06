//! Token-bearing calls to a controller's Open API.
//!
//! The API carries its auth token in the URL path, which makes the URL a
//! credential. The URL is therefore built here, handed straight to the
//! bounded HTTP client, and never returned, and every error leaves without
//! it, naming the controller's address instead.

use std::net::{IpAddr, SocketAddr};

use anyhow::{Context, Result};
use hypercolor_driver_support::http::{BoundedRequest, BoundedResponse, ReadBudget};
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::nanoleaf_http_client;

/// One controller's authenticated endpoints. It has no `Debug` or
/// `Display` on purpose, so it cannot be printed with its token.
pub(super) struct AuthenticatedApi<'a> {
    controller: SocketAddr,
    auth_token: &'a str,
}

impl<'a> AuthenticatedApi<'a> {
    pub(super) const fn new(ip: IpAddr, api_port: u16, auth_token: &'a str) -> Self {
        Self {
            controller: SocketAddr::new(ip, api_port),
            auth_token,
        }
    }

    /// `GET` `path` under the token and decode the reply. `purpose` names
    /// the call in errors.
    pub(super) async fn get_json<T: DeserializeOwned>(
        &self,
        path: &str,
        budget: ReadBudget,
        purpose: &str,
    ) -> Result<T> {
        let request = nanoleaf_http_client()?.get(self.url(path));
        self.send(request, budget, purpose)
            .await?
            .json()
            .await
            .with_context(|| {
                format!(
                    "failed to decode Nanoleaf {purpose} response from {}",
                    self.controller
                )
            })
    }

    /// `PUT` `body` as JSON to `path` under the token. Only the status of
    /// the reply matters. `purpose` names the call in errors.
    pub(super) async fn put_json<B: Serialize + ?Sized>(
        &self,
        path: &str,
        body: &B,
        budget: ReadBudget,
        purpose: &str,
    ) -> Result<()> {
        let request = nanoleaf_http_client()?.put(self.url(path)).json(body);
        self.send(request, budget, purpose).await.map(drop)
    }

    fn url(&self, path: &str) -> String {
        format!(
            "http://{}/api/v1/{}{path}",
            self.controller, self.auth_token
        )
    }

    async fn send(
        &self,
        request: BoundedRequest,
        budget: ReadBudget,
        purpose: &str,
    ) -> Result<BoundedResponse> {
        request
            .send(budget)
            .await
            .and_then(BoundedResponse::error_for_status)
            .with_context(|| format!("Nanoleaf {purpose} request to {} failed", self.controller))
    }
}
