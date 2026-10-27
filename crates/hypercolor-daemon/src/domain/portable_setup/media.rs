//! Media preparation holds the ordinary library writer through publication.

use std::collections::BTreeMap;
use std::sync::Arc;

use hypercolor_core::asset::{AssetLibrary, library::portable::PreparedAssetReplacement};
use hypercolor_core::portable_setup::SetupBlobStore;
use hypercolor_types::portable_setup::PortableAsset;
use tokio::sync::{OwnedRwLockWriteGuard, RwLock};
use uuid::Uuid;

use super::internal;
use crate::domain::DomainError;
use crate::persistence::{AtomicWriteWaitOutcome, PersistenceShutdown};

pub(super) struct PreparedMedia {
    guard: OwnedRwLockWriteGuard<AssetLibrary>,
    replacement: Option<PreparedAssetReplacement>,
}

pub(super) async fn prepare_media(
    library: Arc<RwLock<AssetLibrary>>,
    before: &BTreeMap<Uuid, PortableAsset>,
    desired: &BTreeMap<Uuid, PortableAsset>,
    blobs: &SetupBlobStore,
) -> Result<PreparedMedia, DomainError> {
    let guard = library.write_owned().await;
    if guard.portable_snapshot() != *before {
        return Err(DomainError::conflict(
            "media changed during restore preparation",
        ));
    }
    let before = before.clone();
    let desired = desired.clone();
    let blobs = blobs.clone();
    tokio::task::spawn_blocking(move || {
        let replacement = guard
            .prepare_portable_replace(&before, &desired, &blobs)
            .map_err(internal)?;
        Ok(PreparedMedia {
            guard,
            replacement: Some(replacement),
        })
    })
    .await
    .map_err(internal)?
}

impl PreparedMedia {
    pub(super) fn persist(
        &mut self,
        shutdown: &PersistenceShutdown,
    ) -> Result<AtomicWriteWaitOutcome, DomainError> {
        self.replacement
            .as_mut()
            .expect("unpublished media replacement")
            .persist(shutdown)
            .map_err(internal)
    }

    pub(super) fn publish(&mut self) {
        self.replacement
            .take()
            .expect("media published once")
            .publish(&mut self.guard);
    }
}
