use bytes::Bytes;
use serde_json::Value;
use std::time::Instant;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelDescriptor {
    pub id: u16,
    pub name: String,
}

impl ChannelDescriptor {
    pub fn new(id: u16, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    ClientToServer,
    ServerToClient,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SessionRecord {
    TransportFrame {
        channel_id: u16,
        direction: Direction,
        bytes: Bytes,
    },
    Metadata {
        channel_id: u16,
        key: String,
        value: Value,
    },
    External {
        source: &'static str,
        body: Bytes,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReplayEntry {
    pub elapsed_ns: u64,
    pub record: SessionRecord,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SessionTape {
    channels: Vec<ChannelDescriptor>,
    entries: Vec<ReplayEntry>,
}

impl SessionTape {
    #[must_use]
    pub fn new(channels: Vec<ChannelDescriptor>, entries: Vec<ReplayEntry>) -> Self {
        Self { channels, entries }
    }

    #[must_use]
    pub fn channels(&self) -> &[ChannelDescriptor] {
        &self.channels
    }

    #[must_use]
    pub fn entries(&self) -> &[ReplayEntry] {
        &self.entries
    }

    pub fn into_entries(self) -> Vec<ReplayEntry> {
        self.entries
    }
}

#[derive(Debug)]
pub struct SessionRecorder {
    channels: Vec<ChannelDescriptor>,
    started_at: Instant,
    entries: Vec<ReplayEntry>,
}

impl SessionRecorder {
    #[must_use]
    pub fn new(channels: impl Into<Vec<ChannelDescriptor>>) -> Self {
        Self {
            channels: channels.into(),
            started_at: Instant::now(),
            entries: Vec::new(),
        }
    }

    pub fn record_transport_frame(
        &mut self,
        channel_id: u16,
        direction: Direction,
        bytes: impl Into<Bytes>,
    ) {
        self.record(SessionRecord::TransportFrame {
            channel_id,
            direction,
            bytes: bytes.into(),
        });
    }

    pub fn record_metadata(&mut self, channel_id: u16, key: impl Into<String>, value: Value) {
        self.record(SessionRecord::Metadata {
            channel_id,
            key: key.into(),
            value,
        });
    }

    pub fn record_external(&mut self, source: &'static str, body: impl Into<Bytes>) {
        self.record(SessionRecord::External {
            source,
            body: body.into(),
        });
    }

    #[must_use]
    pub fn entries(&self) -> &[ReplayEntry] {
        &self.entries
    }

    #[must_use]
    pub fn finish(self) -> SessionTape {
        SessionTape {
            channels: self.channels,
            entries: self.entries,
        }
    }

    fn record(&mut self, record: SessionRecord) {
        let elapsed_ns = self
            .started_at
            .elapsed()
            .as_nanos()
            .min(u128::from(u64::MAX)) as u64;
        self.entries.push(ReplayEntry { elapsed_ns, record });
    }
}
