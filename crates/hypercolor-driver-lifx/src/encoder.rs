//! Frame encoding: engine RGB frames to LIFX datagrams per device class
//! (Spec 88 §5).

use std::ops::Range;

use crate::color::{Hsbk, HsbkEncoder};
use crate::protocol::{
    ApplyMode, EXTENDED_ZONES_PER_MESSAGE, FrameAddress, Message, ProtocolError, SET64_COLORS,
    Serial, encode_message,
};
use crate::topology::{LifxTopology, MatrixSlot, TileGeometry};

/// Frame buffer the hidden-buffer path writes into before copying.
const HIDDEN_FRAME_BUFFER: u8 = 1;
const VISIBLE_FRAME_BUFFER: u8 = 0;

/// Output strategy for one tile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TilePlan {
    pub width: u8,
    pub height: u8,
    /// Write to a hidden frame buffer and copy it into view.
    pub buffered: bool,
}

impl TilePlan {
    fn from_geometry(tile: &TileGeometry) -> Self {
        Self {
            width: tile.width,
            height: tile.height,
            buffered: tile.zone_count() > SET64_COLORS && tile.supported_frame_buffers > 1,
        }
    }

    fn zone_count(self) -> usize {
        usize::from(self.width) * usize::from(self.height)
    }

    /// Rows one Set64 carries.
    fn rows_per_message(self) -> usize {
        (SET64_COLORS / usize::from(self.width.max(1))).max(1)
    }

    fn set64_messages(self) -> usize {
        usize::from(self.height).div_ceil(self.rows_per_message())
    }

    fn messages(self) -> usize {
        let chunks = self.set64_messages();
        if self.buffered { chunks + 1 } else { chunks }
    }
}

/// How frames are painted on one device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputPlan {
    /// One SetColor.
    Single,
    /// SetColorZones runs.
    Legacy { zones: u8 },
    /// SetExtendedColorZones chunks of 82.
    Extended { zones: u16 },
    /// Set64 per tile, with the hidden-buffer path above 64 zones.
    Matrix {
        tiles: Vec<TilePlan>,
        slots: Vec<MatrixSlot>,
    },
}

impl OutputPlan {
    /// Plan for a probed topology.
    #[must_use]
    pub fn from_topology(topology: &LifxTopology) -> Self {
        match topology {
            LifxTopology::SingleZone => Self::Single,
            LifxTopology::Multizone {
                zones,
                extended: true,
            } => Self::Extended { zones: *zones },
            LifxTopology::Multizone {
                zones,
                extended: false,
            } => Self::Legacy {
                zones: u8::try_from(*zones).unwrap_or(u8::MAX),
            },
            LifxTopology::Matrix { tiles, .. } => Self::Matrix {
                tiles: tiles.iter().map(TilePlan::from_geometry).collect(),
                slots: topology
                    .matrix_layout()
                    .map(|layout| layout.slots)
                    .unwrap_or_default(),
            },
        }
    }

    /// Engine LEDs one frame carries.
    #[must_use]
    pub fn led_count(&self) -> usize {
        match self {
            Self::Single => 1,
            Self::Legacy { zones } => usize::from(*zones),
            Self::Extended { zones } => usize::from(*zones),
            Self::Matrix { slots, .. } => slots.len(),
        }
    }

    /// Datagrams the most expensive frame needs, never below one.
    #[must_use]
    pub fn worst_case_messages(&self) -> u32 {
        let messages = match self {
            Self::Single => 1,
            Self::Legacy { zones } => usize::from(*zones),
            Self::Extended { zones } => usize::from(*zones).div_ceil(EXTENDED_ZONES_PER_MESSAGE),
            Self::Matrix { tiles, .. } => tiles.iter().map(|tile| tile.messages()).sum(),
        };
        u32::try_from(messages.max(1)).unwrap_or(u32::MAX)
    }

    /// Datagrams the cheapest frame needs, never below one. Only legacy
    /// strips vary: a frame costs one message per run of equal colors.
    #[must_use]
    pub fn best_case_messages(&self) -> u32 {
        match self {
            Self::Legacy { .. } => 1,
            _ => self.worst_case_messages(),
        }
    }
}

