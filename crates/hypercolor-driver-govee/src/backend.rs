use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, PoisonError, RwLock as StdRwLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use async_trait::async_trait;
use hypercolor_driver_api::{
    BackendInfo, DeviceBackend, DeviceDeliveryAck, DeviceDeliveryId, DeviceDeliveryObserver,
    DeviceFrameSink, DeviceWriteOutcome, DiscoveredDevice, OutputCadence,
};
use hypercolor_driver_support::CredentialStore;
use hypercolor_types::device::{DeviceError, DeviceId, DeviceInfo};
use tokio::net::UdpSocket;

use crate::GoveeConfig;
use tokio::sync::Mutex;

use crate::capabilities::{GoveeCapabilities, SkuProfile, fallback_profile, profile_for_sku};
use crate::cloud::{CloudClient, V1Command};
use crate::lan::protocol::{DEVICE_PORT, LanCommand, encode_command};
use crate::lan::razer::{encode_razer_frame_base64, encode_razer_mode_base64};

pub struct GoveeBackend {
    config: GoveeConfig,
    devices: StdRwLock<HashMap<DeviceId, Arc<Mutex<GoveeDeviceState>>>>,
    shared_socket: SharedLanSocket,
    credential_store: Option<Arc<CredentialStore>>,
    cloud_base_url: Option<String>,
    cloud_client: Option<CloudClient>,
}

type SharedLanSocket = Arc<Mutex<Option<Arc<UdpSocket>>>>;

#[derive(Clone)]
struct GoveeDeviceState {
    info: DeviceInfo,
    profile: SkuProfile,
    address: Option<SocketAddr>,
    cloud_id: Option<String>,
    last_sent: Option<Vec<[u8; 3]>>,
    last_write_at: Option<Instant>,
    razer_enabled: bool,
}

impl GoveeBackend {
    #[must_use]
    pub fn new(config: GoveeConfig) -> Self {
        Self {
            config,
            devices: StdRwLock::new(HashMap::new()),
            shared_socket: Arc::new(Mutex::new(None)),
            credential_store: None,
            cloud_base_url: None,
            cloud_client: None,
        }
    }

    #[must_use]
    pub fn with_credential_store(mut self, credential_store: Arc<CredentialStore>) -> Self {
        self.credential_store = Some(credential_store);
        self
    }

    #[must_use]
    pub fn with_cloud_client(mut self, cloud_client: CloudClient) -> Self {
        self.cloud_client = Some(cloud_client);
        self
    }

    #[must_use]
    pub fn with_cloud_base_url(mut self, cloud_base_url: impl Into<String>) -> Self {
        self.cloud_base_url = Some(cloud_base_url.into());
        self
    }

    async fn ensure_socket(shared_socket: &SharedLanSocket) -> Result<Arc<UdpSocket>> {
        let mut shared_socket = shared_socket.lock().await;
        if let Some(socket) = shared_socket.as_ref() {
            return Ok(Arc::clone(socket));
        }

        let socket = Arc::new(
            UdpSocket::bind(("0.0.0.0", 0))
                .await
                .context("failed to bind Govee LAN command socket")?,
        );
        *shared_socket = Some(Arc::clone(&socket));
        Ok(socket)
    }

    async fn send_lan_command(
        shared_socket: &SharedLanSocket,
        address: SocketAddr,
        command: LanCommand,
    ) -> Result<()> {
        let payload = encode_command(&command)?;
        let socket = Self::ensure_socket(shared_socket).await?;
        socket
            .send_to(&payload, address)
            .await
            .with_context(|| format!("failed to send Govee LAN command to {address}"))?;
        Ok(())
    }

    async fn send_command(&self, id: &DeviceId, command: LanCommand) -> Result<()> {
        let device = self
            .devices
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
            .cloned()
            .with_context(|| format!("Govee device {id} is not known"))?;
        let device = device.lock().await;
        let address = device
            .address
            .with_context(|| format!("Govee device {id} has no LAN address"))?;
        drop(device);
        Self::send_lan_command(&self.shared_socket, address, command).await
    }

    fn frame_interval_for(config: &GoveeConfig, device: &GoveeDeviceState) -> Duration {
        if device.address.is_none() {
            return Duration::from_secs(6);
        }
        let fps = if device
            .profile
            .capabilities
            .contains(GoveeCapabilities::RAZER_STREAMING)
        {
            config.razer_fps
        } else {
            config.lan_state_fps
        }
        .max(1);
        Duration::from_millis(1000 / u64::from(fps))
    }

