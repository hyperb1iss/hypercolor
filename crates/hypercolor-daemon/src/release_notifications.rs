//! Optional release notifications, installed explicitly by the OSS binary.

use std::{path::Path, sync::Arc, time::Duration};

use anyhow::{Context, Result};
use async_trait::async_trait;
use axum::{
    Json,
    extract::State,
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use hypercolor_core::{bus::HypercolorBus, config::ConfigManager};
use hypercolor_types::{
    api::updates::{ReleaseUpdate, UpdatePreferencesPatch, UpdateStatus},
    event::HypercolorEvent,
};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, Notify};
use tokio_util::sync::CancellationToken;
use utoipa_axum::router::OpenApiRouter;

use crate::{
    api::{
        envelope,
        openapi::{self, OperationDoc},
    },
    app_state::AppState,
    daemon::DaemonExtensionInstaller,
    domain::DomainError,
    extensions::{ApiExtension, DaemonLifecycleExtension},
    persistence::{AdmittedAtomicWrite, AtomicFileWriter},
    startup::DaemonState,
};

const CHECK_INTERVAL: Duration = Duration::from_hours(24);
const FAILURE_RETRY_INTERVAL: Duration = Duration::from_mins(15);
const RELEASE_API: &str = "https://api.github.com/repos/hyperb1iss/hypercolor/releases/latest";

/// Register this installer in OSS hosts; downstream updaters omit it entirely.
pub struct OssUpdateInstaller;