/// Header fields shared by every datagram of a device stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamAddress {
    pub source: u32,
    pub target: Serial,
    pub sequence: u8,
}

impl StreamAddress {
    fn next(&mut self) -> FrameAddress {
        let address = FrameAddress {
            source: self.source,
            target: self.target,
            ack_required: false,
            res_required: false,
            sequence: self.sequence,
        };
        self.sequence = self.sequence.wrapping_add(1);
        address
    }
}

/// Reusable frame encoder for one device.
#[derive(Debug, Clone)]
pub struct FrameEncoder {
    plan: OutputPlan,
    colors: Vec<Hsbk>,
    frame_buffers: Vec<Vec<Hsbk>>,
    buf: Vec<u8>,
    datagrams: Vec<Range<usize>>,
}

impl FrameEncoder {
    #[must_use]
    pub fn new(plan: OutputPlan) -> Self {
        let frame_buffers = match &plan {
            OutputPlan::Matrix { tiles, .. } => tiles
                .iter()
                .map(|tile| Vec::with_capacity(tile.zone_count()))
                .collect(),
            _ => Vec::new(),
        };
        Self {
            plan,
            colors: Vec::new(),
            frame_buffers,
            buf: Vec::new(),
            datagrams: Vec::new(),
        }
    }

    #[must_use]
    pub const fn plan(&self) -> &OutputPlan {
        &self.plan
    }

    /// Encode one frame. Short frames pad with black and long frames are
    /// truncated to the plan's LED count. Returns the datagram count.
    ///
    /// # Errors
    ///
    /// Returns a [`ProtocolError`] if a message cannot be encoded, which
    /// only happens for impossible geometry.
    pub fn encode(
        &mut self,
        rgb: &[[u8; 3]],
        converter: &HsbkEncoder,
        duration_ms: u32,
        address: &mut StreamAddress,
    ) -> Result<usize, ProtocolError> {
        self.buf.clear();
        self.datagrams.clear();
        let led_count = self.plan.led_count();
        self.colors.clear();
        self.colors.extend(
            (0..led_count).map(|index| converter.encode(rgb.get(index).copied().unwrap_or([0; 3]))),
        );
        let black = Hsbk::new(0, 0, 0, converter.kelvin);

        match &self.plan {
            OutputPlan::Single => {
                let color = self.colors.first().copied().unwrap_or(black);
                push_datagram(
                    &mut self.buf,
                    &mut self.datagrams,
                    address,
                    &Message::SetColor { color, duration_ms },
                )?;
            }
            OutputPlan::Legacy { .. } => {
                encode_legacy(
                    &self.colors,
                    duration_ms,
                    address,
                    &mut self.buf,
                    &mut self.datagrams,
                )?;
            }
            OutputPlan::Extended { .. } => {
                let last = self
                    .colors
                    .len()
                    .div_ceil(EXTENDED_ZONES_PER_MESSAGE)
                    .saturating_sub(1);
                for (chunk_index, chunk) in
                    self.colors.chunks(EXTENDED_ZONES_PER_MESSAGE).enumerate()
                {
                    let zone_index =
                        u16::try_from(chunk_index * EXTENDED_ZONES_PER_MESSAGE).unwrap_or(u16::MAX);
                    push_datagram(
                        &mut self.buf,
                        &mut self.datagrams,
                        address,
                        &Message::SetExtendedColorZones {
                            duration_ms,
                            apply: if chunk_index == last {
                                ApplyMode::Apply
                            } else {
                                ApplyMode::NoApply
                            },
                            zone_index,
                            colors: chunk,
                        },
                    )?;
                }
            }
            OutputPlan::Matrix { tiles, slots } => {
                for (frame_buffer, tile) in self.frame_buffers.iter_mut().zip(tiles) {
                    frame_buffer.clear();
                    frame_buffer.resize(tile.zone_count(), black);
                }
                for (slot, color) in slots.iter().zip(&self.colors) {
                    if let Some(zone) = self
                        .frame_buffers
                        .get_mut(usize::from(slot.tile))
                        .and_then(|frame_buffer| frame_buffer.get_mut(usize::from(slot.zone)))
                    {
                        *zone = *color;
                    }
                }
                for (tile_index, (tile, frame_buffer)) in
                    tiles.iter().zip(&self.frame_buffers).enumerate()
                {
                    encode_tile(
                        u8::try_from(tile_index).unwrap_or(u8::MAX),
                        *tile,
                        frame_buffer,
                        duration_ms,
                        address,
                        &mut self.buf,
                        &mut self.datagrams,
                    )?;
                }
            }
        }
        Ok(self.datagrams.len())
    }

