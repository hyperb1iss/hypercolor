//! Virtual display simulator persistence and daemon-local backend wiring.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, RwLock};

use hypercolor_core::device::{BackendInfo, DeviceBackend, DiscoveryConnectBehavior};
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceColorSpace, DeviceFamily,
    DeviceFeatures, DeviceFingerprint, DeviceId, DeviceInfo, ZoneInfo,
};

use crate::discovery::{
    DiscoveryRuntime, apply_persisted_device_settings, execute_lifecycle_actions,
    sync_registry_state,
};
use crate::logical_devices::LogicalDevice;

pub const SIMULATED_DISPLAY_BACKEND_ID: &str = "simulator";
const SIMULATED_DISPLAY_FAMILY: &str = "simulator";
const DEFAULT_SIMULATED_DISPLAY_FPS: u32 = 15;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimulatedDisplayConfig {
    pub id: DeviceId,
    pub name: String,
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub circular: bool,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

impl SimulatedDisplayConfig {
    #[must_use]
    pub fn normalized(mut self) -> Self {
        self.name = self.name.trim().to_owned();
        if self.name.is_empty() {
            self.name = format!("Simulated Display {}", self.id);
        }
        self.width = self.width.max(1);
        self.height = self.height.max(1);
        self
    }

    #[must_use]
    pub fn device_info(&self) -> DeviceInfo {
        DeviceInfo {
            id: self.id,
            name: self.name.clone(),
            vendor: "Hypercolor".to_owned(),
            family: DeviceFamily::Custom(SIMULATED_DISPLAY_FAMILY.to_owned()),
            model: Some("virtual_display".to_owned()),
            connection_type: ConnectionType::Bridge,
            zones: vec![ZoneInfo {
                name: "Display".to_owned(),
                led_count: 0,
                topology: hypercolor_types::device::DeviceTopologyHint::Display {
                    width: self.width,
                    height: self.height,
                    circular: self.circular,
                },
                color_format: DeviceColorFormat::Jpeg,
            }],
            firmware_version: None,
            capabilities: DeviceCapabilities {
                led_count: 0,
                supports_direct: false,
                supports_brightness: false,
                has_display: true,
                display_resolution: Some((self.width, self.height)),
                max_fps: DEFAULT_SIMULATED_DISPLAY_FPS,
                color_space: DeviceColorSpace::Rgb,
                features: DeviceFeatures::default(),
            },
        }
    }

    #[must_use]
    pub fn fingerprint(&self) -> DeviceFingerprint {
        DeviceFingerprint(format!("{SIMULATED_DISPLAY_BACKEND_ID}:{}", self.id))
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct PersistedSimulatedDisplaySnapshot {
    #[serde(default)]
    displays: Vec<SimulatedDisplayConfig>,
}

#[derive(Debug, Clone)]
pub struct SimulatedDisplayStore {
    path: PathBuf,
    displays: HashMap<DeviceId, SimulatedDisplayConfig>,
}

impl SimulatedDisplayStore {
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            displays: HashMap::new(),
        }
    }

    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::new(path.to_path_buf()));
        }

        let raw = fs::read_to_string(path)
            .with_context(|| format!("failed to read simulated displays at {}", path.display()))?;
        let snapshot: PersistedSimulatedDisplaySnapshot = serde_json::from_str(&raw)
            .with_context(|| format!("failed to parse simulated displays at {}", path.display()))?;

        let mut store = Self::new(path.to_path_buf());
        for display in snapshot.displays {
            store.upsert(display);
        }
        Ok(store)
    }

    #[must_use]
    pub fn list(&self) -> Vec<SimulatedDisplayConfig> {
        let mut displays: Vec<_> = self.displays.values().cloned().collect();
        displays.sort_by(|left, right| left.name.cmp(&right.name).then(left.id.0.cmp(&right.id.0)));
        displays
    }

    #[must_use]
    pub fn get(&self, id: DeviceId) -> Option<SimulatedDisplayConfig> {
        self.displays.get(&id).cloned()
    }

    pub fn upsert(&mut self, config: SimulatedDisplayConfig) {
        let normalized = config.normalized();
        self.displays.insert(normalized.id, normalized);
    }

    pub fn remove(&mut self, id: DeviceId) -> Option<SimulatedDisplayConfig> {
        self.displays.remove(&id)
    }

    pub fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!(
                    "failed to create simulated display directory {}",
                    parent.display()
                )
            })?;
        }

        let payload = serde_json::to_string_pretty(&PersistedSimulatedDisplaySnapshot {
            displays: self.list(),
        })
        .context("failed to serialize simulated displays")?;
        let tmp_path = self.path.with_extension("tmp");
        fs::write(&tmp_path, payload).with_context(|| {
            format!(
                "failed to write temporary simulated displays {}",
                tmp_path.display()
            )
        })?;
        fs::rename(&tmp_path, &self.path).with_context(|| {
            format!(
                "failed to move temporary simulated displays {} into {}",
                tmp_path.display(),
                self.path.display()
            )
        })?;
        Ok(())
    }
}