    async fn cloud_client_from(
        credential_store: Option<&Arc<CredentialStore>>,
        cloud_client: Option<&CloudClient>,
        cloud_base_url: Option<&str>,
    ) -> Result<Option<CloudClient>> {
        if let Some(client) = cloud_client {
            return Ok(Some(client.clone()));
        }

        let Some(store) = credential_store else {
            return Ok(None);
        };
        let Some(api_key) = store
            .get_driver_json("govee", "account")
            .await
            .and_then(|value| {
                value
                    .get("api_key")
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .map(ToOwned::to_owned)
            })
            .filter(|value| !value.is_empty())
        else {
            return Ok(None);
        };

        match cloud_base_url {
            Some(base_url) => CloudClient::with_base_url(api_key, base_url).map(Some),
            None => CloudClient::new(api_key).map(Some),
        }
    }

    async fn send_cloud_command_to(
        credential_store: Option<&Arc<CredentialStore>>,
        cloud_client: Option<&CloudClient>,
        cloud_base_url: Option<&str>,
        model: &str,
        cloud_id: &str,
        command: V1Command,
    ) -> Result<()> {
        let client = Self::cloud_client_from(credential_store, cloud_client, cloud_base_url)
            .await?
            .context("Govee cloud credentials are not configured")?;
        client.v1_control(model, cloud_id, command).await
    }

    async fn send_cloud_command(&self, id: &DeviceId, command: V1Command) -> Result<()> {
        let device = self
            .devices
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
            .cloned()
            .with_context(|| format!("Govee device {id} is not known"))?;
        let device = device.lock().await;
        let cloud_id = device
            .cloud_id
            .clone()
            .with_context(|| format!("Govee device {id} is not cloud-backed"))?;
        let model = device.info.model.clone().unwrap_or_default();
        drop(device);
        Self::send_cloud_command_to(
            self.credential_store.as_ref(),
            self.cloud_client.as_ref(),
            self.cloud_base_url.as_deref(),
            &model,
            &cloud_id,
            command,
        )
        .await
    }

    async fn write_device_colors(
        id: &DeviceId,
        device: &Arc<Mutex<GoveeDeviceState>>,
        colors: &[[u8; 3]],
        config: &GoveeConfig,
        shared_socket: &SharedLanSocket,
        credential_store: Option<&Arc<CredentialStore>>,
        cloud_client: Option<&CloudClient>,
        cloud_base_url: Option<&str>,
        delivery: Option<(DeviceDeliveryId, &dyn DeviceDeliveryObserver)>,
    ) -> Result<DeviceWriteOutcome> {
        if colors.is_empty() {
            return Ok(DeviceWriteOutcome::SuppressedDuplicate);
        }

        let mut device = device.lock().await;
        let (command, sent_frame) = if device
            .profile
            .capabilities
            .contains(GoveeCapabilities::RAZER_STREAMING)
            && device
                .profile
                .razer_led_count
                .is_some_and(|count| usize::from(count) == colors.len())
            && let Some(pt) = encode_razer_frame_base64(colors)
        {
            (LanCommand::Razer { pt }, colors.to_vec())
        } else {
            let [red, green, blue] = mean_color(colors);
            (
                LanCommand::ColorWc { red, green, blue },
                vec![[red, green, blue]],
            )
        };

        if device.last_sent.as_deref() == Some(sent_frame.as_slice()) {
            return Ok(DeviceWriteOutcome::SuppressedDuplicate);
        }
        if device.last_write_at.is_some_and(|last_write| {
            last_write.elapsed() < Self::frame_interval_for(config, &device)
        }) {
            return Ok(DeviceWriteOutcome::SuppressedCadence);
        }

        match command {
            LanCommand::Razer { pt } => {
                let address = device
                    .address
                    .with_context(|| format!("Govee device {id} has no LAN address"))?;
                if let Some((id, observer)) = delivery {
                    observer.transport_started(id);
                }
                Self::send_lan_command(shared_socket, address, LanCommand::Razer { pt }).await?;
            }
            LanCommand::ColorWc { red, green, blue } => {
                if let Some(address) = device.address {
                    if let Some((id, observer)) = delivery {
                        observer.transport_started(id);
                    }
                    Self::send_lan_command(
                        shared_socket,
                        address,
                        LanCommand::ColorWc { red, green, blue },
                    )
                    .await?;
                } else {
                    let cloud_id = device
                        .cloud_id
                        .clone()
                        .with_context(|| format!("Govee device {id} is not cloud-backed"))?;
                    let model = device.info.model.clone().unwrap_or_default();
                    if let Some((id, observer)) = delivery {
                        observer.transport_started(id);
                    }
                    Self::send_cloud_command_to(
                        credential_store,
                        cloud_client,
                        cloud_base_url,
                        &model,
                        &cloud_id,
                        V1Command::Color {
                            r: red,
                            g: green,
                            b: blue,
                        },
                    )
                    .await?;
                }
            }
            _ => unreachable!("Govee write_colors only emits color frame commands"),
        }

        device.last_sent = Some(sent_frame);
        device.last_write_at = Some(Instant::now());
        Ok(DeviceWriteOutcome::Sent)
    }
}

