use std::sync::{Arc, Mutex, MutexGuard};

use arc_swap::ArcSwap;
use hypercolor_core::input::browser::{
    BrowserInputAttachment, BrowserInputChildKey, BrowserInputPublicationId,
    BrowserInputRegistryHandle,
};
use hypercolor_core::input::routing::{InteractionRouteRequest, SourceIncarnation};
use hypercolor_types::config::InteractionRoutePolicy;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthoritativeBrowserLease {
    pub key: BrowserInputChildKey,
    pub publication_id: BrowserInputPublicationId,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InteractionRoutingSnapshot {
    pub generation: u64,
    pub config_generation: u64,
    pub daemon_policy: InteractionRoutePolicy,
    pub preview_policy: InteractionRoutePolicy,
    pub authoritative_browser: Option<AuthoritativeBrowserLease>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthoritativeClaimOutcome {
    Granted,
    AlreadyOwned,
}

#[derive(Debug, thiserror::Error, Eq, PartialEq)]
pub enum AuthoritativeClaimError {
    #[error("interactive preview is no longer active")]
    PreviewInactive,
    #[error("another interactive preview owns authoritative browser input")]
    Conflict,
}

#[derive(Clone)]
pub struct InteractionRoutingControl {
    browser_registry: BrowserInputRegistryHandle,
    snapshot: Arc<ArcSwap<InteractionRoutingSnapshot>>,
    writer: Arc<Mutex<()>>,
}

impl InteractionRoutingControl {
    #[must_use]
    pub fn new(
        browser_registry: BrowserInputRegistryHandle,
        config_generation: u64,
        daemon_policy: InteractionRoutePolicy,
        preview_policy: InteractionRoutePolicy,
    ) -> Self {
        Self {
            browser_registry,
            snapshot: Arc::new(ArcSwap::from_pointee(InteractionRoutingSnapshot {
                generation: 1,
                config_generation,
                daemon_policy,
                preview_policy,
                authoritative_browser: None,
            })),
            writer: Arc::new(Mutex::new(())),
        }
    }

    #[must_use]
    pub fn snapshot(&self) -> Arc<InteractionRoutingSnapshot> {
        self.snapshot.load_full()
    }

    pub fn publish_policies(
        &self,
        config_generation: u64,
        daemon_policy: InteractionRoutePolicy,
        preview_policy: InteractionRoutePolicy,
    ) -> Arc<InteractionRoutingSnapshot> {
        let _writer = self.lock_writer();
        let current = self.snapshot.load_full();
        if current.config_generation == config_generation
            && current.daemon_policy == daemon_policy
            && current.preview_policy == preview_policy
        {
            return current;
        }

        self.publish_locked(InteractionRoutingSnapshot {
            generation: next_generation(current.generation),
            config_generation,
            daemon_policy,
            preview_policy,
            authoritative_browser: current.authoritative_browser.clone(),
        })
    }

    pub fn claim_authoritative(
        &self,
        attachment: &BrowserInputAttachment,
    ) -> Result<AuthoritativeClaimOutcome, AuthoritativeClaimError> {
        let _writer = self.lock_writer();
        if !self.attachment_is_active(attachment) {
            return Err(AuthoritativeClaimError::PreviewInactive);
        }

        let current = self.snapshot.load_full();
        if let Some(owner) = current.authoritative_browser.as_ref() {
            if lease_matches_attachment(owner, attachment) {
                return Ok(AuthoritativeClaimOutcome::AlreadyOwned);
            }
            if self.lease_is_active(owner) {
                return Err(AuthoritativeClaimError::Conflict);
            }
        }

        self.publish_locked(InteractionRoutingSnapshot {
            generation: next_generation(current.generation),
            config_generation: current.config_generation,
            daemon_policy: current.daemon_policy,
            preview_policy: current.preview_policy,
            authoritative_browser: Some(AuthoritativeBrowserLease {
                key: attachment.key().clone(),
                publication_id: attachment.publication_id(),
            }),
        });
        Ok(AuthoritativeClaimOutcome::Granted)
    }

    pub fn release_authoritative(&self, attachment: &BrowserInputAttachment) -> bool {
        let _writer = self.lock_writer();
        let current = self.snapshot.load_full();
        if !current
            .authoritative_browser
            .as_ref()
            .is_some_and(|owner| lease_matches_attachment(owner, attachment))
        {
            return false;
        }

        self.publish_locked(InteractionRoutingSnapshot {
            generation: next_generation(current.generation),
            config_generation: current.config_generation,
            daemon_policy: current.daemon_policy,
            preview_policy: current.preview_policy,
            authoritative_browser: None,
        });
        true
    }

    pub fn close_preview(&self, attachment: &BrowserInputAttachment) -> bool {
        let closed = attachment.close();
        let released = self.release_authoritative(attachment);
        closed || released
    }

    #[must_use]
    pub fn daemon_request(&self) -> InteractionRouteRequest {
        let snapshot = self.snapshot();
        InteractionRouteRequest {
            policy: snapshot.daemon_policy,
            browser_source: snapshot
                .authoritative_browser
                .as_ref()
                .map(|owner| SourceIncarnation::browser_child(owner.publication_id.get())),
        }
    }

    #[must_use]
    pub fn preview_request(&self, attachment: &BrowserInputAttachment) -> InteractionRouteRequest {
        InteractionRouteRequest {
            policy: self.snapshot().preview_policy,
            browser_source: Some(SourceIncarnation::browser_child(
                attachment.publication_id().get(),
            )),
        }
    }

    fn attachment_is_active(&self, attachment: &BrowserInputAttachment) -> bool {
        self.browser_registry
            .snapshot()
            .child(attachment.key())
            .is_some_and(|child| {
                child.is_active() && child.publication_id() == attachment.publication_id()
            })
    }

    fn lease_is_active(&self, lease: &AuthoritativeBrowserLease) -> bool {
        self.browser_registry
            .snapshot()
            .child(&lease.key)
            .is_some_and(|child| {
                child.is_active() && child.publication_id() == lease.publication_id
            })
    }

    fn publish_locked(
        &self,
        snapshot: InteractionRoutingSnapshot,
    ) -> Arc<InteractionRoutingSnapshot> {
        let snapshot = Arc::new(snapshot);
        self.snapshot.store(Arc::clone(&snapshot));
        snapshot
    }

    fn lock_writer(&self) -> MutexGuard<'_, ()> {
        self.writer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn lease_matches_attachment(
    lease: &AuthoritativeBrowserLease,
    attachment: &BrowserInputAttachment,
) -> bool {
    lease.key == *attachment.key() && lease.publication_id == attachment.publication_id()
}

fn next_generation(current: u64) -> u64 {
    current
        .checked_add(1)
        .expect("interaction routing generation exhausted")
}

#[cfg(test)]
mod tests;
