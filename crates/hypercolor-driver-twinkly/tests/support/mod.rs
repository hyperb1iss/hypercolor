//! In-repo fake Twinkly device bound to 127.0.0.1.
//!
//! One fake answers the REST API on a TCP port, the discovery datagram on
//! one UDP port, and records real-time datagrams on another. Replies are
//! written by hand here rather than with the crate's encoders, so the tests
//! check the driver against an independent byte layout.

#![allow(dead_code, reason = "each test binary uses a different subset")]

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use hypercolor_driver_twinkly::TwinklyEndpoint;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::task::JoinHandle;

pub type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// What the fake reports about itself.
#[derive(Debug, Clone)]
pub struct FakeSpec {
    pub mac: String,
    pub device_name: String,
    pub product_code: String,
    pub fw_family: Option<String>,
    pub firmware: String,
    pub led_count: u32,
    pub led_profile: String,
    pub frame_rate: f64,
    /// Layout JSON body, or `None` to answer the layout with HTTP 404.
    pub layout: Option<serde_json::Value>,
    /// Token lifetime reported by login, in seconds.
    pub token_lifetime: u64,
    /// Issue the same token text on every login.
    pub reuse_tokens: bool,
}

impl FakeSpec {
    /// A generation II RGB string on firmware that takes version 3 frames.
    pub fn gen2_rgb(led_count: u32) -> Self {
        Self {
            mac: "98:f4:ab:1c:c1:90".to_owned(),
            device_name: "Porch String".to_owned(),
            product_code: "TWS250STP".to_owned(),
            fw_family: Some("F".to_owned()),
            firmware: "2.8.3".to_owned(),
            led_count,
            led_profile: "RGB".to_owned(),
            frame_rate: 23.81,
            layout: None,
            token_lifetime: 14_400,
            reuse_tokens: false,
        }
    }

    pub fn with_layout(mut self, layout: serde_json::Value) -> Self {
        self.layout = Some(layout);
        self
    }
}

/// One HTTP request the fake served.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRecord {
    pub method: String,
    pub path: String,
    pub token: Option<String>,
    pub body: String,
}

#[derive(Debug)]
struct State {
    spec: FakeSpec,
    issued: u8,
    valid_token: Option<String>,
    pending_token: Option<String>,
    mode: String,
    requests: Vec<HttpRecord>,
    datagrams: Vec<Vec<u8>>,
    discovery_requests: Vec<Vec<u8>>,
    failing_mode_posts: usize,
    last_rt_source: Option<SocketAddr>,
    slow_mode_restore: bool,
}

/// A running fake device.
pub struct FakeTwinkly {
    http_addr: SocketAddr,
    rt_addr: SocketAddr,
    discovery_addr: SocketAddr,
    state: Arc<Mutex<State>>,
    tasks: Vec<JoinHandle<()>>,
}

