//! API credentials supplied by a downstream daemon build.
//!
//! The environment keys are the only credentials the engine mints on its
//! own. A downstream build that issues per-client credentials installs a
//! [`CredentialAuthority`]; the security middleware consults it after the
//! environment keys, an installed authority turns authentication on for
//! every non-loopback request, and the startup bind rule accepts it in
//! place of `HYPERCOLOR_API_KEY` when it can grant control.
//!
//! Authority credentials are client credentials. They grant read or
//! control and never protected control, which stays with the operator's
//! own credentials: the control environment key, the launcher session,
//! and in-process trusted control.

use std::fmt;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

/// Access an authority credential grants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CredentialTier {
    /// `GET`, `HEAD`, and `OPTIONS` requests only.
    Read,
    /// Every request a control key may make, except protected control.
    Control,
}

/// A credential an authority recognized.
///
/// The engine inserts the grant into the request's extensions, so a
/// handler can read `Option<Extension<CredentialGrant>>` to learn which
/// credential called. Cancelling the revocation token ends every
/// long-lived session the credential opened, such as a WebSocket.
#[derive(Clone)]
pub struct CredentialGrant {
    tier: CredentialTier,
    credential_id: Arc<str>,
    revocation: CancellationToken,
}

impl CredentialGrant {
    /// Describe a recognized credential.
    #[must_use]
    pub fn new(
        tier: CredentialTier,
        credential_id: impl Into<Arc<str>>,
        revocation: CancellationToken,
    ) -> Self {
        Self {
            tier,
            credential_id: credential_id.into(),
            revocation,
        }
    }

    /// The access this credential grants.
    #[must_use]
    pub const fn tier(&self) -> CredentialTier {
        self.tier
    }

    /// The authority's non-secret name for this credential.
    #[must_use]
    pub fn credential_id(&self) -> &str {
        &self.credential_id
    }

    /// Fires when the credential is revoked.
    #[must_use]
    pub const fn revocation(&self) -> &CancellationToken {
        &self.revocation
    }

    /// Clamp the grant to the highest tier its authority may issue.
    pub(crate) fn clamped_to(mut self, ceiling: CredentialTier) -> Self {
        self.tier = self.tier.min(ceiling);
        self
    }
}

impl fmt::Debug for CredentialGrant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CredentialGrant")
            .field("tier", &self.tier)
            .field("credential_id", &self.credential_id)
            .field("revoked", &self.revocation.is_cancelled())
            .finish()
    }
}

/// A source of API credentials beyond the environment keys.
///
/// The middleware calls [`authenticate`](Self::authenticate) on the
/// request path for every presented bearer token that is not the launcher
/// session credential, so an implementation must not block or perform
/// I/O, and must compare secrets in constant time.
pub trait CredentialAuthority: Send + Sync + 'static {
    /// The highest tier this authority can ever grant.
    ///
    /// Fixed for the life of the process. The startup bind rule reads it
    /// before any credential exists: an authority that can grant control
    /// satisfies a network bind the way `HYPERCOLOR_API_KEY` does. Grants
    /// above the ceiling are clamped to it.
    fn ceiling(&self) -> CredentialTier;

    /// Resolve a presented bearer token, or `None` when it is not one of
    /// this authority's live credentials.
    fn authenticate(&self, presented: &str) -> Option<CredentialGrant>;
}
