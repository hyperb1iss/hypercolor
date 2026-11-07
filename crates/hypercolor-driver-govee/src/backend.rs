use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, PoisonError, RwLock as StdRwLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use hypercolor_driver_api::{
    BackendInfo, DeviceBackend, DeviceFrameSink, DeviceWriteOutcome, DiscoveredDevice, OutputPolicy,
};
use hypercolor_driver_support::{
    CredentialStore, DeviceSession, FrameDelivery, Session, SessionTable,
};
use hypercolor_types::device::{DeviceError, DeviceId, DeviceInfo};
use tokio::net::UdpSocket;

use crate::GoveeConfig;
use tokio::sync::Mutex;

use crate::capabilities::{GoveeCapabilities, SkuProfile, fallback_profile, profile_for_sku};
use crate::cloud::{CloudClient, V1Command};
use crate::lan::protocol::{DEVICE_PORT, LanCommand, encode_command};
use crate::lan::razer::{encode_razer_frame_base64, encode_razer_mode_base64};

/// Shortest spacing between color commands to a cloud-only device, which
/// keeps it inside the v1 control API's per-device rate limit.
const CLOUD_FRAME_INTERVAL: Duration = Duration::from_secs(6);

/// Budget for leaving Razer mode and powering off, which may take one
/// cloud request.
const CLEANUP_BUDGET: Duration = Duration::from_secs(5);

pub struct GoveeBackend {
    config: GoveeConfig,
    adopted: StdRwLock<HashMap<DeviceId, GoveeRoute>>,
    sessions: SessionTable<GoveeSession>,
    link: GoveeLink,
}

type SharedLanSocket = Arc<Mutex<Option<Arc<UdpSocket>>>>;

/// Where one discovered device is reachable.
#[derive(Clone)]
struct GoveeRoute {
    info: DeviceInfo,
    profile: SkuProfile,
    address: Option<SocketAddr>,
    cloud_id: Option<String>,
}

impl GoveeRoute {
    fn streams_razer(&self) -> bool {
        self.profile
            .capabilities
            .contains(GoveeCapabilities::RAZER_STREAMING)
    }

    fn frame_interval(&self, config: &GoveeConfig) -> Duration {
        if self.address.is_none() {
            return CLOUD_FRAME_INTERVAL;
        }
        let fps = if self.streams_razer() {
            config.razer_fps
        } else {
            config.lan_state_fps
        }
        .max(1);
        Duration::from_millis(1000 / u64::from(fps))
    }

    fn output_policy(&self, config: &GoveeConfig) -> OutputPolicy {
        if self.address.is_none() {
            return OutputPolicy::from_min_interval(CLOUD_FRAME_INTERVAL);
        }
        OutputPolicy::from_fps(if self.streams_razer() {
            config.razer_fps
        } else {
            config.lan_state_fps
        })
    }
}

/// The two ways out of this backend. One LAN socket serves every device and
/// lives as long as the backend, so no session owns it.
#[derive(Clone, Default)]
struct GoveeLink {
    shared_socket: SharedLanSocket,
    credential_store: Option<Arc<CredentialStore>>,
    cloud_base_url: Option<String>,
    cloud_client: Option<CloudClient>,
}

