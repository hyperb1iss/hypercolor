//! Nanoleaf panel touch input over the SSE-owned UDP stream.

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use hypercolor_driver_api::DeviceInputSink;
use hypercolor_types::device::DeviceId;
use hypercolor_types::device_input::{DeviceInputEdge, TouchPosition};
use hypercolor_types::spatial::LedTopology;
use tokio::net::UdpSocket;

use crate::NanoleafPanelLayout;

/// Translates Nanoleaf's panel-level contacts into device-attributed input.
/// Panel touch sensors report strength, not a position within the panel.
#[doc(hidden)]
pub struct NanoleafTouchDecoder {
    panels: HashMap<u16, TouchPanel>,
    held: HashSet<u16>,
}

#[derive(Clone)]
struct TouchPanel {
    segment: Arc<str>,
    position: TouchPosition,
}

impl NanoleafTouchDecoder {
    /// Select the touch surfaces present in a Canvas or Shapes layout.
    #[must_use]
    pub fn new(panels: &[NanoleafPanelLayout], global_orientation: f32) -> Self {
        Self {
            panels: panels
                .iter()
                .filter(|panel| matches!(panel.shape_type, 2..=4 | 7..=9))
                .filter_map(|panel| {
                    let hint = crate::geometry::panel_layout_hint(panel, global_orientation)?;
                    let LedTopology::Custom { positions } = hint.topology? else {
                        return None;
                    };
                    let position = positions.first()?;
                    Some((
                        panel.panel_id,
                        TouchPanel {
                            segment: Arc::from(format!("Panel {}", panel.panel_id)),
                            position: TouchPosition {
                                x: position.x,
                                y: position.y,
                                pressure: 0.0,
                            },
                        },
                    ))
                })
                .collect(),
            held: HashSet::new(),
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.panels.is_empty()
    }

    /// Decode one complete datagram, without changing held state on malformed input.
    ///
    /// # Errors
    /// Returns an error for truncated packets, trailing bytes, or unknown touch types.
    pub fn decode(&mut self, packet: &[u8]) -> Result<Vec<DeviceInputEdge>> {
        let Some(count) = packet.get(..2) else {
            bail!("Nanoleaf touch packet is missing its panel count");
        };
        let count = usize::from(u16::from_be_bytes([count[0], count[1]]));
        if packet.len() != 2 + count * 5 {
            bail!("Nanoleaf touch packet length does not match its panel count");
        }
        let records = packet[2..].as_chunks::<5>().0;
        if records.iter().any(|record| (record[2] >> 4) & 7 > 4) {
            bail!("Nanoleaf touch packet contains an unknown touch type");
        }

        let mut edges = Vec::new();
        for record in records {
            let panel = u16::from_be_bytes([record[0], record[1]]);
            let kind = (record[2] >> 4) & 7;
            let Some(surface) = self.panels.get(&panel).cloned() else {
                continue;
            };
            let position = TouchPosition {
                pressure: f32::from(record[2] & 0x0f) / 15.0,
                ..surface.position
            };
            if kind == 4 {
                let from = u16::from_be_bytes([record[3], record[4]]);
                if from != panel {
                    self.release(from, &mut edges);
                }
            }
            match kind {
                1 | 2 | 4 => {
                    if self.held.insert(panel) {
                        edges.push(DeviceInputEdge::TouchBegan {
                            contact: u32::from(panel),
                            position,
                            segment: Some(surface.segment),
                        });
                    } else {
                        edges.push(DeviceInputEdge::TouchMoved {
                            contact: u32::from(panel),
                            position,
                        });
                    }
                }
                3 => self.release(panel, &mut edges),
                _ => {}
            }
        }
        Ok(edges)
    }

    fn release(&mut self, panel: u16, edges: &mut Vec<DeviceInputEdge>) {
        if self.held.remove(&panel)
            && let Some(surface) = self.panels.get(&panel)
        {
            edges.push(DeviceInputEdge::TouchEnded {
                contact: u32::from(panel),
                position: surface.position,
            });
        }
    }
}

pub(crate) async fn run(
    ip: IpAddr,
    api_port: u16,
    token: &str,
    device_id: DeviceId,
    label: &str,
    mut decoder: NanoleafTouchDecoder,
    sink: Arc<dyn DeviceInputSink>,
) -> Result<()> {
    let bind = if ip.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" };
    let mut packet = vec![0_u8; 65_535];
    loop {
        let socket = UdpSocket::bind(bind)
            .await
            .context("bind Nanoleaf touch socket")?;
        let response = crate::api::AuthenticatedApi::new(ip, api_port, token)
            .touch_events(socket.local_addr()?.port())
            .await;
        let mut response = match response {
            Ok(response) if response.status().is_success() => response,
            Ok(response) if response.status().is_client_error() => {
                bail!(
                    "Nanoleaf touch subscription rejected: HTTP {}",
                    response.status()
                );
            }
            _ => {
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            }
        };
        let publisher = sink.attach(device_id, label);
        decoder.held.clear();
        loop {
            tokio::select! {
                chunk = response.receive() => {
                    if !matches!(chunk, Ok(true)) {
                        break;
                    }
                }
                received = socket.recv_from(&mut packet) => {
                    let (length, sender) = received.context("receive Nanoleaf touch packet")?;
                    if sender.ip() != ip {
                        continue;
                    }
                    if let Ok(edges) = decoder.decode(&packet[..length])
                        && !publisher.publish(&edges)
                    {
                        return Ok(());
                    }
                }
            }
        }
        // A gap retires held contacts before the next SSE session starts.
        drop(publisher);
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}
