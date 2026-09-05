//! Virtual display simulator persistence and daemon-local backend wiring.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex, PoisonError};
use std::time::SystemTime;

use anyhow::{Context, Result, bail};
use hypercolor_core::device::DeviceLifecycleManager;
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, RwLock};

use hypercolor_driver_api::{
    BackendInfo, DeviceBackend, DiscoveredDevice, DiscoveryConnectBehavior,
};
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceColorSpace, DeviceError,
    DeviceFamily, DeviceFeatures, DeviceFingerprint, DeviceId, DeviceInfo, DeviceOrigin,
    DisplayFrameFormat, FingerprintNamespace, OwnedDisplayFramePayload, SegmentInfo,
};

use crate::discovery::{
    DiscoveryRuntime, apply_persisted_device_settings, execute_lifecycle_actions,
    sync_registry_state,
};
use crate::logical_devices::LogicalDevice;
use crate::persistence::write_atomic;

pub const SIMULATED_DISPLAY_BACKEND_ID: &str = "simulator";
const SIMULATED_DISPLAY_FAMILY: &str = "simulator";
const DEFAULT_SIMULATED_DISPLAY_FPS: u32 = 15;

pub use hypercolor_types::api::simulators::SimulatedDisplay as SimulatedDisplayConfig;

/// Daemon-side behavior of a simulated display resource.
pub trait SimulatedDisplayExt: Sized {
    /// Clamp the geometry and fill an empty name.
    #[must_use]
    fn normalized(self) -> Self;
    /// The device the simulator presents to the registry.
    #[must_use]
    fn device_info(&self) -> DeviceInfo;
    /// The stable identity a simulator keeps across restarts.
    #[must_use]
    fn fingerprint(&self) -> DeviceFingerprint;
}

impl SimulatedDisplayExt for SimulatedDisplayConfig {
    fn normalized(mut self) -> Self {
        self.name = self.name.trim().to_owned();
        if self.name.is_empty() {
            self.name = format!("Simulated Display {}", self.id);
        }
        self.width = self.width.max(1);
        self.height = self.height.max(1);
        self
    }

    fn device_info(&self) -> DeviceInfo {
        DeviceInfo {
            id: self.id,
            name: self.name.clone(),
            vendor: "Hypercolor".to_owned(),
            family: DeviceFamily::named(SIMULATED_DISPLAY_FAMILY.to_owned()),
            model: Some("virtual_display".to_owned()),
            connection_type: ConnectionType::Bridge,
            origin: DeviceOrigin::native(
                SIMULATED_DISPLAY_FAMILY,
                SIMULATED_DISPLAY_BACKEND_ID,
                ConnectionType::Bridge,
            ),
            segments: vec![SegmentInfo {
                name: "Display".to_owned(),
                led_count: 0,
                topology: hypercolor_types::device::DeviceTopologyHint::Display {
                    width: self.width,
                    height: self.height,
                    circular: self.circular,
                    format: DisplayFrameFormat::Jpeg,
                },
                color_format: DeviceColorFormat::Rgb,
                layout_hint: None,
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

    fn fingerprint(&self) -> DeviceFingerprint {
        DeviceFingerprint::mint(
            FingerprintNamespace::Bridge,
            SIMULATED_DISPLAY_BACKEND_ID,
            &self.id.to_string(),
        )
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
        write_atomic(&self.path, payload.as_bytes())
            .context("failed to persist simulated displays")?;
        Ok(())
    }
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
    connected: StdMutex<HashSet<DeviceId>>,
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
            connected: StdMutex::new(HashSet::new()),
        }
    }

    async fn store_display_frame(&self, id: &DeviceId, jpeg_data: Arc<Vec<u8>>) -> Result<()> {
        if !self
            .connected
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(id)
        {
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
            description: "Daemon-local virtual LCD devices for layout and display-face workflows"
                .to_owned(),
        }
    }

    fn adopt_device(&self, discovered: &DiscoveredDevice) -> Result<(), DeviceError> {
        if discovered.info.output_backend_id() == SIMULATED_DISPLAY_BACKEND_ID {
            Ok(())
        } else {
            Err(DeviceError::NotAdopted {
                device_id: discovered.info.id,
            })
        }
    }

    async fn connected_device_info(
        &self,
        id: &DeviceId,
    ) -> Result<Option<DeviceInfo>, DeviceError> {
        let store = self.store.read().await;
        Ok(store
            .get(*id)
            .filter(|config| config.enabled)
            .map(|config| config.device_info()))
    }

    async fn connect(&self, id: &DeviceId) -> Result<(), DeviceError> {
        let store = self.store.read().await;
        let Some(config) = store.get(*id) else {
            return Err(DeviceError::NotAdopted { device_id: *id });
        };
        if !config.enabled {
            return Err(DeviceError::connection(id, "simulated display is disabled"));
        }
        self.connected
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(*id);
        Ok(())
    }

    async fn disconnect(&self, id: &DeviceId) -> Result<(), DeviceError> {
        self.connected
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(id);
        self.runtime.write().await.remove(*id);
        Ok(())
    }

    async fn write_colors(&self, _id: &DeviceId, colors: &[[u8; 3]]) -> Result<(), DeviceError> {
        if colors.is_empty() {
            return Ok(());
        }

        Err(DeviceError::Unsupported {
            backend: SIMULATED_DISPLAY_BACKEND_ID.to_owned(),
            operation: "LED color output",
        })
    }

    async fn write_display_payload_owned(
        &self,
        id: &DeviceId,
        payload: Arc<OwnedDisplayFramePayload>,
    ) -> Result<(), DeviceError> {
        // The simulator keeps the last frame as the JPEG it serves as a
        // preview image, so it takes what the daemon's JPEG route sends.
        if payload.format != DisplayFrameFormat::Jpeg {
            return Err(DeviceError::Unsupported {
                backend: SIMULATED_DISPLAY_BACKEND_ID.to_owned(),
                operation: "RGB display output",
            });
        }
        self.store_display_frame(id, Arc::clone(&payload.data))
            .await
            .map_err(|error| DeviceError::write(id, error))
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
    DeviceLifecycleManager::canonical_layout_device_id(
        &config.device_info(),
        Some(&config.fingerprint()),
    )
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
    manager.register_backend(Arc::new(SimulatedDisplayBackend::new(store, runtime)));
    true
}
