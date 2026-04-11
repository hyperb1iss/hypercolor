use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;

use tokio::sync::{Mutex, RwLock, watch};

use hypercolor_types::device::DeviceId;
use hypercolor_types::overlay::{DisplayOverlayConfig, OverlaySlot, OverlaySlotId};
use serde::Serialize;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OverlaySlotStatus {
    Active,
    Disabled,
    Failed,
    HtmlGated,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlaySlotRuntime {
    pub last_rendered_at: Option<SystemTime>,
    pub consecutive_failures: u32,
    pub last_error: Option<String>,
    pub status: OverlaySlotStatus,
}

impl OverlaySlotRuntime {
    #[must_use]
    pub fn from_slot(slot: &OverlaySlot) -> Self {
        Self {
            last_rendered_at: None,
            consecutive_failures: 0,
            last_error: None,
            status: if slot.enabled {
                OverlaySlotStatus::Active
            } else {
                OverlaySlotStatus::Disabled
            },
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DisplayOverlayRuntime {
    pub slots: HashMap<OverlaySlotId, OverlaySlotRuntime>,
}

impl DisplayOverlayRuntime {
    #[must_use]
    pub fn slot(&self, slot_id: OverlaySlotId) -> Option<&OverlaySlotRuntime> {
        self.slots.get(&slot_id)
    }
}

#[derive(Debug, Default)]
pub struct DisplayOverlayRuntimeRegistry {
    runtimes: RwLock<HashMap<DeviceId, Arc<DisplayOverlayRuntime>>>,
}

impl DisplayOverlayRuntimeRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn get(&self, device_id: DeviceId) -> Arc<DisplayOverlayRuntime> {
        self.runtimes
            .read()
            .await
            .get(&device_id)
            .cloned()
            .unwrap_or_else(empty_overlay_runtime)
    }

    pub async fn set(&self, device_id: DeviceId, runtime: DisplayOverlayRuntime) {
        self.runtimes
            .write()
            .await
            .insert(device_id, Arc::new(runtime));
    }

    pub async fn clear(&self, device_id: DeviceId) {
        self.runtimes.write().await.remove(&device_id);
    }
}

fn empty_overlay_runtime() -> Arc<DisplayOverlayRuntime> {
    Arc::new(DisplayOverlayRuntime::default())
}
