//! Hue entertainment backend.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, PoisonError, RwLock as StdRwLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use hypercolor_color::Rgb;
use hypercolor_driver_api::{
    BackendInfo, DeviceBackend, DeviceFrameSink, DeviceLifecyclePolicy, DeviceWriteOutcome,
    DiscoveredDevice, OutputPolicy,
};
use hypercolor_driver_support::{
    CredentialStore, DeviceSession, FrameDelivery, Session, SessionTable,
};
use hypercolor_types::device::{DeviceError, DeviceId, DeviceInfo};

use super::bridge::HueBridgeClient;
use super::color::{CieXyb, ColorGamut, rgb_to_cie_xyb};
use super::streaming::HueStreamSession;
use super::types::{
    HueBridgeIdentity, HueDiscoveredBridge, HueEntertainmentConfig, HueLight, build_device_info,
    choose_entertainment_config,
};

const SIZE_MISMATCH_WARN_INTERVAL: Duration = Duration::from_mins(1);
const HUE_MAX_FRAME_SILENCE: Duration = Duration::from_secs(1);

/// Deadline for a healthy bridge handshake, not for a single request.
///
/// `connect` issues four sequential authenticated HTTP calls (identity, lights,
/// entertainment configs, start streaming) before the DTLS handshake even
/// begins, and every one of those carries its own five second client timeout
/// with an https-then-http scheme fallback. The shared default of five seconds
/// is the budget for one of those calls, so it expires on any slow bridge and
/// pushes a healthy connect into the reconnect path.
///
/// This cannot cover the pathological case where every request burns its full
/// allowance, because connects run inline and the deadline therefore doubles as
/// the ceiling on how long one unresponsive bridge holds the discovery sweep
/// open. A bridge that sick should land in the reconnect path. Covering the
/// pathological case needs a tracked, cancellable background connect instead.
const HUE_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// Longest the DTLS goodbye may take, so a wedged one leaves the stop its
/// whole budget.
const HUE_DTLS_CLOSE_BOUND: Duration = Duration::from_secs(1);

/// Budget for closing the DTLS stream and stopping entertainment mode.
///
/// A bridge left streaming refuses the next client, so the stop must get
/// its one full HTTP client timeout even after a caller stopped waiting.
const HUE_CLEANUP_BUDGET: Duration = Duration::from_secs(6);

/// Philips Hue backend configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HueConfig {
    /// Preferred entertainment configuration name or ID.
    #[serde(default)]
    pub entertainment_config: Option<String>,

    /// Manual bridge IPs for networks where mDNS discovery is unavailable.
    #[serde(default)]
    pub bridge_ips: Vec<IpAddr>,

    /// Use CIE xy color conversion when streaming to Hue.
    #[serde(default = "bool_true")]
    pub use_cie_xy: bool,
}

impl Default for HueConfig {
    fn default() -> Self {
        Self {
            entertainment_config: None,
            bridge_ips: Vec::new(),
            use_cie_xy: true,
        }
    }
}

const fn bool_true() -> bool {
    true
}

/// Hue backend implementing `DeviceBackend`.
pub struct HueBackend {
    config: HueConfig,
    credential_store: Arc<CredentialStore>,
    discovered: Arc<StdRwLock<HashMap<DeviceId, HueDiscoveredBridge>>>,
    sessions: SessionTable<HueSession>,
}

/// One open entertainment stream.
struct HueSession {
    device_id: DeviceId,
    bridge_id: String,
    client: HueBridgeClient,
    stream: HueStreamSession,
    entertainment_config: HueEntertainmentConfig,
    channel_gamuts: Vec<ColorGamut>,
    info: DeviceInfo,
    brightness: u8,
    use_cie_xy: bool,
    last_size_mismatch_warn_at: Option<Instant>,
}

impl DeviceSession for HueSession {
    type Facts = ();

    const CLEANUP: Duration = HUE_CLEANUP_BUDGET;

    async fn write_frame(
        &mut self,
        colors: &[[u8; 3]],
        delivery: Option<&FrameDelivery>,
    ) -> Result<DeviceWriteOutcome, DeviceError> {
        let expected_led_count = usize::try_from(self.info.total_led_count()).unwrap_or(usize::MAX);
        if colors.len() != expected_led_count {
            let should_warn = self
                .last_size_mismatch_warn_at
                .is_none_or(|last_warn_at| last_warn_at.elapsed() >= SIZE_MISMATCH_WARN_INTERVAL);
            if should_warn {
                warn!(
                    device_id = %self.device_id,
                    expected_led_count,
                    actual_led_count = colors.len(),
                    "Hue frame size mismatch; collapsing or padding to match entertainment channels"
                );
                self.last_size_mismatch_warn_at = Some(Instant::now());
            }
        }

        let collapsed = collapse_channel_colors(
            self.entertainment_config.channels.as_slice(),
            colors,
            self.brightness,
        );
        let sent = if self.use_cie_xy {
            let cie_colors = collapsed
                .iter()
                .zip(self.channel_gamuts.iter().copied())
                .map(|(color, gamut)| rgb_to_cie_xyb(color[0], color[1], color[2], &gamut))
                .collect::<Vec<CieXyb>>();
            if let Some(delivery) = delivery {
                delivery.transport_started();
            }
            self.stream.send_frame(cie_colors.as_slice()).await
        } else {
            if let Some(delivery) = delivery {
                delivery.transport_started();
            }
            self.stream.send_rgb_frame(&collapsed).await
        };
        sent.map_err(|error| DeviceError::write(self.device_id, error))?;
        Ok(DeviceWriteOutcome::Sent)
    }

