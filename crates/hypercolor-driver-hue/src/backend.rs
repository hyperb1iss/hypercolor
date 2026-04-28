//! Hue entertainment backend.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use hypercolor_core::device::net::CredentialStore;
use hypercolor_driver_api::{BackendInfo, DeviceBackend};
use hypercolor_types::device::{DeviceId, DeviceInfo};

use super::bridge::HueBridgeClient;
use super::color::{CieXyb, ColorGamut, rgb_to_cie_xyb};
use super::scanner::{HueKnownBridge, HueScanner};
use super::streaming::HueStreamSession;
use super::types::{
    HueBridgeIdentity, HueDiscoveredBridge, HueEntertainmentConfig, HueLight, build_device_info,
    choose_entertainment_config,
};

const SIZE_MISMATCH_WARN_INTERVAL: Duration = Duration::from_secs(60);

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
    mdns_enabled: bool,
    discovered: HashMap<DeviceId, HueDiscoveredBridge>,
    bridges: HashMap<DeviceId, HueBridgeState>,
}

struct HueBridgeState {
    bridge_id: String,
    ip: IpAddr,
    api_port: u16,
    client: HueBridgeClient,
    stream: HueStreamSession,
    entertainment_config: HueEntertainmentConfig,
    channel_gamuts: Vec<ColorGamut>,
    info: DeviceInfo,
    brightness: u8,
    last_size_mismatch_warn_at: Option<Instant>,
}

impl HueBackend {
    /// Create a new Hue backend using the configured manual bridge IPs.
    #[must_use]
    pub fn new(config: HueConfig, credential_store: Arc<CredentialStore>) -> Self {
        Self::with_mdns_enabled(config, credential_store, true)
    }

    /// Create a backend with explicit `mDNS` enablement.
    #[must_use]
    pub fn with_mdns_enabled(
        config: HueConfig,
        credential_store: Arc<CredentialStore>,
        mdns_enabled: bool,
    ) -> Self {
        Self {
            config,
            credential_store,
            mdns_enabled,
            discovered: HashMap::new(),
            bridges: HashMap::new(),
        }
    }

    /// Seed the backend with a previously discovered bridge.
    pub fn remember_bridge(&mut self, bridge: HueDiscoveredBridge) {
        self.discovered.insert(bridge.info.id, bridge);
    }

    fn known_bridges(&self) -> Vec<HueKnownBridge> {
        let mut known: HashMap<IpAddr, HueKnownBridge> = self
            .config
            .bridge_ips
            .iter()
            .copied()
            .map(HueKnownBridge::from_ip)
            .map(|bridge| (bridge.ip, bridge))
            .collect();

        for bridge in self.discovered.values() {
            known
                .entry(bridge.ip)
                .and_modify(|existing| {
                    if existing.bridge_id.is_empty() {
                        existing.bridge_id.clone_from(&bridge.bridge_id);
                    }
                    if existing.api_port == 0 {
                        existing.api_port = bridge.api_port;
                    }
                    if existing.name.is_empty() {
                        existing.name.clone_from(&bridge.info.name);
                    }
                    if existing.model_id.is_empty() {
                        existing.model_id = bridge.info.model.clone().unwrap_or_default();
                    }
                    if existing.sw_version.is_empty() {
                        existing.sw_version =
                            bridge.info.firmware_version.clone().unwrap_or_default();
                    }
                })
                .or_insert_with(|| HueKnownBridge {
                    bridge_id: bridge.bridge_id.clone(),
                    ip: bridge.ip,
                    api_port: bridge.api_port,
                    name: bridge.info.name.clone(),
                    model_id: bridge.info.model.clone().unwrap_or_default(),
                    sw_version: bridge.info.firmware_version.clone().unwrap_or_default(),
                });
        }

        let mut resolved: Vec<_> = known.into_values().collect();
        resolved.sort_by_key(|bridge| bridge.ip);
        resolved
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "Hue backend lifecycle bundles discovery enrichment, streaming bootstrap, and frame mapping"
)]
#[async_trait::async_trait]
impl DeviceBackend for HueBackend {
    fn info(&self) -> BackendInfo {
        BackendInfo {
            id: "hue".to_owned(),
            name: "Philips Hue".to_owned(),
            description: "Philips Hue lights via Entertainment streaming".to_owned(),
        }
    }

    async fn discover(&mut self) -> Result<Vec<DeviceInfo>> {
        let mut scanner = HueScanner::with_options(
            self.known_bridges(),
            Arc::clone(&self.credential_store),
            Duration::from_secs(2),
            self.mdns_enabled,
            self.config.entertainment_config.clone(),
        );
        let bridges = scanner.scan_bridges().await?;
        self.discovered = bridges
            .iter()
            .cloned()
            .map(|bridge| (bridge.info.id, bridge))
            .collect();

        Ok(bridges.into_iter().map(|bridge| bridge.info).collect())
    }

    async fn connected_device_info(&self, id: &DeviceId) -> Result<Option<DeviceInfo>> {
        Ok(self.bridges.get(id).map(|bridge| bridge.info.clone()))
    }