impl DaemonExtensionInstaller for OssUpdateInstaller {
    fn install(&self, daemon: &mut DaemonState) -> Result<()> {
        let service = match ReleaseNotifications::open(
            &ConfigManager::state_dir().join("release-notifications.json"),
            Arc::clone(&daemon.event_bus),
        ) {
            Ok(service) => Arc::new(service),
            Err(error) => {
                tracing::warn!(%error, "Release notifications unavailable");
                return Ok(());
            }
        };
        daemon.register_extension_state(Arc::clone(&service))?;
        daemon.register_api_extension(service.clone());
        daemon.register_lifecycle_extension(service);
        Ok(())
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
struct SavedState {
    automatic_checks: bool,
    skipped_version: Option<String>,
    last_checked: Option<DateTime<Utc>>,
    release: Option<ReleaseUpdate>,
}

impl Default for SavedState {
    fn default() -> Self {
        Self {
            automatic_checks: true,
            skipped_version: None,
            last_checked: None,
            release: None,
        }
    }
}

/// Extension-owned state; its presence also advertises the UI capability.
pub(crate) struct ReleaseNotifications {
    saved: Mutex<SavedState>,
    writer: AtomicFileWriter,
    client: reqwest::Client,
    bus: Arc<HypercolorBus>,
    wake: Notify,
    cancel: CancellationToken,
    worker: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl ReleaseNotifications {
    fn open(path: &Path, bus: Arc<HypercolorBus>) -> Result<Self> {
        let saved = match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                tracing::warn!(%error, "Release notification state is unreadable; automatic checks disabled");
                SavedState {
                    automatic_checks: false,
                    ..SavedState::default()
                }
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => SavedState::default(),
            Err(error) => return Err(error.into()),
        };
        Ok(Self {
            saved: Mutex::new(saved),
            writer: AtomicFileWriter::new(path)?,
            client: reqwest::Client::builder()
                .user_agent(concat!("Hypercolor/", env!("CARGO_PKG_VERSION")))
                .timeout(Duration::from_secs(30))
                .build()?,
            bus,
            wake: Notify::new(),
            cancel: CancellationToken::new(),
            worker: Mutex::new(None),
        })
    }

    async fn status(&self) -> UpdateStatus {
        let saved = self.saved.lock().await;
        Self::snapshot(&saved)
    }

    fn snapshot(saved: &SavedState) -> UpdateStatus {
        UpdateStatus {
            current_version: env!("CARGO_PKG_VERSION").into(),
            automatic_checks: saved.automatic_checks,
            skipped_version: saved.skipped_version.clone(),
            last_checked: saved.last_checked.map(|date| date.to_rfc3339()),
            release: saved
                .release
                .clone()
                .filter(|release| newer(&release.version, env!("CARGO_PKG_VERSION"))),
        }
    }

    fn admit(&self, saved: &mut SavedState, next: SavedState) -> Result<AdmittedAtomicWrite> {
        let bytes = serde_json::to_vec_pretty(&next)?;
        let pending = self.writer.reserve().admit(bytes);
        // Admission retains this intent for retry even if the caller is cancelled.
        *saved = next;
        Ok(pending)
    }

    async fn commit(pending: AdmittedAtomicWrite) {
        match tokio::task::spawn_blocking(move || pending.commit()).await {
            Ok(Ok(_)) => {}
            Ok(Err(error)) => {
                tracing::warn!(%error, "Failed to persist release notifications; retry remains active");
            }
            Err(error) => {
                tracing::warn!(%error, "Release notification persistence worker failed; retry remains active");
            }
        }
    }

    fn publish(&self) {
        self.bus.publish(HypercolorEvent::ExtensionStateChanged {
            source: "oss-updates".into(),
            kind: "status".into(),
            payload: serde_json::Value::Null,
        });
    }

    async fn patch(&self, patch: UpdatePreferencesPatch) -> Result<UpdateStatus, DomainError> {
        if patch.clear_skipped_version && patch.skipped_version.is_some() {
            return Err(DomainError::validation(
                "Cannot set and clear skipped_version together",
            ));
        }
        if let Some(version) = &patch.skipped_version {
            semver::Version::parse(version).map_err(|_| {
                DomainError::validation("skipped_version must be a semantic version")
            })?;
        }
        let mut saved = self.saved.lock().await;
        let mut next = saved.clone();
        if let Some(enabled) = patch.automatic_checks {
            next.automatic_checks = enabled;
        }
        if patch.clear_skipped_version {
            next.skipped_version = None;
        }
        if let Some(version) = patch.skipped_version {
            next.skipped_version = Some(version);
        }
        let pending = self
            .admit(&mut saved, next)
            .map_err(DomainError::Internal)?;
        let status = Self::snapshot(&saved);
        drop(saved);
        self.wake.notify_one();
        self.publish();
        Self::commit(pending).await;
        Ok(status)
    }

    async fn check(&self) -> Result<UpdateStatus> {
        self.check_url(RELEASE_API).await
    }

    async fn check_url(&self, url: &str) -> Result<UpdateStatus> {
        let release = self
            .client
            .get(url)
            .header("Accept", "application/vnd.github+json")
            .send()
            .await?
            .error_for_status()?
            .json::<GithubRelease>()
            .await?;
        let available = release.into_update(
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            std::env::consts::ARCH,
        )?;
        let mut saved = self.saved.lock().await;
        let mut next = saved.clone();
        next.release = available;
        next.last_checked = Some(Utc::now());
        let pending = self.admit(&mut saved, next)?;
        let status = Self::snapshot(&saved);
        drop(saved);
        self.publish();
        Self::commit(pending).await;
        Ok(status)
    }

    async fn run(self: Arc<Self>) {
        self.run_with_url(RELEASE_API, FAILURE_RETRY_INTERVAL).await;
    }

    async fn run_with_url(self: Arc<Self>, url: &str, retry_interval: Duration) {
        loop {
            let delay = {
                let saved = self.saved.lock().await;
                if saved.automatic_checks {
                    saved
                        .last_checked
                        .and_then(|checked| {
                            (checked + chrono::Duration::days(1) - Utc::now())
                                .to_std()
                                .ok()
                        })
                        .unwrap_or(Duration::ZERO)
                } else {
                    CHECK_INTERVAL
                }
            };
            tokio::select! {
                () = self.cancel.cancelled() => return,
                () = self.wake.notified() => continue,
                () = tokio::time::sleep(delay) => {}
            }
            if !self.saved.lock().await.automatic_checks {
                continue;
            }
            tokio::select! {
                () = self.cancel.cancelled() => return,
                result = self.check_url(url) => {
                    if let Err(error) = result {
                        tracing::debug!(%error, "Release notification check failed");
                        // Recover from an offline startup without waiting another day.
                        tokio::select! {
                            () = self.cancel.cancelled() => return,
                            () = self.wake.notified() => {},
                            () = tokio::time::sleep(retry_interval) => {},
                        }
                    }
                }
            }
        }
    }
}

#[async_trait]
impl DaemonLifecycleExtension for ReleaseNotifications {
    fn name(&self) -> &'static str {
        "oss-release-notifications"
    }
    async fn start(&self, daemon: &DaemonState) -> Result<()> {
        let service = daemon
            .extensions
            .get::<Self>()
            .context("release notifications not registered")?;
        *self.worker.lock().await = Some(tokio::spawn(service.run()));
        Ok(())
    }
    async fn shutdown(&self, _: &DaemonState) -> Result<()> {
        self.cancel.cancel();
        if let Some(worker) = self.worker.lock().await.take() {
            worker.await?;
        }
        Ok(())
    }
}

impl ApiExtension for ReleaseNotifications {
    fn name(&self) -> &'static str {
        "oss-release-notifications"
    }
    fn mount_api_routes(
        &self,
        router: OpenApiRouter<Arc<AppState>>,
    ) -> OpenApiRouter<Arc<AppState>> {
        router
            .routes(openapi::documented_route(
                "/release-notifications",
                axum::routing::get(get_status).patch(patch_preferences),
                [
                    OperationDoc::get::<UpdateStatus>(
                        "release_notifications",
                        "system",
                        "Read release notifications",
                    ),
                    OperationDoc::patch::<UpdateStatus>(
                        "release_notification_preferences",
                        "system",
                        "Set release notification preferences",
                    )
                    .body::<UpdatePreferencesPatch>(),
                ],
            ))
            .routes(openapi::documented_route(
                "/release-notifications/check",
                axum::routing::post(check_release),
                [OperationDoc::post::<UpdateStatus>(
                    "check_release_notifications",
                    "system",
                    "Check GitHub for a stable release",
                )],
            ))
    }
}