impl GoveeLink {
    async fn socket(&self) -> Result<Arc<UdpSocket>> {
        let mut shared_socket = self.shared_socket.lock().await;
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

    async fn send_lan(&self, address: SocketAddr, command: LanCommand) -> Result<()> {
        let payload = encode_command(&command)?;
        let socket = self.socket().await?;
        socket
            .send_to(&payload, address)
            .await
            .with_context(|| format!("failed to send Govee LAN command to {address}"))?;
        Ok(())
    }

    async fn cloud_client(&self) -> Result<Option<CloudClient>> {
        if let Some(client) = &self.cloud_client {
            return Ok(Some(client.clone()));
        }

        let Some(store) = &self.credential_store else {
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

        match &self.cloud_base_url {
            Some(base_url) => CloudClient::with_base_url(api_key, base_url).map(Some),
            None => CloudClient::new(api_key).map(Some),
        }
    }

    async fn send_cloud(&self, route: &GoveeRoute, command: V1Command) -> Result<()> {
        let cloud_id = route
            .cloud_id
            .as_deref()
            .with_context(|| format!("Govee device {} is not cloud-backed", route.info.id))?;
        let model = route.info.model.clone().unwrap_or_default();
        let client = self
            .cloud_client()
            .await?
            .context("Govee cloud credentials are not configured")?;
        client.v1_control(&model, cloud_id, command).await
    }
}

/// One connected device.
struct GoveeSession {
    device_id: DeviceId,
    route: GoveeRoute,
    link: GoveeLink,
    config: GoveeConfig,
    last_sent: Option<Vec<[u8; 3]>>,
    last_write_at: Option<Instant>,
    razer_enabled: bool,
}

impl GoveeSession {
    async fn set_brightness(&self, brightness: u8) -> Result<()> {
        match self.route.address {
            None => {
                self.link
                    .send_cloud(&self.route, V1Command::Brightness(brightness))
                    .await
            }
            Some(address) => {
                self.link
                    .send_lan(
                        address,
                        LanCommand::Brightness {
                            value: brightness.clamp(1, 100),
                        },
                    )
                    .await
            }
        }
    }

    async fn send_color(
        &self,
        command: LanCommand,
        delivery: Option<&FrameDelivery>,
    ) -> Result<()> {
        let started = || {
            if let Some(delivery) = delivery {
                delivery.transport_started();
            }
        };
        match command {
            LanCommand::Razer { pt } => {
                let address = self.route.address.with_context(|| {
                    format!("Govee device {} has no LAN address", self.device_id)
                })?;
                started();
                self.link.send_lan(address, LanCommand::Razer { pt }).await
            }
            LanCommand::ColorWc { red, green, blue } => {
                if let Some(address) = self.route.address {
                    started();
                    self.link
                        .send_lan(address, LanCommand::ColorWc { red, green, blue })
                        .await
                } else {
                    if self.route.cloud_id.is_none() {
                        bail!("Govee device {} is not cloud-backed", self.device_id);
                    }
                    started();
                    self.link
                        .send_cloud(
                            &self.route,
                            V1Command::Color {
                                r: red,
                                g: green,
                                b: blue,
                            },
                        )
                        .await
                }
            }
            _ => unreachable!("Govee write_colors only emits color frame commands"),
        }
    }

    async fn write_colors(
        &mut self,
        colors: &[[u8; 3]],
        delivery: Option<&FrameDelivery>,
    ) -> Result<DeviceWriteOutcome> {
        if colors.is_empty() {
            return Ok(DeviceWriteOutcome::SuppressedDuplicate);
        }

        let (command, sent_frame) = if self.route.streams_razer()
            && self
                .route
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

        if self.last_sent.as_deref() == Some(sent_frame.as_slice()) {
            return Ok(DeviceWriteOutcome::SuppressedDuplicate);
        }
        if self.last_write_at.is_some_and(|last_write| {
            last_write.elapsed() < self.route.frame_interval(&self.config)
        }) {
            return Ok(DeviceWriteOutcome::SuppressedCadence);
        }

        self.send_color(command, delivery).await?;

        self.last_sent = Some(sent_frame);
        self.last_write_at = Some(Instant::now());
        Ok(DeviceWriteOutcome::Sent)
    }

    async fn shut_down(&self) -> Result<()> {
        if self.razer_enabled
            && let Some(address) = self.route.address
        {
            self.link
                .send_lan(
                    address,
                    LanCommand::Razer {
                        pt: encode_razer_mode_base64(false),
                    },
                )
                .await?;
        }
        if self.config.power_off_on_disconnect {
            match self.route.address {
                Some(address) => {
                    self.link
                        .send_lan(address, LanCommand::Turn { on: false })
                        .await?;
                }
                None => {
                    self.link
                        .send_cloud(&self.route, V1Command::Turn(false))
                        .await?;
                }
            }
        }
        Ok(())
    }
}

impl DeviceSession for GoveeSession {
    type Facts = ();

    const CLEANUP: Duration = CLEANUP_BUDGET;

    async fn write_frame(
        &mut self,
        colors: &[[u8; 3]],
        delivery: Option<&FrameDelivery>,
    ) -> std::result::Result<DeviceWriteOutcome, DeviceError> {
        self.write_colors(colors, delivery)
            .await
            .map_err(|error| DeviceError::write(self.device_id, error))
    }

    async fn finish(&mut self) -> std::result::Result<(), DeviceError> {
        self.shut_down()
            .await
            .map_err(|error| DeviceError::write(self.device_id, error))
    }
}

impl GoveeBackend {
    #[must_use]
    pub fn new(config: GoveeConfig) -> Self {
        Self {
            config,
            adopted: StdRwLock::new(HashMap::new()),
            sessions: SessionTable::new(),
            link: GoveeLink::default(),
        }
    }

    #[must_use]
    pub fn with_credential_store(mut self, credential_store: Arc<CredentialStore>) -> Self {
        self.link.credential_store = Some(credential_store);
        self
    }

    #[must_use]
    pub fn with_cloud_client(mut self, cloud_client: CloudClient) -> Self {
        self.link.cloud_client = Some(cloud_client);
        self
    }

    #[must_use]
    pub fn with_cloud_base_url(mut self, cloud_base_url: impl Into<String>) -> Self {
        self.link.cloud_base_url = Some(cloud_base_url.into());
        self
    }

    fn route(&self, id: &DeviceId) -> Option<GoveeRoute> {
        self.adopted
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
            .cloned()
    }

    fn opener(&self) -> GoveeOpener {
        GoveeOpener {
            config: self.config.clone(),
            link: self.link.clone(),
        }
    }
}

/// What an open takes from the backend, since it runs on the session table's
/// task and outlives the connect that started it.
struct GoveeOpener {
    config: GoveeConfig,
    link: GoveeLink,
}

impl GoveeOpener {
    /// Wake the device and open its stream.
    async fn open_session(&self, id: DeviceId, route: GoveeRoute) -> Result<Session<GoveeSession>> {
        let mut razer_enabled = false;
        if let Some(address) = route.address {
            self.link
                .send_lan(address, LanCommand::Turn { on: true })
                .await?;
            if route.streams_razer() && route.profile.razer_led_count.is_some() {
                self.link
                    .send_lan(
                        address,
                        LanCommand::Razer {
                            pt: encode_razer_mode_base64(true),
                        },
                    )
                    .await?;
                razer_enabled = true;
            }
        } else {
            self.link.send_cloud(&route, V1Command::Turn(true)).await?;
        }
        Ok(Session::new(
            id,
            (),
            GoveeSession {
                device_id: id,
                route,
                link: self.link.clone(),
                config: self.config.clone(),
                last_sent: None,
                last_write_at: None,
                razer_enabled,
            },
        ))
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
        let id = discovered.info.id;
        let route = {
            let mut adopted = self.adopted.write().unwrap_or_else(PoisonError::into_inner);
            let route = adopted.entry(id).or_insert_with(|| GoveeRoute {
                info: discovered.info.clone(),
                profile: profile.clone(),
                address,
                cloud_id: cloud_id.clone(),
            });
            route.info.clone_from(&discovered.info);
            route.profile = profile;
            if address.is_some() {
                route.address = address;
            }
            if cloud_id.is_some() {
                route.cloud_id = cloud_id;
            }
            route.clone()
        };
        // A rescan can move a device under its live session.
        if let Some(sink) = self.sessions.sink(&id)
            && let Some(mut session) = sink.try_admit()
        {
            session.route = route;
        }
        Ok(())
    }

    async fn connected_device_info(
        &self,
        id: &DeviceId,
    ) -> std::result::Result<Option<DeviceInfo>, DeviceError> {
        Ok(self.route(id).map(|route| route.info))
    }

    fn supports_temporary_direct_control(&self, _info: &DeviceInfo) -> bool {
        true
    }

    async fn connect(&self, id: &DeviceId) -> std::result::Result<(), DeviceError> {
        let id = *id;
        self.sessions
            .open(&id, || {
                let route = self.route(&id);
                let opener = self.opener();
                async move {
                    let route = route.ok_or(DeviceError::NotAdopted { device_id: id })?;
                    opener
                        .open_session(id, route)
                        .await
                        .map_err(|error| DeviceError::connection(id, error))
                }
            })
            .await
    }

    async fn disconnect(&self, id: &DeviceId) -> std::result::Result<(), DeviceError> {
        self.sessions.close(id).await
    }

    async fn retire(&self) -> std::result::Result<(), DeviceError> {
        self.sessions.retire().await
    }

    async fn write_colors(
        &self,
        id: &DeviceId,
        colors: &[[u8; 3]],
    ) -> std::result::Result<(), DeviceError> {
        self.sessions.require(id)?.write(colors).await.map(|_| ())
    }

    async fn set_brightness(
        &self,
        id: &DeviceId,
        brightness: u8,
    ) -> std::result::Result<(), DeviceError> {
        self.sessions
            .require(id)?
            .transact(async |session| session.set_brightness(brightness).await)
            .await?
            .map_err(|error| DeviceError::write(id, error))
    }

    fn output_policy(&self, id: &DeviceId) -> Option<OutputPolicy> {
        self.route(id)
            .map(|route| route.output_policy(&self.config))
    }

    fn frame_sink(&self, id: &DeviceId) -> Option<Arc<dyn DeviceFrameSink>> {
        self.sessions.frame_sink(id)
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