    /// Close the DTLS stream and take the bridge out of entertainment mode.
    ///
    /// A failed DTLS close only means the bridge already dropped the stream,
    /// so it never stands in the way of the stop that frees the bridge.
    async fn finish(&mut self) -> Result<(), DeviceError> {
        match tokio::time::timeout(HUE_DTLS_CLOSE_BOUND, self.stream.close()).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => debug!(device_id = %self.device_id, %error, "Hue DTLS close failed"),
            Err(_) => debug!(device_id = %self.device_id, "Hue DTLS close timed out"),
        }
        self.client
            .stop_streaming(&self.entertainment_config.id)
            .await
            .map_err(|error| DeviceError::write(self.device_id, error))
    }
}

impl HueBackend {
    /// Create a new Hue backend using the configured manual bridge IPs.
    #[must_use]
    pub fn new(config: HueConfig, credential_store: Arc<CredentialStore>) -> Self {
        Self {
            config,
            credential_store,
            discovered: Arc::new(StdRwLock::new(HashMap::new())),
            sessions: SessionTable::new(),
        }
    }

    fn opener(&self) -> HueOpener {
        HueOpener {
            config: self.config.clone(),
            credential_store: Arc::clone(&self.credential_store),
            discovered: Arc::clone(&self.discovered),
        }
    }
}

/// What an open takes from the backend, since it runs on the session table's
/// task and outlives the connect that started it.
struct HueOpener {
    config: HueConfig,
    credential_store: Arc<CredentialStore>,
    discovered: Arc<StdRwLock<HashMap<DeviceId, HueDiscoveredBridge>>>,
}

impl HueOpener {
    #[expect(
        clippy::too_many_lines,
        reason = "the Hue connect walks credentials, bridge state, entertainment setup, and the stream in one sequence"
    )]
    async fn open_session(
        &self,
        id: &DeviceId,
        discovered: HueDiscoveredBridge,
    ) -> std::result::Result<Session<HueSession>, DeviceError> {
        let (api_key, client_key) =
            load_bridge_credentials(&self.credential_store, &discovered.bridge_id, discovered.ip)
                .await
                .with_context(|| {
                    format!(
                        "Hue bridge {} at {} requires pairing credentials",
                        discovered.info.name, discovered.ip
                    )
                })
                .map_err(|error| DeviceError::connection(id, error))?;

        let client = HueBridgeClient::authenticated_with_port(
            discovered.ip,
            discovered.api_port,
            api_key.clone(),
        );
        let bridge_identity = client.bridge_identity().await.unwrap_or(HueBridgeIdentity {
            bridge_id: discovered.bridge_id.clone(),
            name: discovered.info.name.clone(),
            model_id: discovered.info.model.clone().unwrap_or_default(),
            sw_version: discovered.info.firmware_version.clone().unwrap_or_default(),
        });
        let lights = client
            .lights()
            .await
            .context("failed to fetch Hue bridge lights")
            .map_err(|error| DeviceError::connection(id, error))?;
        let configs = client
            .entertainment_configs()
            .await
            .context("failed to fetch Hue entertainment configurations")
            .map_err(|error| DeviceError::connection(id, error))?;
        let entertainment_config = choose_entertainment_config(
            self.config.entertainment_config.as_deref(),
            configs.as_slice(),
        )
        .or(discovered.entertainment_config.clone())
        .with_context(|| {
            format!(
                "Hue bridge {} does not expose a compatible entertainment configuration",
                discovered.info.name
            )
        })
        .map_err(|error| DeviceError::connection(id, error))?;

        client
            .start_streaming(&entertainment_config.id)
            .await
            .with_context(|| {
                format!(
                    "failed to activate Hue entertainment config {} on {}",
                    entertainment_config.name, discovered.info.name
                )
            })
            .map_err(|error| DeviceError::connection(id, error))?;

        let stream = match HueStreamSession::connect(
            discovered.ip,
            &api_key,
            &client_key,
            &entertainment_config.id,
            entertainment_config.channels.clone(),
        )
        .await
        {
            Ok(stream) => stream,
            Err(error) => {
                let _ = client.stop_streaming(&entertainment_config.id).await;
                let error = error.context(format!(
                    "failed to establish Hue entertainment stream for {}",
                    discovered.info.name
                ));
                return Err(DeviceError::connection(id, error));
            }
        };

        let info = build_device_info(
            &bridge_identity.bridge_id,
            &bridge_identity.name,
            Some(bridge_identity.model_id.as_str()),
            Some(bridge_identity.sw_version.as_str()),
            Some(&entertainment_config),
            lights.as_slice(),
        );
        let channel_gamuts =
            resolve_channel_gamuts(entertainment_config.channels.as_slice(), lights.as_slice());

        self.discovered
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(
                *id,
                HueDiscoveredBridge {
                    bridge_id: bridge_identity.bridge_id.clone(),
                    ip: discovered.ip,
                    api_port: discovered.api_port,
                    info: info.clone(),
                    entertainment_config: Some(entertainment_config.clone()),
                    lights,
                    connect_behavior: discovered.connect_behavior,
                    metadata: discovered.metadata,
                },
            );

        info!(
            device_id = %id,
            bridge_id = %bridge_identity.bridge_id,
            ip = %discovered.ip,
            channels = entertainment_config.channels.len(),
            "Connected to Hue bridge"
        );
        Ok(Session::new(
            *id,
            (),
            HueSession {
                device_id: *id,
                bridge_id: bridge_identity.bridge_id.clone(),
                client,
                stream,
                entertainment_config,
                channel_gamuts,
                info,
                brightness: u8::MAX,
                use_cie_xy: self.config.use_cie_xy,
                last_size_mismatch_warn_at: None,
            },
        ))
    }
}