fn service(state: &AppState) -> Result<Arc<ReleaseNotifications>, DomainError> {
    state
        .extensions
        .get()
        .ok_or_else(|| DomainError::service_unavailable("Release notifications are disabled"))
}

async fn get_status(State(state): State<Arc<AppState>>) -> Response {
    match service(&state) {
        Ok(service) => envelope::ok(service.status().await),
        Err(error) => error.into_response(),
    }
}
async fn patch_preferences(
    State(state): State<Arc<AppState>>,
    Json(patch): Json<UpdatePreferencesPatch>,
) -> Response {
    match service(&state) {
        Ok(service) => match service.patch(patch).await {
            Ok(status) => envelope::ok(status),
            Err(error) => error.into_response(),
        },
        Err(error) => error.into_response(),
    }
}
async fn check_release(State(state): State<Arc<AppState>>) -> Response {
    match service(&state) {
        Ok(service) => match service.check().await {
            Ok(status) => envelope::ok(status),
            Err(error) => {
                tracing::warn!(%error, "Manual release check failed");
                DomainError::service_unavailable(
                    "Could not check GitHub. Try again later or visit the releases page.",
                )
                .into_response()
            }
        },
        Err(error) => error.into_response(),
    }
}

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
    name: Option<String>,
    body: Option<String>,
    html_url: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<GithubAsset>,
}
#[derive(Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
}

fn newer(candidate: &str, current: &str) -> bool {
    match (
        semver::Version::parse(candidate.trim_start_matches('v')),
        semver::Version::parse(current),
    ) {
        (Ok(candidate), Ok(current)) => {
            candidate.pre.is_empty() && candidate.cmp_precedence(&current).is_gt()
        }
        _ => false,
    }
}

impl GithubRelease {
    fn into_update(self, current: &str, os: &str, arch: &str) -> Result<Option<ReleaseUpdate>> {
        let version = semver::Version::parse(self.tag_name.trim_start_matches('v'))?;
        if self.draft || self.prerelease || !newer(&version.to_string(), current) {
            return Ok(None);
        }
        let expected = match (os, arch) {
            ("macos", "aarch64") => Some(format!("Hypercolor-{version}-arm64.dmg")),
            ("windows", "x86_64") => Some(format!("Hypercolor_{version}_x64-setup.exe")),
            ("linux", "x86_64") => Some(format!("hypercolor-{version}-linux-amd64.tar.gz")),
            ("linux", "aarch64") => Some(format!("hypercolor-{version}-linux-arm64.tar.gz")),
            _ => None,
        };
        let download_url = expected
            .and_then(|name| self.assets.iter().find(|asset| asset.name == name))
            .map(|asset| asset.browser_download_url.clone())
            .filter(|url| {
                url.starts_with("https://github.com/hyperb1iss/hypercolor/releases/download/")
            });
        anyhow::ensure!(
            self.html_url
                .starts_with("https://github.com/hyperb1iss/hypercolor/releases/tag/"),
            "Unexpected release URL"
        );
        Ok(Some(ReleaseUpdate {
            version: version.to_string(),
            name: self.name.unwrap_or(self.tag_name),
            notes: self.body.unwrap_or_default(),
            release_url: self.html_url,
            download_url,
        }))
    }
}

#[cfg(test)]
#[path = "../tests/release_notifications/unit.rs"]
mod tests;