fn default_enabled() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimulatedDisplayFrame {
    pub jpeg_data: Arc<Vec<u8>>,
    pub captured_at: SystemTime,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Default)]
pub struct SimulatedDisplayRuntime {
    frames: HashMap<DeviceId, SimulatedDisplayFrame>,
}

impl SimulatedDisplayRuntime {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_frame(&mut self, device_id: DeviceId, frame: SimulatedDisplayFrame) {
        self.frames.insert(device_id, frame);
    }

    #[must_use]
    pub fn frame(&self, device_id: DeviceId) -> Option<SimulatedDisplayFrame> {
        self.frames.get(&device_id).cloned()
    }

    pub fn remove(&mut self, device_id: DeviceId) {
        self.frames.remove(&device_id);
    }
}

pub struct SimulatedDisplayBackend {
    store: Arc<RwLock<SimulatedDisplayStore>>,
    runtime: Arc<RwLock<SimulatedDisplayRuntime>>,
    connected: HashSet<DeviceId>,
}

impl SimulatedDisplayBackend {
    #[must_use]
    pub fn new(
        store: Arc<RwLock<SimulatedDisplayStore>>,
        runtime: Arc<RwLock<SimulatedDisplayRuntime>>,
    ) -> Self {
        Self {
            store,
            runtime,
            connected: HashSet::new(),
        }
    }

    async fn store_display_frame(&self, id: &DeviceId, jpeg_data: Arc<Vec<u8>>) -> Result<()> {
        if !self.connected.contains(id) {
            bail!("simulated display {id} is not connected");
        }
        let store = self.store.read().await;
        let Some(config) = store.get(*id) else {
            bail!("simulated display {id} is not configured");
        };
        self.runtime.write().await.set_frame(
            *id,
            SimulatedDisplayFrame {
                jpeg_data,
                captured_at: SystemTime::now(),
                width: config.width,
                height: config.height,
            },
        );
        Ok(())
    }
}

#[async_trait::async_trait]
impl DeviceBackend for SimulatedDisplayBackend {
    fn info(&self) -> BackendInfo {
        BackendInfo {
            id: SIMULATED_DISPLAY_BACKEND_ID.to_owned(),
            name: "Virtual Display Simulator".to_owned(),
            description: "Daemon-local virtual LCD devices for layout and overlay workflows"
                .to_owned(),
        }
    }

    async fn discover(&mut self) -> Result<Vec<DeviceInfo>> {
        let store = self.store.read().await;
        Ok(store
            .list()
            .into_iter()
            .filter(|config| config.enabled)
            .map(|config| config.device_info())
            .collect())
    }

    async fn connected_device_info(&self, id: &DeviceId) -> Result<Option<DeviceInfo>> {
        let store = self.store.read().await;
        Ok(store
            .get(*id)
            .filter(|config| config.enabled)
            .map(|config| config.device_info()))
    }

    async fn connect(&mut self, id: &DeviceId) -> Result<()> {
        let store = self.store.read().await;
        let Some(config) = store.get(*id) else {
            bail!("simulated display {id} is not configured");
        };
        if !config.enabled {
            bail!("simulated display {id} is disabled");
        }
        self.connected.insert(*id);
        Ok(())
    }