#[async_trait::async_trait]
impl DeviceBackend for HueBackend {
    fn info(&self) -> BackendInfo {
        BackendInfo {
            id: "hue".to_owned(),
            name: "Philips Hue".to_owned(),
            description: "Philips Hue lights via Entertainment streaming".to_owned(),
        }
    }

    fn adopt_device(&self, discovered: &DiscoveredDevice) -> Result<(), DeviceError> {
        let ip = discovered
            .metadata
            .get("ip")
            .and_then(|value| value.parse::<IpAddr>().ok())
            .ok_or(DeviceError::NotAdopted {
                device_id: discovered.info.id,
            })?;
        let api_port = discovered
            .metadata
            .get("api_port")
            .and_then(|value| value.parse::<u16>().ok())
            .ok_or(DeviceError::NotAdopted {
                device_id: discovered.info.id,
            })?;
        let bridge_id =
            discovered
                .metadata
                .get("bridge_id")
                .cloned()
                .ok_or(DeviceError::NotAdopted {
                    device_id: discovered.info.id,
                })?;
        self.discovered
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(
                discovered.info.id,
                HueDiscoveredBridge {
                    bridge_id,
                    ip,
                    api_port,
                    info: discovered.info.clone(),
                    entertainment_config: None,
                    lights: Vec::new(),
                    connect_behavior: discovered.connect_behavior,
                    metadata: discovered.metadata.clone(),
                },
            );
        Ok(())
    }

    async fn connected_device_info(
        &self,
        id: &DeviceId,
    ) -> std::result::Result<Option<DeviceInfo>, DeviceError> {
        let Some(sink) = self.sessions.sink(id) else {
            return Ok(None);
        };
        let Ok(bridge) = sink.admit().await else {
            return Ok(None);
        };
        let client = bridge.client.clone();
        let config_id = bridge.entertainment_config.id.clone();
        drop(bridge);
        // Discovery can wait on HTTP without blocking the frame sink's DTLS writes.
        let refreshed = async {
            let (lights, configs) =
                tokio::try_join!(client.lights(), client.entertainment_configs())?;
            let config = configs
                .into_iter()
                .find(|config| config.id == config_id)
                .context("active Hue entertainment configuration is missing")?;
            Ok::<_, anyhow::Error>((lights, config))
        }
        .await;
        let Ok(mut bridge) = sink.admit().await else {
            return Ok(None);
        };
        match refreshed {
            Ok((lights, config)) => {
                let gamuts = resolve_channel_gamuts(&config.channels, &lights);
                let info = build_device_info(
                    &bridge.bridge_id,
                    &bridge.info.name,
                    bridge.info.model.as_deref(),
                    bridge.info.firmware_version.as_deref(),
                    Some(&config),
                    &lights,
                );
                if let Err(error) = bridge.stream.update_channels(config.channels.clone()) {
                    warn!(device_id = %id, %error, "retaining Hue topology after invalid refresh");
                } else {
                    bridge.entertainment_config = config;
                    bridge.channel_gamuts = gamuts;
                    bridge.info = info;
                }
            }
            Err(error) => {
                warn!(device_id = %id, %error, "retaining Hue topology after failed refresh");
            }
        }
        Ok(Some(bridge.info.clone()))
    }

