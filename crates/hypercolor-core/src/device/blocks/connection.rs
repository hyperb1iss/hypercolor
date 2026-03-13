//! Unix socket connection to blocksd.
//!
//! Handles NDJSON request/response and binary frame writes on the same socket.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::time::timeout;

use super::types::{DiscoverResponse, PongResponse};

/// Binary frame constants matching blocksd's protocol.
const BINARY_MAGIC: u8 = 0xBD;
const BINARY_TYPE_FRAME: u8 = 0x01;
/// 1 magic + 1 type + 4 uid (LE) + 675 pixels = 681 bytes.
const BINARY_FRAME_SIZE: usize = 681;
/// 15 × 15 × 3 bytes (RGB888).
const PIXEL_DATA_SIZE: usize = 675;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

/// Active connection to a blocksd instance.
pub struct BlocksConnection {
    reader: BufReader<tokio::io::ReadHalf<UnixStream>>,
    writer: tokio::io::WriteHalf<UnixStream>,
    read_buf: String,
    frame_buf: Vec<u8>,
}

impl BlocksConnection {
    /// Connect to blocksd at the given socket path.
    pub async fn connect(path: &Path) -> Result<Self> {
        let stream = timeout(CONNECT_TIMEOUT, UnixStream::connect(path))
            .await
            .context("blocksd connect timeout")?
            .with_context(|| format!("failed to connect to blocksd at {}", path.display()))?;

        let (read_half, write_half) = tokio::io::split(stream);

        Ok(Self {
            reader: BufReader::new(read_half),
            writer: write_half,
            read_buf: String::with_capacity(4096),
            frame_buf: vec![0u8; BINARY_FRAME_SIZE],
        })
    }

    /// Send a ping and verify blocksd is responsive.
    pub async fn ping(&mut self) -> Result<PongResponse> {
        let response = self.json_request(r#"{"type":"ping","id":"hc"}"#).await?;

        serde_json::from_str(&response).context("failed to parse pong response")
    }

    /// Discover all connected ROLI devices.
    pub async fn discover(&mut self) -> Result<DiscoverResponse> {
        let response = self
            .json_request(r#"{"type":"discover","id":"hc"}"#)
            .await?;

        serde_json::from_str(&response).context("failed to parse discover response")
    }

    /// Subscribe to device events.
    pub async fn subscribe(&mut self, events: &[&str]) -> Result<()> {
        let events_json: Vec<String> = events.iter().map(|e| format!("\"{e}\"")).collect();
        let request = format!(
            r#"{{"type":"subscribe","events":[{}]}}"#,
            events_json.join(",")
        );
        let _response = self.json_request(&request).await?;
        Ok(())
    }

    /// Set brightness for a device.
    pub async fn set_brightness(&mut self, uid: u32, brightness: u8) -> Result<()> {
        let request = format!(r#"{{"type":"brightness","uid":{uid},"value":{brightness}}}"#);
        let _response = self.json_request(&request).await?;
        Ok(())
    }

    /// Write an RGB888 frame using the binary fast path.
    ///
    /// Returns `Ok(true)` if accepted, `Ok(false)` if dropped (backpressure).
    pub async fn write_frame_binary(&mut self, uid: u32, colors: &[[u8; 3]]) -> Result<bool> {
        // Build binary frame: magic + type + uid (LE) + pixels
        self.frame_buf[0] = BINARY_MAGIC;
        self.frame_buf[1] = BINARY_TYPE_FRAME;
        self.frame_buf[2..6].copy_from_slice(&uid.to_le_bytes());

        // Copy pixel data (up to 225 pixels)
        let pixel_count = colors.len().min(225);
        for (i, color) in colors[..pixel_count].iter().enumerate() {
            let offset = 6 + i * 3;
            self.frame_buf[offset] = color[0];
            self.frame_buf[offset + 1] = color[1];
            self.frame_buf[offset + 2] = color[2];
        }

        // Zero-fill remaining pixels if fewer than 225
        if pixel_count < 225 {
            let start = 6 + pixel_count * 3;
            self.frame_buf[start..BINARY_FRAME_SIZE].fill(0);
        }

        self.writer
            .write_all(&self.frame_buf)
            .await
            .context("blocksd frame write failed")?;
        self.writer.flush().await?;

        // Read single-byte response
        let mut response = [0u8; 1];
        timeout(REQUEST_TIMEOUT, self.reader.read_exact(&mut response))
            .await
            .context("blocksd frame response timeout")?
            .context("blocksd frame response read failed")?;

        Ok(response[0] == 0x01)
    }

    /// Read the next server-sent event (blocking).
    pub async fn read_event(&mut self) -> Result<serde_json::Value> {
        self.read_buf.clear();
        self.reader
            .read_line(&mut self.read_buf)
            .await
            .context("blocksd event read failed")?;

        if self.read_buf.is_empty() {
            bail!("blocksd connection closed");
        }

        serde_json::from_str(&self.read_buf).context("failed to parse blocksd event")
    }

    // ── Internal ────────────────────────────────────────────────────────

    /// Send a JSON request and read the response line.
    async fn json_request(&mut self, request: &str) -> Result<String> {
        // Write request + newline
        self.writer
            .write_all(request.as_bytes())
            .await
            .context("blocksd write failed")?;
        self.writer
            .write_all(b"\n")
            .await
            .context("blocksd write newline failed")?;
        self.writer.flush().await?;

        // Read response line
        self.read_buf.clear();
        timeout(REQUEST_TIMEOUT, self.reader.read_line(&mut self.read_buf))
            .await
            .context("blocksd response timeout")?
            .context("blocksd response read failed")?;

        if self.read_buf.is_empty() {
            bail!("blocksd connection closed during request");
        }

        Ok(self.read_buf.clone())
    }
}

/// Check whether the blocksd socket exists at the given path.
pub fn socket_exists(path: &Path) -> bool {
    path.exists()
}

/// Default blocksd socket path from environment.
pub fn default_socket_path() -> PathBuf {
    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
        PathBuf::from(runtime_dir).join("blocksd/blocksd.sock")
    } else {
        PathBuf::from("/tmp/blocksd/blocksd.sock")
    }
}