    async fn disconnect(&mut self, id: &DeviceId) -> Result<()> {
        self.connected.remove(id);
        self.runtime.write().await.remove(*id);
        Ok(())
    }

    async fn write_colors(&mut self, id: &DeviceId, colors: &[[u8; 3]]) -> Result<()> {
        if colors.is_empty() {
            return Ok(());
        }

        bail!("simulated display {id} does not accept LED color writes");
    }

    async fn write_display_frame(&mut self, id: &DeviceId, jpeg_data: &[u8]) -> Result<()> {
        self.store_display_frame(id, Arc::new(jpeg_data.to_vec()))
            .await
    }

    async fn write_display_frame_owned(
        &mut self,
        id: &DeviceId,
        jpeg_data: Arc<Vec<u8>>,
    ) -> Result<()> {
        self.store_display_frame(id, jpeg_data).await
    }

    fn target_fps(&self, _id: &DeviceId) -> Option<u32> {
        Some(DEFAULT_SIMULATED_DISPLAY_FPS)
    }
}

pub async fn activate_simulated_displays(
    runtime: &DiscoveryRuntime,
    store: &Arc<RwLock<SimulatedDisplayStore>>,
) -> Result<Vec<DeviceId>> {
    let configs = {
        let store = store.read().await;
        store.list()
    };

    let mut activated = Vec::with_capacity(configs.len());
    for config in configs {
        let info = config.device_info();
        let fingerprint = config.fingerprint();
        let mut metadata = HashMap::new();
        metadata.insert(
            "backend_id".to_owned(),
            SIMULATED_DISPLAY_BACKEND_ID.to_owned(),
        );
        metadata.insert("simulator".to_owned(), "true".to_owned());

        let device_id = runtime
            .device_registry
            .add_with_fingerprint_and_metadata(info, fingerprint.clone(), metadata)
            .await;
        let persisted_settings = apply_persisted_device_settings(runtime, device_id).await;
        let Some(tracked) = runtime.device_registry.get(&device_id).await else {
            continue;
        };

        let actions = {
            let mut lifecycle = runtime.lifecycle_manager.lock().await;
            let mut actions = lifecycle.on_discovered_with_behavior(
                device_id,
                &tracked.info,
                SIMULATED_DISPLAY_BACKEND_ID,
                Some(&fingerprint),
                DiscoveryConnectBehavior::AutoConnect,
            );
            if config.enabled && persisted_settings.enabled {
                if let Ok(enable_actions) = lifecycle.on_user_enable(device_id) {
                    actions.extend(enable_actions);
                }
            } else if let Ok(disable_actions) = lifecycle.on_user_disable(device_id) {
                actions = disable_actions;
            }
            actions
        };

        execute_lifecycle_actions(runtime.clone(), actions).await;
        sync_registry_state(runtime, device_id).await;
        activated.push(device_id);
    }

    Ok(activated)
}

#[must_use]
pub fn default_layout_device_id(config: &SimulatedDisplayConfig) -> String {
    format!("{SIMULATED_DISPLAY_BACKEND_ID}:{}", config.id)
}

#[must_use]
pub async fn logical_device_ids_for_simulator(
    logical_devices: &Arc<RwLock<HashMap<String, LogicalDevice>>>,
    simulator_id: DeviceId,
) -> Vec<String> {
    let store = logical_devices.read().await;
    let mut ids = store
        .values()
        .filter(|entry| entry.physical_device_id == simulator_id)
        .map(|entry| entry.id.clone())
        .collect::<Vec<_>>();
    ids.sort();
    ids
}

#[must_use]
pub async fn register_backend_for_tests(
    backend_manager: &Arc<Mutex<hypercolor_core::device::BackendManager>>,
    store: Arc<RwLock<SimulatedDisplayStore>>,
    runtime: Arc<RwLock<SimulatedDisplayRuntime>>,
) -> bool {
    let mut manager = backend_manager.lock().await;
    manager.register_backend(Box::new(SimulatedDisplayBackend::new(store, runtime)));
    true
}
