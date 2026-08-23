use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::sync::RwLock;

use hypercolor_types::scene::{SceneId, ZoneId};
use hypercolor_types::spatial::SpatialLayout;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ZoneLayoutPreviewOwner(uuid::Uuid);

impl ZoneLayoutPreviewOwner {
    #[must_use]
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }
}

impl Default for ZoneLayoutPreviewOwner {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
struct OwnedLayoutPreview {
    owner: ZoneLayoutPreviewOwner,
    layout: SpatialLayout,
}

#[derive(Debug, Default)]
pub struct ZoneLayoutPreviewStore {
    layouts: RwLock<HashMap<(SceneId, ZoneId), OwnedLayoutPreview>>,
    generation: AtomicU64,
}

impl ZoneLayoutPreviewStore {
    #[cfg(test)]
    pub(crate) async fn block_writes_for_test(
        &self,
        entered: tokio::sync::oneshot::Sender<()>,
        release: tokio::sync::oneshot::Receiver<()>,
    ) {
        let _guard = self.layouts.write().await;
        let _ = entered.send(());
        let _ = release.await;
    }

    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    pub async fn set(
        &self,
        owner: ZoneLayoutPreviewOwner,
        scene_id: SceneId,
        zone_id: ZoneId,
        layout: SpatialLayout,
    ) {
        let mut layouts = self.layouts.write().await;
        self.bump_generation();
        layouts.insert((scene_id, zone_id), OwnedLayoutPreview { owner, layout });
    }

    pub async fn clear(&self, scene_id: SceneId, zone_id: ZoneId) -> bool {
        let mut layouts = self.layouts.write().await;
        let removed = layouts.remove(&(scene_id, zone_id)).is_some();
        if removed {
            self.bump_generation();
        }
        removed
    }

    pub async fn clear_many<I>(&self, keys: I) -> bool
    where
        I: IntoIterator<Item = (SceneId, ZoneId)>,
    {
        let mut layouts = self.layouts.write().await;
        let mut removed = false;
        for key in keys {
            removed |= layouts.remove(&key).is_some();
        }
        if removed {
            self.bump_generation();
        }
        removed
    }

    pub async fn clear_owned_many<I>(&self, owner: ZoneLayoutPreviewOwner, keys: I) -> bool
    where
        I: IntoIterator<Item = (SceneId, ZoneId)>,
    {
        let mut layouts = self.layouts.write().await;
        let mut removed = false;
        for key in keys {
            if layouts
                .get(&key)
                .is_some_and(|preview| preview.owner == owner)
            {
                layouts.remove(&key);
                removed = true;
            }
        }
        if removed {
            self.bump_generation();
        }
        removed
    }

    pub async fn clear_scene(&self, scene_id: SceneId) -> bool {
        let mut layouts = self.layouts.write().await;
        let previous_len = layouts.len();
        layouts.retain(|(candidate_scene_id, _), _| *candidate_scene_id != scene_id);
        let removed = layouts.len() != previous_len;
        if removed {
            self.bump_generation();
        }
        removed
    }

    pub(crate) async fn clear_at_scene_commit(
        &self,
        scenes: &[SceneId],
        zones: &[(SceneId, ZoneId)],
    ) -> bool {
        if scenes.is_empty() && zones.is_empty() {
            return false;
        }
        let mut layouts = self.layouts.write().await;
        let previous_len = layouts.len();
        layouts.retain(|(scene_id, zone_id), _| {
            !scenes.contains(scene_id) && !zones.contains(&(*scene_id, *zone_id))
        });
        let removed = layouts.len() != previous_len;
        if removed {
            self.bump_generation();
        }
        removed
    }

    pub async fn scene_overrides_with_generation(
        &self,
        scene_id: SceneId,
    ) -> (u64, HashMap<ZoneId, SpatialLayout>) {
        let layouts = self.layouts.read().await;
        let overrides = layouts
            .iter()
            .filter(|((candidate_scene_id, _), _)| *candidate_scene_id == scene_id)
            .map(|((_, zone_id), preview)| (*zone_id, preview.layout.clone()))
            .collect();
        (self.generation(), overrides)
    }

    pub async fn scene_overrides(&self, scene_id: SceneId) -> HashMap<ZoneId, SpatialLayout> {
        self.scene_overrides_with_generation(scene_id).await.1
    }