impl Drop for FakeTwinkly {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

impl FakeTwinkly {
    pub async fn spawn(spec: FakeSpec) -> TestResult<Self> {
        let loopback = IpAddr::V4(Ipv4Addr::LOCALHOST);
        let listener = TcpListener::bind((loopback, 0)).await?;
        let rt = UdpSocket::bind((loopback, 0)).await?;
        let discovery = UdpSocket::bind((loopback, 0)).await?;
        let http_addr = listener.local_addr()?;
        let rt_addr = rt.local_addr()?;
        let discovery_addr = discovery.local_addr()?;
        let state = Arc::new(Mutex::new(State {
            spec,
            issued: 0,
            valid_token: None,
            pending_token: None,
            mode: "movie".to_owned(),
            requests: Vec::new(),
            datagrams: Vec::new(),
            discovery_requests: Vec::new(),
            failing_mode_posts: 0,
            last_rt_source: None,
            slow_mode_restore: false,
        }));

        let http_state = Arc::clone(&state);
        let http = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let state = Arc::clone(&http_state);
                tokio::spawn(async move {
                    let _ = serve(stream, &state).await;
                });
            }
        });
        let rt_state = Arc::clone(&state);
        let rt_task = tokio::spawn(async move {
            let mut buffer = vec![0_u8; 2048];
            while let Ok((len, source)) = rt.recv_from(&mut buffer).await {
                let mut state = rt_state.lock().expect("state lock");
                state.datagrams.push(buffer[..len].to_vec());
                state.last_rt_source = Some(source);
            }
        });
        let discovery_state = Arc::clone(&state);
        let discovery_task = tokio::spawn(async move {
            let mut buffer = vec![0_u8; 256];
            while let Ok((len, source)) = discovery.recv_from(&mut buffer).await {
                let name = {
                    let mut state = discovery_state.lock().expect("state lock");
                    state.discovery_requests.push(buffer[..len].to_vec());
                    state.spec.device_name.clone()
                };
                if buffer[..len] != *b"\x01discover" {
                    continue;
                }
                // IPv4 127.0.0.1 with its octets reversed, then OK, name, NUL.
                let mut reply = vec![1, 0, 0, 127, b'O', b'K'];
                reply.extend_from_slice(name.as_bytes());
                reply.push(0);
                let _ = discovery.send_to(&reply, source).await;
            }
        });

        Ok(Self {
            http_addr,
            rt_addr,
            discovery_addr,
            state,
            tasks: vec![http, rt_task, discovery_task],
        })
    }

    pub fn endpoint(&self) -> TwinklyEndpoint {
        TwinklyEndpoint {
            ip: self.http_addr.ip(),
            http_port: self.http_addr.port(),
            rt_port: self.rt_addr.port(),
        }
    }

    pub const fn discovery_addr(&self) -> SocketAddr {
        self.discovery_addr
    }

    pub const fn http_port(&self) -> u16 {
        self.http_addr.port()
    }

    pub const fn rt_port(&self) -> u16 {
        self.rt_addr.port()
    }

    pub fn requests(&self) -> Vec<HttpRecord> {
        self.state.lock().expect("state lock").requests.clone()
    }

    pub fn datagrams(&self) -> Vec<Vec<u8>> {
        self.state.lock().expect("state lock").datagrams.clone()
    }

    /// Take a while to apply any mode switch other than real-time.
    pub fn slow_down_mode_restores(&self) {
        self.state.lock().expect("state lock").slow_mode_restore = true;
    }

    /// Address the newest real-time datagram came from.
    pub fn last_rt_source(&self) -> Option<SocketAddr> {
        self.state.lock().expect("state lock").last_rt_source
    }

    pub fn discovery_requests(&self) -> Vec<Vec<u8>> {
        self.state
            .lock()
            .expect("state lock")
            .discovery_requests
            .clone()
    }

    pub fn mode(&self) -> String {
        self.state.lock().expect("state lock").mode.clone()
    }

    /// Decoded bytes of the token the device currently honors.
    pub fn valid_token_bytes(&self) -> Option<[u8; 8]> {
        let token = self.state.lock().expect("state lock").valid_token.clone()?;
        let decoded = STANDARD.decode(token).ok()?;
        decoded.as_slice().try_into().ok()
    }

    /// Simulate another client logging in: the current token stops working.
    pub fn revoke_token(&self) {
        let mut state = self.state.lock().expect("state lock");
        state.issued = state.issued.wrapping_add(1);
        state.valid_token = Some(token_text(0xee, state.issued));
    }

    /// Answer the next `count` mode changes with HTTP 500.
    pub fn fail_mode_posts(&self, count: usize) {
        self.state.lock().expect("state lock").failing_mode_posts = count;
    }

    /// Simulate the Twinkly app switching to a movie.
    pub fn set_mode(&self, mode: &str) {
        mode.clone_into(&mut self.state.lock().expect("state lock").mode);
    }

    /// Wait until at least `count` datagrams arrived.
    pub async fn wait_for_datagrams(&self, count: usize) -> Vec<Vec<u8>> {
        for _ in 0..200 {
            let datagrams = self.datagrams();
            if datagrams.len() >= count {
                return datagrams;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        self.datagrams()
    }

    /// Wait until `predicate` holds for the recorded requests.
    pub async fn wait_for_requests(
        &self,
        predicate: impl Fn(&[HttpRecord]) -> bool,
    ) -> Vec<HttpRecord> {
        for _ in 0..300 {
            let requests = self.requests();
            if predicate(&requests) {
                return requests;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        self.requests()
    }
}

fn token_text(seed: u8, issued: u8) -> String {
    STANDARD.encode([seed, issued, 2, 3, 4, 5, 6, 7])
}

async fn serve(mut stream: TcpStream, state: &Mutex<State>) -> std::io::Result<()> {
    let mut raw = Vec::new();
    let mut buffer = [0_u8; 4096];
    let header_end = loop {
        let read = stream.read(&mut buffer).await?;
        if read == 0 {
            return Ok(());
        }
        raw.extend_from_slice(&buffer[..read]);
        if let Some(end) = raw.windows(4).position(|window| window == b"\r\n\r\n") {
            break end + 4;
        }
    };
    let head = String::from_utf8_lossy(&raw[..header_end]).to_string();
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_owned();
    let path = parts.next().unwrap_or_default().to_owned();
    let mut content_length = 0_usize;
    let mut token = None;
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            let name = name.trim().to_ascii_lowercase();
            let value = value.trim();
            if name == "content-length" {
                content_length = value.parse().unwrap_or(0);
            } else if name == "x-auth-token" {
                token = Some(value.to_owned());
            }
        }
    }
    while raw.len() < header_end + content_length {
        let read = stream.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        raw.extend_from_slice(&buffer[..read]);
    }
    let body_end = (header_end + content_length).min(raw.len());
    let body = String::from_utf8_lossy(&raw[header_end..body_end]).to_string();

    // A slow restore lands after any session that raced ahead of it.
    let slow_restore = state.lock().expect("state lock").slow_mode_restore;
    if slow_restore && path.ends_with("/led/mode") && method == "POST" && !body.contains("rt") {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let (status, reply) = respond(state, &method, &path, token.as_deref(), &body);
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
        reply.len()
    );
    stream.write_all(response.as_bytes()).await?;
    stream.shutdown().await
}

fn respond(
    state: &Mutex<State>,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: &str,
) -> (&'static str, String) {
    let mut state = state.lock().expect("state lock");
    state.requests.push(HttpRecord {
        method: method.to_owned(),
        path: path.to_owned(),
        token: token.map(ToOwned::to_owned),
        body: body.to_owned(),
    });
    let authorized = token.is_some() && token == state.valid_token.as_deref();
    let ok = |value: serde_json::Value| ("200 OK", value.to_string());
    let unauthorized = ("401 Unauthorized", "Invalid Token.".to_owned());

    match (method, path) {
        ("GET", "/xled/v1/gestalt") => {
            let spec = &state.spec;
            let mut gestalt = serde_json::json!({
                "product_name": "Twinkly",
                "hardware_version": "100",
                "bytes_per_led": if spec.led_profile == "RGBW" { 4 } else { 3 },
                "product_code": spec.product_code,
                "device_name": spec.device_name,
                "mac": spec.mac,
                "number_of_led": spec.led_count,
                "max_supported_led": 1200,
                "led_profile": spec.led_profile,
                "frame_rate": spec.frame_rate,
                "movie_capacity": 992,
                "code": 1000,
            });
            if let Some(family) = &spec.fw_family {
                gestalt["fw_family"] = serde_json::json!(family);
            }
            ok(gestalt)
        }
        ("GET", "/xled/v1/fw/version") => ok(serde_json::json!({
            "version": state.spec.firmware,
            "code": 1000,
        })),
        ("POST", "/xled/v1/login") => {
            let challenge = serde_json::from_str::<serde_json::Value>(body)
                .ok()
                .and_then(|value| {
                    value
                        .get("challenge")
                        .and_then(|c| c.as_str())
                        .map(ToOwned::to_owned)
                });
            if challenge.is_none_or(|challenge| {
                STANDARD
                    .decode(challenge)
                    .map_or(true, |bytes| bytes.len() != 32)
            }) {
                return ("200 OK", serde_json::json!({ "code": 1101 }).to_string());
            }
            if !state.spec.reuse_tokens {
                state.issued = state.issued.wrapping_add(1);
            }
            let token = token_text(
                0x5a,
                if state.spec.reuse_tokens {
                    1
                } else {
                    state.issued
                },
            );
            state.pending_token = Some(token.clone());
            // Logging in signs out the previous token right away.
            state.valid_token = None;
            ok(serde_json::json!({
                "authentication_token": token,
                "authentication_token_expires_in": state.spec.token_lifetime,
                "challenge-response": "0123456789abcdef0123456789abcdef01234567",
                "code": 1000,
            }))
        }
        ("POST", "/xled/v1/verify") => {
            let expected = serde_json::json!({
                "challenge-response": "0123456789abcdef0123456789abcdef01234567",
            });
            let parsed = serde_json::from_str::<serde_json::Value>(body).ok();
            if token.is_none()
                || token != state.pending_token.as_deref()
                || parsed.as_ref() != Some(&expected)
            {
                return unauthorized;
            }
            state.valid_token = state.pending_token.take();
            ok(serde_json::json!({ "code": 1000 }))
        }
        ("GET", "/xled/v1/led/mode") => {
            if !authorized {
                return unauthorized;
            }
            ok(serde_json::json!({ "mode": state.mode, "code": 1000 }))
        }
        ("POST", "/xled/v1/led/mode") => {
            if !authorized {
                return unauthorized;
            }
            if state.failing_mode_posts > 0 {
                state.failing_mode_posts -= 1;
                return ("500 Internal Server Error", "busy".to_owned());
            }
            let mode = serde_json::from_str::<serde_json::Value>(body)
                .ok()
                .and_then(|value| {
                    value
                        .get("mode")
                        .and_then(|m| m.as_str())
                        .map(ToOwned::to_owned)
                });
            match mode {
                Some(mode) => {
                    state.mode = mode;
                    ok(serde_json::json!({ "code": 1000 }))
                }
                None => ("200 OK", serde_json::json!({ "code": 1105 }).to_string()),
            }
        }
        ("GET", "/xled/v1/led/layout/full") => {
            if !authorized {
                return unauthorized;
            }
            match &state.spec.layout {
                Some(layout) => ok(layout.clone()),
                None => ("404 Not Found", "Resource not found.".to_owned()),
            }
        }
        _ => ("404 Not Found", "Resource not found.".to_owned()),
    }
}