    fn supports_temporary_direct_control(&self, _info: &DeviceInfo) -> bool {
        true
    }

    fn lifecycle_policy(&self, _info: &DeviceInfo) -> DeviceLifecyclePolicy {
        DeviceLifecyclePolicy::default().with_connect_timeout(HUE_CONNECT_TIMEOUT)
    }

    async fn connect(&self, id: &DeviceId) -> std::result::Result<(), DeviceError> {
        let id = *id;
        self.sessions
            .open(&id, || {
                let discovered = self
                    .discovered
                    .read()
                    .unwrap_or_else(PoisonError::into_inner)
                    .get(&id)
                    .cloned();
                let opener = self.opener();
                async move {
                    let discovered = discovered.ok_or(DeviceError::NotAdopted { device_id: id })?;
                    opener.open_session(&id, discovered).await
                }
            })
            .await
    }

    async fn disconnect(&self, id: &DeviceId) -> std::result::Result<(), DeviceError> {
        let result = self.sessions.close(id).await;
        info!(device_id = %id, "Disconnected from Hue bridge");
        result
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
        self.sessions.require(id)?.admit().await?.brightness = brightness;
        Ok(())
    }

    fn output_policy(&self, _id: &DeviceId) -> Option<OutputPolicy> {
        Some(OutputPolicy::from_fps(50).with_replay_after(HUE_MAX_FRAME_SILENCE))
    }

    fn frame_sink(&self, id: &DeviceId) -> Option<Arc<dyn DeviceFrameSink>> {
        self.sessions.frame_sink(id)
    }
}

async fn load_bridge_credentials(
    credential_store: &CredentialStore,
    bridge_id: &str,
    ip: IpAddr,
) -> Option<(String, String)> {
    for key in [bridge_id.to_owned(), format!("ip:{ip}")] {
        let Some(credentials) = credential_store.get_driver_json("hue", &key).await else {
            continue;
        };
        let Some(api_key) = credentials
            .get("api_key")
            .and_then(serde_json::Value::as_str)
        else {
            continue;
        };
        let Some(client_key) = credentials
            .get("client_key")
            .and_then(serde_json::Value::as_str)
        else {
            continue;
        };
        return Some((api_key.to_owned(), client_key.to_owned()));
    }
    None
}

fn resolve_channel_gamuts(
    channels: &[super::types::HueChannel],
    lights: &[HueLight],
) -> Vec<ColorGamut> {
    let lights_by_id: HashMap<&str, &HueLight> = lights
        .iter()
        .map(|light| (light.id.as_str(), light))
        .collect();

    channels
        .iter()
        .map(|channel| {
            channel
                .members
                .iter()
                .filter_map(|member| member.light_id.as_deref())
                .find_map(|light_id| lights_by_id.get(light_id).copied())
                .map_or(super::color::GAMUT_C, HueLight::resolved_gamut)
        })
        .collect()
}

fn collapse_channel_colors(
    channels: &[super::types::HueChannel],
    colors: &[[u8; 3]],
    brightness: u8,
) -> Vec<[u8; 3]> {
    let mut offset = 0_usize;
    let mut collapsed = Vec::with_capacity(channels.len());

    for channel in channels {
        let segment_count = usize::try_from(channel.segment_count.max(1)).unwrap_or(usize::MAX);
        let channel_slice_end = colors.len().min(offset.saturating_add(segment_count));
        let channel_slice = &colors[offset.min(colors.len())..channel_slice_end];
        let averaged = average_colors(channel_slice);
        collapsed.push(scale_color(averaged, brightness));
        offset = offset.saturating_add(segment_count);
    }

    collapsed
}

fn average_colors(colors: &[[u8; 3]]) -> [u8; 3] {
    if colors.is_empty() {
        return [0, 0, 0];
    }

    let mut totals = [0_u32; 3];
    for [red, green, blue] in colors.iter().copied() {
        totals[0] += u32::from(red);
        totals[1] += u32::from(green);
        totals[2] += u32::from(blue);
    }
    let count = u32::try_from(colors.len()).unwrap_or(u32::MAX).max(1);

    [
        u8::try_from(totals[0] / count).unwrap_or(u8::MAX),
        u8::try_from(totals[1] / count).unwrap_or(u8::MAX),
        u8::try_from(totals[2] / count).unwrap_or(u8::MAX),
    ]
}

fn scale_color([red, green, blue]: [u8; 3], brightness: u8) -> [u8; 3] {
    if brightness == u8::MAX {
        return [red, green, blue];
    }

    let scaled = Rgb::new(red, green, blue).scale(f32::from(brightness) / f32::from(u8::MAX));
    [scaled.r, scaled.g, scaled.b]
}
