//! Media preparation holds the ordinary library writer through publication.

use std::collections::BTreeMap;
use std::path::PathBuf;
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
    root: PathBuf,
    operation_id: Uuid,
    desired: BTreeMap<Uuid, PortableAsset>,
}

pub(super) async fn prepare_media(
    library: Arc<RwLock<AssetLibrary>>,
    before: &BTreeMap<Uuid, PortableAsset>,
    desired: &BTreeMap<Uuid, PortableAsset>,
    blobs: &SetupBlobStore,
    operation_id: Uuid,
) -> Result<PreparedMedia, DomainError> {
    prepare(library, before, desired, blobs, operation_id, false).await
}

pub(super) async fn prepare_media_recovery(
    library: Arc<RwLock<AssetLibrary>>,
    before: &BTreeMap<Uuid, PortableAsset>,
    desired: &BTreeMap<Uuid, PortableAsset>,
    blobs: &SetupBlobStore,
    operation_id: Uuid,
) -> Result<PreparedMedia, DomainError> {
    prepare(library, before, desired, blobs, operation_id, true).await
}

async fn prepare(
    library: Arc<RwLock<AssetLibrary>>,
    before: &BTreeMap<Uuid, PortableAsset>,
    desired: &BTreeMap<Uuid, PortableAsset>,
    blobs: &SetupBlobStore,
    operation_id: Uuid,
    recovering: bool,
) -> Result<PreparedMedia, DomainError> {
    let guard = library.write_owned().await;
    if !recovering && guard.portable_snapshot() != *before {
        return Err(DomainError::conflict(
            "media changed during restore preparation",
        ));
    }
    let before = before.clone();
    let desired = desired.clone();
    let blobs = blobs.clone();
    tokio::task::spawn_blocking(move || {
        let root = guard.root().to_owned();
        let replacement = if recovering {
            guard.prepare_portable_recovery(&before, &desired, &blobs, operation_id)
        } else {
            guard.prepare_portable_replace(&before, &desired, &blobs, operation_id)
        }
        .map_err(internal)?;
        Ok(PreparedMedia {
            guard,
            replacement: Some(replacement),
            root,
            operation_id,
            desired,
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

    pub(super) fn finalize(
        &self,
        shutdown: &PersistenceShutdown,
    ) -> Result<AtomicWriteWaitOutcome, DomainError> {
        hypercolor_core::asset::library::portable::finalize_portable_restore(
            &self.root,
            self.operation_id,
            &self.desired,
            shutdown,
        )
        .map_err(internal)
    }

    pub(super) fn publish(&mut self) {
        self.replacement
            .take()
            .expect("media published once")
            .publish(&mut self.guard);
    }
}

pub(super) async fn finalize_media(
    library: Arc<RwLock<AssetLibrary>>,
    operation_id: Uuid,
    desired: &BTreeMap<Uuid, PortableAsset>,
    shutdown: &PersistenceShutdown,
) -> Result<AtomicWriteWaitOutcome, DomainError> {
    let guard = library.write_owned().await;
    let desired = desired.clone();
    let shutdown = shutdown.clone();
    tokio::task::spawn_blocking(move || {
        hypercolor_core::asset::library::portable::finalize_portable_restore(
            guard.root(),
            operation_id,
            &desired,
            &shutdown,
        )
        .map_err(internal)
    })
    .await
    .map_err(internal)?
}