struct GoveeFrameSink {
    device_id: DeviceId,
    device: Arc<Mutex<GoveeDeviceState>>,
    config: GoveeConfig,
    shared_socket: SharedLanSocket,
    credential_store: Option<Arc<CredentialStore>>,
    cloud_client: Option<CloudClient>,
    cloud_base_url: Option<String>,
}

#[async_trait]
impl DeviceFrameSink for GoveeFrameSink {
    async fn write_colors_shared(
        &self,
        colors: Arc<Vec<[u8; 3]>>,
    ) -> std::result::Result<(), DeviceError> {
        self.write_colors_shared_outcome(colors).await.map(|_| ())
    }

    async fn write_colors_shared_outcome(
        &self,
        colors: Arc<Vec<[u8; 3]>>,
    ) -> std::result::Result<DeviceWriteOutcome, DeviceError> {
        GoveeBackend::write_device_colors(
            &self.device_id,
            &self.device,
            colors.as_slice(),
            &self.config,
            &self.shared_socket,
            self.credential_store.as_ref(),
            self.cloud_client.as_ref(),
            self.cloud_base_url.as_deref(),
            None,
        )
        .await
        .map_err(|error| DeviceError::write(self.device_id, error))
    }

    async fn deliver_colors_shared_observed(
        &self,
        id: DeviceDeliveryId,
        colors: Arc<Vec<[u8; 3]>>,
        observer: Arc<dyn DeviceDeliveryObserver>,
    ) -> DeviceDeliveryAck {
        let payload_bytes = colors.len().saturating_mul(3);
        let started_at = Instant::now();
        let result = GoveeBackend::write_device_colors(
            &self.device_id,
            &self.device,
            colors.as_slice(),
            &self.config,
            &self.shared_socket,
            self.credential_store.as_ref(),
            self.cloud_client.as_ref(),
            self.cloud_base_url.as_deref(),
            Some((id, observer.as_ref())),
        )
        .await
        .map_err(|error| DeviceError::write(self.device_id, error));
        DeviceDeliveryAck::from_write_result(id, payload_bytes, started_at.elapsed(), result)
    }
}

#[async_trait]
impl DeviceBackend for GoveeBackend {
    fn info(&self) -> BackendInfo {
        BackendInfo {
            id: "govee".to_owned(),
            name: "Govee LAN".to_owned(),
            description: "Govee Wi-Fi lights over LAN UDP".to_owned(),
        }
    }

    fn adopt_device(&self, discovered: &DiscoveredDevice) -> Result<(), DeviceError> {
        let sku = discovered
            .metadata
            .get("sku")
            .or(discovered.info.model.as_ref())
            .cloned()
            .ok_or(DeviceError::NotAdopted {
                device_id: discovered.info.id,
            })?;
        let address = discovered
            .metadata
            .get("ip")
            .and_then(|value| value.parse::<IpAddr>().ok())
            .map(|ip| {
                let port = discovered
                    .metadata
                    .get("port")
                    .and_then(|value| value.parse::<u16>().ok())
                    .unwrap_or(DEVICE_PORT);
                SocketAddr::new(ip, port)
            });
        let cloud_id = discovered.metadata.get("cloud_device_id").cloned();
        if address.is_none() && cloud_id.is_none() {
            return Err(DeviceError::NotAdopted {
                device_id: discovered.info.id,
            });
        }
        let profile = profile_for_sku(&sku).unwrap_or_else(|| fallback_profile(&sku));
        self.devices
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(discovered.info.id)
            .and_modify(|state| {
                if let Ok(mut state) = state.try_lock() {
                    state.info.clone_from(&discovered.info);
                    state.profile.clone_from(&profile);
                    if address.is_some() {
                        state.address = address;
                    }
                    if cloud_id.is_some() {
                        state.cloud_id.clone_from(&cloud_id);
                    }
                }
            })
            .or_insert_with(|| {
                Arc::new(Mutex::new(GoveeDeviceState {
                    info: discovered.info.clone(),
                    profile,
                    address,
                    cloud_id,
                    last_sent: None,
                    last_write_at: None,
                    razer_enabled: false,
                }))
            });
        Ok(())
    }

