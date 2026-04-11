use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{Mutex, RwLock, watch};

use hypercolor_types::device::DeviceId;
use hypercolor_types::overlay::DisplayOverlayConfig;

#[derive(Debug, Default)]
pub struct DisplayOverlayRegistry {
    configs: RwLock<HashMap<DeviceId, Arc<DisplayOverlayConfig>>>,
    watchers: Mutex<HashMap<DeviceId, watch::Sender<Arc<DisplayOverlayConfig>>>>,
}

impl DisplayOverlayRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn get(&self, device_id: DeviceId) -> Arc<DisplayOverlayConfig> {
        self.configs
            .read()
            .await
            .get(&device_id)
            .cloned()
            .unwrap_or_else(empty_overlay_config)
    }

    pub async fn receiver_for(
        &self,
        device_id: DeviceId,
    ) -> watch::Receiver<Arc<DisplayOverlayConfig>> {
        if let Some(sender) = self.watchers.lock().await.get(&device_id).cloned() {
            return sender.subscribe();
        }

        let initial = self.get(device_id).await;
        let mut watchers = self.watchers.lock().await;
        let sender = watchers
            .entry(device_id)
            .or_insert_with(|| watch::channel(initial).0)
            .clone();
        sender.subscribe()
    }

    pub async fn set(&self, device_id: DeviceId, config: DisplayOverlayConfig) {
        let config = Arc::new(config.normalized());
        self.configs
            .write()
            .await
            .insert(device_id, Arc::clone(&config));

        let mut watchers = self.watchers.lock().await;
        let sender = watchers
            .entry(device_id)
            .or_insert_with(|| watch::channel(empty_overlay_config()).0);
        sender.send_replace(config);
    }

    pub async fn clear(&self, device_id: DeviceId) {
        self.configs.write().await.remove(&device_id);

        if let Some(sender) = self.watchers.lock().await.get(&device_id) {
            sender.send_replace(empty_overlay_config());
        }
    }
}

fn empty_overlay_config() -> Arc<DisplayOverlayConfig> {
    Arc::new(DisplayOverlayConfig::default())
}
