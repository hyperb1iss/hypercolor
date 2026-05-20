use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::sync::RwLock;

use hypercolor_types::scene::{SceneId, ZoneId};
use hypercolor_types::spatial::SpatialLayout;

#[derive(Debug, Default)]
pub struct ZoneLayoutPreviewStore {
    layouts: RwLock<HashMap<(SceneId, ZoneId), SpatialLayout>>,
    generation: AtomicU64,
}

impl ZoneLayoutPreviewStore {
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    pub async fn set(&self, scene_id: SceneId, zone_id: ZoneId, layout: SpatialLayout) {
        let mut layouts = self.layouts.write().await;
        layouts.insert((scene_id, zone_id), layout);
        self.bump_generation();
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

    pub async fn scene_overrides_with_generation(
        &self,
        scene_id: SceneId,
    ) -> (u64, HashMap<ZoneId, SpatialLayout>) {
        let layouts = self.layouts.read().await;
        let overrides = layouts
            .iter()
            .filter(|((candidate_scene_id, _), _)| *candidate_scene_id == scene_id)
            .map(|((_, zone_id), layout)| (*zone_id, layout.clone()))
            .collect();
        (self.generation(), overrides)
    }

    pub async fn scene_overrides(&self, scene_id: SceneId) -> HashMap<ZoneId, SpatialLayout> {
        self.scene_overrides_with_generation(scene_id).await.1
    }

    fn bump_generation(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
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
            spaces: None,
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

        assert_eq!(store.generation(), 0);
        store.set(scene_id, zone_id, layout("preview")).await;
        store
            .set(other_scene_id, other_zone_id, layout("other"))
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

        store.set(scene_id, first_zone_id, layout("first")).await;
        store.set(scene_id, second_zone_id, layout("second")).await;
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
}