    async fn connected_device_info(
        &self,
        id: &DeviceId,
    ) -> std::result::Result<Option<DeviceInfo>, DeviceError> {
        let device = self
            .devices
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
            .cloned();
        let Some(device) = device else {
            return Ok(None);
        };
        Ok(Some(device.lock().await.info.clone()))
    }

    fn supports_temporary_direct_control(&self, _info: &DeviceInfo) -> bool {
        true
    }

    async fn connect(&self, id: &DeviceId) -> std::result::Result<(), DeviceError> {
        let device = self
            .devices
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
            .cloned();
        let Some(device) = device else {
            return Err(DeviceError::NotAdopted { device_id: *id });
        };

        if device
            .try_lock()
            .is_ok_and(|device| device.address.is_some())
        {
            self.send_command(id, LanCommand::Turn { on: true })
                .await
                .map_err(|error| DeviceError::connection(id, error))?;
        } else {
            self.send_cloud_command(id, V1Command::Turn(true))
                .await
                .map_err(|error| DeviceError::connection(id, error))?;
            return Ok(());
        }
        let should_enable_razer = device.try_lock().is_ok_and(|device| {
            device
                .profile
                .capabilities
                .contains(GoveeCapabilities::RAZER_STREAMING)
                && device.profile.razer_led_count.is_some()
        });
        if should_enable_razer {
            self.send_command(
                id,
                LanCommand::Razer {
                    pt: encode_razer_mode_base64(true),
                },
            )
            .await
            .map_err(|error| DeviceError::connection(id, error))?;
            device.lock().await.razer_enabled = true;
        }

        Ok(())
    }

    async fn disconnect(&self, id: &DeviceId) -> std::result::Result<(), DeviceError> {
        let device = self
            .devices
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
            .cloned();
        let Some(device) = device else {
            return Ok(());
        };
        let (razer_enabled, has_lan_address) = {
            let state = device.lock().await;
            (state.razer_enabled, state.address.is_some())
        };
        if razer_enabled {
            self.send_command(
                id,
                LanCommand::Razer {
                    pt: encode_razer_mode_base64(false),
                },
            )
            .await
            .map_err(|error| DeviceError::write(id, error))?;
        }
        if self.config.power_off_on_disconnect {
            if has_lan_address {
                self.send_command(id, LanCommand::Turn { on: false })
                    .await
                    .map_err(|error| DeviceError::write(id, error))?;
            } else {
                self.send_cloud_command(id, V1Command::Turn(false))
                    .await
                    .map_err(|error| DeviceError::write(id, error))?;
            }
        }
        let mut device = device.lock().await;
        device.razer_enabled = false;
        device.last_sent = None;
        Ok(())
    }

    async fn write_colors(
        &self,
        id: &DeviceId,
        colors: &[[u8; 3]],
    ) -> std::result::Result<(), DeviceError> {
        let device = self
            .devices
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
            .cloned();
        let Some(device) = device else {
            return Err(DeviceError::Disconnected {
                device: id.to_string(),
            });
        };
        Self::write_device_colors(
            id,
            &device,
            colors,
            &self.config,
            &self.shared_socket,
            self.credential_store.as_ref(),
            self.cloud_client.as_ref(),
            self.cloud_base_url.as_deref(),
            None,
        )
        .await
        .map_err(|error| DeviceError::write(id, error))
        .map(|_| ())
    }