    async fn connect(&mut self, id: &DeviceId) -> Result<()> {
        if self.bridges.contains_key(id) {
            return Ok(());
        }

        let Some(discovered) = self.discovered.get(id).cloned() else {
            bail!("Hue bridge {id} is not known; run discovery first");
        };

        let (api_key, client_key) =
            load_bridge_credentials(&self.credential_store, &discovered.bridge_id, discovered.ip)
                .await
                .with_context(|| {
                    format!(
                        "Hue bridge {} at {} requires pairing credentials",
                        discovered.info.name, discovered.ip
                    )
                })?;

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
            .context("failed to fetch Hue bridge lights")?;
        let configs = client
            .entertainment_configs()
            .await
            .context("failed to fetch Hue entertainment configurations")?;
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
        })?;

        client
            .start_streaming(&entertainment_config.id)
            .await
            .with_context(|| {
                format!(
                    "failed to activate Hue entertainment config {} on {}",
                    entertainment_config.name, discovered.info.name
                )
            })?;

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
                return Err(error).with_context(|| {
                    format!(
                        "failed to establish Hue entertainment stream for {}",
                        discovered.info.name
                    )
                });
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

        self.discovered.insert(
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

        self.bridges.insert(
            *id,
            HueBridgeState {
                bridge_id: bridge_identity.bridge_id.clone(),
                ip: discovered.ip,
                api_port: discovered.api_port,
                client,
                stream,
                entertainment_config,
                channel_gamuts,
                info,
                brightness: u8::MAX,
                last_size_mismatch_warn_at: None,
            },
        );

        info!(
            device_id = %id,
            bridge_id = %bridge_identity.bridge_id,
            ip = %discovered.ip,
            channels = self
                .bridges
                .get(id)
                .map_or(0, |bridge| bridge.entertainment_config.channels.len()),
            "Connected to Hue bridge"
        );
        Ok(())
    }

    async fn disconnect(&mut self, id: &DeviceId) -> Result<()> {
        let Some(bridge) = self.bridges.remove(id) else {
            bail!("Hue bridge {id} is not connected");
        };

        let close_result = bridge.stream.close().await;
        let stop_result = bridge
            .client
            .stop_streaming(&bridge.entertainment_config.id)
            .await;

        info!(
            device_id = %id,
            bridge_id = %bridge.bridge_id,
            ip = %bridge.ip,
            api_port = bridge.api_port,
            "Disconnected from Hue bridge"
        );

        close_result?;
        stop_result?;
        Ok(())
    }

    async fn write_colors(&mut self, id: &DeviceId, colors: &[[u8; 3]]) -> Result<()> {
        let bridge = self
            .bridges
            .get_mut(id)
            .with_context(|| format!("Hue bridge {id} is not connected"))?;

        let expected_led_count =
            usize::try_from(bridge.info.total_led_count()).unwrap_or(usize::MAX);
        if colors.len() != expected_led_count {
            let should_warn = bridge
                .last_size_mismatch_warn_at
                .is_none_or(|last_warn_at| last_warn_at.elapsed() >= SIZE_MISMATCH_WARN_INTERVAL);
            if should_warn {
                warn!(
                    device_id = %id,
                    expected_led_count,
                    actual_led_count = colors.len(),
                    "Hue frame size mismatch; collapsing or padding to match entertainment channels"
                );
                bridge.last_size_mismatch_warn_at = Some(Instant::now());
            }
        }

        let collapsed = collapse_channel_colors(
            bridge.entertainment_config.channels.as_slice(),
            colors,
            bridge.brightness,
        );
        let cie_colors = collapsed
            .iter()
            .zip(bridge.channel_gamuts.iter().copied())
            .map(|(color, gamut)| rgb_to_cie_xyb(color[0], color[1], color[2], &gamut))
            .collect::<Vec<CieXyb>>();

        bridge.stream.send_frame(cie_colors.as_slice()).await
    }

    async fn set_brightness(&mut self, id: &DeviceId, brightness: u8) -> Result<()> {
        let bridge = self
            .bridges
            .get_mut(id)
            .with_context(|| format!("Hue bridge {id} is not connected"))?;
        bridge.brightness = brightness;
        Ok(())
    }

    fn target_fps(&self, _id: &DeviceId) -> Option<u32> {
        Some(50)
    }
}

async fn load_bridge_credentials(
    credential_store: &CredentialStore,
    bridge_id: &str,
    ip: IpAddr,
) -> Option<(String, String)> {
    for key in [format!("hue:{bridge_id}"), format!("hue:ip:{ip}")] {
        let Some(credentials) = credential_store.get_json(&key).await else {
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
        let channel_slice = &colors[offset..channel_slice_end];
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

    [
        scale_channel(red, brightness),
        scale_channel(green, brightness),
        scale_channel(blue, brightness),
    ]
}

fn scale_channel(channel: u8, brightness: u8) -> u8 {
    let scaled = (u16::from(channel) * u16::from(brightness)) / u16::from(u8::MAX);
    u8::try_from(scaled).unwrap_or(u8::MAX)
}