    /// Datagrams produced by the last [`FrameEncoder::encode`] call.
    pub fn datagrams(&self) -> impl ExactSizeIterator<Item = &[u8]> + '_ {
        self.datagrams.iter().map(|range| &self.buf[range.clone()])
    }

    /// The HSBK colors computed for the last frame, in engine LED order.
    #[must_use]
    pub fn colors(&self) -> &[Hsbk] {
        &self.colors
    }
}

fn push_datagram(
    buf: &mut Vec<u8>,
    datagrams: &mut Vec<Range<usize>>,
    address: &mut StreamAddress,
    message: &Message<'_>,
) -> Result<(), ProtocolError> {
    let start = buf.len();
    encode_message(buf, &address.next(), message)?;
    datagrams.push(start..buf.len());
    Ok(())
}

fn encode_legacy(
    colors: &[Hsbk],
    duration_ms: u32,
    address: &mut StreamAddress,
    buf: &mut Vec<u8>,
    datagrams: &mut Vec<Range<usize>>,
) -> Result<(), ProtocolError> {
    let mut runs: Vec<(usize, usize, Hsbk)> = Vec::new();
    for (index, color) in colors.iter().enumerate() {
        match runs.last_mut() {
            Some((_, end, run_color)) if *run_color == *color => *end = index,
            _ => runs.push((index, index, *color)),
        }
    }
    let last = runs.len().saturating_sub(1);
    for (run_index, (start, end, color)) in runs.into_iter().enumerate() {
        push_datagram(
            buf,
            datagrams,
            address,
            &Message::SetColorZones {
                start_index: u8::try_from(start).unwrap_or(u8::MAX),
                end_index: u8::try_from(end).unwrap_or(u8::MAX),
                color,
                duration_ms,
                apply: if run_index == last {
                    ApplyMode::Apply
                } else {
                    ApplyMode::NoApply
                },
            },
        )?;
    }
    Ok(())
}

fn encode_tile(
    tile_index: u8,
    tile: TilePlan,
    frame_buffer: &[Hsbk],
    duration_ms: u32,
    address: &mut StreamAddress,
    buf: &mut Vec<u8>,
    datagrams: &mut Vec<Range<usize>>,
) -> Result<(), ProtocolError> {
    let width = usize::from(tile.width.max(1));
    let rows_per_message = tile.rows_per_message();
    let chunk_len = (rows_per_message * width).min(SET64_COLORS);
    let fb_index = if tile.buffered {
        HIDDEN_FRAME_BUFFER
    } else {
        VISIBLE_FRAME_BUFFER
    };
    let set_duration = if tile.buffered { 0 } else { duration_ms };

    for (chunk_index, chunk) in frame_buffer.chunks(chunk_len).enumerate() {
        let y = u8::try_from(chunk_index * rows_per_message).unwrap_or(u8::MAX);
        push_datagram(
            buf,
            datagrams,
            address,
            &Message::Set64 {
                tile_index,
                length: 1,
                fb_index,
                x: 0,
                y,
                width: tile.width,
                duration_ms: set_duration,
                colors: chunk,
            },
        )?;
    }

    if tile.buffered {
        push_datagram(
            buf,
            datagrams,
            address,
            &Message::CopyFrameBuffer {
                tile_index,
                length: 1,
                src_fb_index: HIDDEN_FRAME_BUFFER,
                dst_fb_index: VISIBLE_FRAME_BUFFER,
                src_x: 0,
                src_y: 0,
                dst_x: 0,
                dst_y: 0,
                width: tile.width,
                height: tile.height,
                duration_ms,
            },
        )?;
    }
    Ok(())
}