    async fn write_colors_shared_outcome(
        &self,
        id: &DeviceId,
        colors: Arc<Vec<[u8; 3]>>,
    ) -> std::result::Result<DeviceWriteOutcome, DeviceError> {
        let device = self
            .devices
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
            .cloned();
        let Some(device) = device else {
            return Err(DeviceError::Disconnected {
                device: id.to_string(),
            });
        };
        Self::write_device_colors(
            id,
            &device,
            colors.as_slice(),
            &self.config,
            &self.shared_socket,
            self.credential_store.as_ref(),
            self.cloud_client.as_ref(),
            self.cloud_base_url.as_deref(),
            None,
        )
        .await
        .map_err(|error| DeviceError::write(id, error))
    }

    async fn deliver_colors_shared_observed(
        &self,
        device_id: &DeviceId,
        delivery_id: DeviceDeliveryId,
        colors: Arc<Vec<[u8; 3]>>,
        observer: Arc<dyn DeviceDeliveryObserver>,
    ) -> DeviceDeliveryAck {
        let payload_bytes = colors.len().saturating_mul(3);
        let started_at = Instant::now();
        let device = self
            .devices
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(device_id)
            .cloned();
        let Some(device) = device else {
            return DeviceDeliveryAck::rejected(
                delivery_id,
                DeviceError::Disconnected {
                    device: device_id.to_string(),
                },
            );
        };
        let result = Self::write_device_colors(
            device_id,
            &device,
            colors.as_slice(),
            &self.config,
            &self.shared_socket,
            self.credential_store.as_ref(),
            self.cloud_client.as_ref(),
            self.cloud_base_url.as_deref(),
            Some((delivery_id, observer.as_ref())),
        )
        .await
        .map_err(|error| DeviceError::write(device_id, error));
        DeviceDeliveryAck::from_write_result(
            delivery_id,
            payload_bytes,
            started_at.elapsed(),
            result,
        )
    }

    async fn set_brightness(
        &self,
        id: &DeviceId,
        brightness: u8,
    ) -> std::result::Result<(), DeviceError> {
        let device = self
            .devices
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
            .cloned();
        let Some(device) = device else {
            return Err(DeviceError::NotAdopted { device_id: *id });
        };
        if device
            .try_lock()
            .map_or(true, |device| device.address.is_none())
        {
            return self
                .send_cloud_command(id, V1Command::Brightness(brightness))
                .await
                .map_err(|error| DeviceError::write(id, error));
        }

        self.send_command(
            id,
            LanCommand::Brightness {
                value: brightness.clamp(1, 100),
            },
        )
        .await
        .map_err(|error| DeviceError::write(id, error))
    }

    fn output_cadence(&self, id: &DeviceId) -> Option<OutputCadence> {
        self.devices
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
            .and_then(|device| {
                let device = device.try_lock().ok()?;
                if device.address.is_none() {
                    return Some(OutputCadence::from_min_interval(Duration::from_secs(6), 0));
                }
                let target_fps = if device
                    .profile
                    .capabilities
                    .contains(GoveeCapabilities::RAZER_STREAMING)
                {
                    self.config.razer_fps
                } else {
                    self.config.lan_state_fps
                };
                Some(OutputCadence::from_fps(target_fps))
            })
    }

    fn frame_sink(&self, id: &DeviceId) -> Option<Arc<dyn DeviceFrameSink>> {
        self.devices
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
            .map(|device| {
                Arc::new(GoveeFrameSink {
                    device_id: *id,
                    device: Arc::clone(device),
                    config: self.config.clone(),
                    shared_socket: Arc::clone(&self.shared_socket),
                    credential_store: self.credential_store.clone(),
                    cloud_client: self.cloud_client.clone(),
                    cloud_base_url: self.cloud_base_url.clone(),
                }) as Arc<dyn DeviceFrameSink>
            })
    }
}

fn mean_color(colors: &[[u8; 3]]) -> [u8; 3] {
    let mut red = 0_u32;
    let mut green = 0_u32;
    let mut blue = 0_u32;
    for [r, g, b] in colors {
        red += u32::from(*r);
        green += u32::from(*g);
        blue += u32::from(*b);
    }
    let count = u32::try_from(colors.len()).unwrap_or(u32::MAX).max(1);
    [
        u8::try_from(red / count).unwrap_or(u8::MAX),
        u8::try_from(green / count).unwrap_or(u8::MAX),
        u8::try_from(blue / count).unwrap_or(u8::MAX),
    ]
}