    fn bump_generation(&self) -> u64 {
        self.generation.fetch_add(1, Ordering::AcqRel) + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hypercolor_types::spatial::{EdgeBehavior, SamplingMode};

    fn layout(id: &str) -> SpatialLayout {
        SpatialLayout {
            id: id.to_owned(),
            name: id.to_owned(),
            description: None,
            canvas_width: 320,
            canvas_height: 200,
            zones: Vec::new(),
            default_sampling_mode: SamplingMode::Bilinear,
            default_edge_behavior: EdgeBehavior::Clamp,
            version: 1,
        }
    }

    #[tokio::test]
    async fn store_tracks_scene_overrides_and_generation() {
        let store = ZoneLayoutPreviewStore::default();
        let scene_id = SceneId::new();
        let other_scene_id = SceneId::new();
        let zone_id = ZoneId::new();
        let other_zone_id = ZoneId::new();
        let owner = ZoneLayoutPreviewOwner::new();

        assert_eq!(store.generation(), 0);
        store.set(owner, scene_id, zone_id, layout("preview")).await;
        store
            .set(owner, other_scene_id, other_zone_id, layout("other"))
            .await;
        assert_eq!(store.generation(), 2);

        let overrides = store.scene_overrides(scene_id).await;
        assert_eq!(overrides.len(), 1);
        assert_eq!(
            overrides.get(&zone_id).map(|layout| layout.id.as_str()),
            Some("preview")
        );

        assert!(store.clear(scene_id, zone_id).await);
        assert_eq!(store.generation(), 3);
        assert!(store.scene_overrides(scene_id).await.is_empty());
    }

    #[tokio::test]
    async fn clear_many_bumps_once_when_any_override_is_removed() {
        let store = ZoneLayoutPreviewStore::default();
        let scene_id = SceneId::new();
        let first_zone_id = ZoneId::new();
        let second_zone_id = ZoneId::new();
        let owner = ZoneLayoutPreviewOwner::new();

        store
            .set(owner, scene_id, first_zone_id, layout("first"))
            .await;
        store
            .set(owner, scene_id, second_zone_id, layout("second"))
            .await;
        assert_eq!(store.generation(), 2);

        assert!(
            store
                .clear_many([(scene_id, first_zone_id), (scene_id, second_zone_id)])
                .await
        );
        assert_eq!(store.generation(), 3);
        assert!(!store.clear_many([(scene_id, first_zone_id)]).await);
        assert_eq!(store.generation(), 3);
    }

    #[tokio::test]
    async fn owner_conditional_clear_preserves_a_newer_clients_preview() {
        let store = ZoneLayoutPreviewStore::default();
        let first_owner = ZoneLayoutPreviewOwner::new();
        let second_owner = ZoneLayoutPreviewOwner::new();
        let scene_id = SceneId::new();
        let zone_id = ZoneId::new();

        store
            .set(first_owner, scene_id, zone_id, layout("first"))
            .await;
        store
            .set(second_owner, scene_id, zone_id, layout("second"))
            .await;

        assert!(
            !store
                .clear_owned_many(first_owner, [(scene_id, zone_id)])
                .await
        );
        assert_eq!(
            store
                .scene_overrides(scene_id)
                .await
                .get(&zone_id)
                .map(|layout| layout.id.as_str()),
            Some("second")
        );
        assert!(
            store
                .clear_owned_many(second_owner, [(scene_id, zone_id)])
                .await
        );
    }

    #[tokio::test]
    async fn clearing_a_scene_prevents_preview_revival_after_reactivation() {
        let store = ZoneLayoutPreviewStore::default();
        let owner = ZoneLayoutPreviewOwner::new();
        let scene_a = SceneId::new();
        let scene_b = SceneId::new();
        let zone_id = ZoneId::new();

        store.set(owner, scene_a, zone_id, layout("scene-a")).await;
        store.set(owner, scene_b, zone_id, layout("scene-b")).await;
        assert!(store.clear_scene(scene_a).await);
        assert!(store.scene_overrides(scene_a).await.is_empty());
        assert_eq!(store.scene_overrides(scene_b).await.len(), 1);
    }
}
