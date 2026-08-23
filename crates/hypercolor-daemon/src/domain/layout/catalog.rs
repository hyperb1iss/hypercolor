use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use hypercolor_types::spatial::SpatialLayout;
use tokio::sync::RwLock;

#[derive(Clone)]
pub(super) struct LayoutCatalog {
    entries: Arc<RwLock<HashMap<String, SpatialLayout>>>,
    path: PathBuf,
}

impl LayoutCatalog {
    pub(super) fn new(entries: HashMap<String, SpatialLayout>, path: PathBuf) -> Self {
        Self {
            entries: Arc::new(RwLock::new(entries)),
            path,
        }
    }

    pub(super) fn entries(&self) -> &RwLock<HashMap<String, SpatialLayout>> {
        &self.entries
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    pub(super) async fn persist(&self) -> anyhow::Result<()> {
        let snapshot = self.entries.read().await.clone();
        self.save_snapshot(snapshot).await
    }

    pub(super) async fn save_snapshot(
        &self,
        snapshot: HashMap<String, SpatialLayout>,
    ) -> anyhow::Result<()> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || crate::layout_store::save(&path, &snapshot))
            .await
            .map_err(|error| anyhow::anyhow!("layout store task failed: {error}"))?
    }

    pub(super) async fn persist_best_effort(&self) {
        if let Err(error) = self.persist().await {
            tracing::warn!(
                path = %self.path.display(),
                %error,
                "Failed to persist layout store"
            );
        }
    }
}
