//! In-repo fake WiZ device bound to 127.0.0.1.
//!
//! Replies are written by hand rather than with the crate's encoders, and
//! signatures are checked with an HMAC computed here from first principles
//! via the `sha2` crate, so the tests check the driver against an
//! independent reading of the protocol.

#![allow(dead_code, reason = "each test binary uses a different subset")]

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use sha2::{Digest as _, Sha256};
use tokio::net::UdpSocket;
use tokio::task::JoinHandle;

pub type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// What the fake reports about itself.
#[derive(Debug, Clone)]
pub struct FakeSpec {
    pub mac: String,
    pub module_name: String,
    pub firmware: String,
    /// `None` answers `getModelConfig` with "Method not found".
    pub min_dim_level: Option<u8>,
    /// Require signed `setPilot` with this 16-byte key.
    pub signing_key: Option<[u8; 16]>,
}

impl FakeSpec {
    pub fn color_bulb() -> Self {
        Self {
            mac: "a8bb50000001".to_owned(),
            module_name: "ESP01_SHRGB1C_31".to_owned(),
            firmware: "1.25.0".to_owned(),
            min_dim_level: Some(10),
            signing_key: None,
        }
    }
}

/// One `setPilot` the fake accepted or rejected.
#[derive(Debug, Clone, PartialEq)]
pub struct PilotRecord {
    pub params: serde_json::Value,
    pub accepted: bool,
}

#[derive(Debug, Default)]
struct State {
    methods: Vec<String>,
    pilots: Vec<PilotRecord>,
    last_pilot_source: Option<SocketAddr>,
}

/// A running fake device.
pub struct FakeWiz {
    addr: SocketAddr,
    state: Arc<Mutex<State>>,
    task: JoinHandle<()>,
}

impl Drop for FakeWiz {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl FakeWiz {
    pub async fn spawn(spec: FakeSpec) -> TestResult<Self> {
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let addr = socket.local_addr()?;
        let state = Arc::new(Mutex::new(State::default()));
        let task_state = Arc::clone(&state);
        let task = tokio::spawn(async move {
            let mut buffer = vec![0_u8; 2048];
            while let Ok((len, source)) = socket.recv_from(&mut buffer).await {
                let raw = String::from_utf8_lossy(&buffer[..len]).to_string();
                if raw.contains("setPilot") {
                    task_state.lock().expect("state lock").last_pilot_source = Some(source);
                }
                if let Some(reply) = respond(&spec, &task_state, &raw) {
                    let _ = socket.send_to(reply.as_bytes(), source).await;
                }
            }
        });
        Ok(Self { addr, state, task })
    }

    pub const fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn methods(&self) -> Vec<String> {
        self.state.lock().expect("state lock").methods.clone()
    }

    pub fn pilots(&self) -> Vec<PilotRecord> {
        self.state.lock().expect("state lock").pilots.clone()
    }

    /// Address the newest `setPilot` came from.
    pub fn last_pilot_source(&self) -> Option<SocketAddr> {
        self.state.lock().expect("state lock").last_pilot_source
    }

    pub async fn wait_for_pilots(&self, count: usize) -> Vec<PilotRecord> {
        for _ in 0..200 {
            let pilots = self.pilots();
            if pilots.len() >= count {
                return pilots;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        self.pilots()
    }
}

fn respond(spec: &FakeSpec, state: &Mutex<State>, raw: &str) -> Option<String> {
    let request: serde_json::Value = serde_json::from_str(raw).ok()?;
    let method = request.get("method")?.as_str()?.to_owned();
    state
        .lock()
        .expect("state lock")
        .methods
        .push(method.clone());
    let error = |code: i64, message: &str| {
        serde_json::json!({
            "method": method,
            "env": "pro",
            "error": { "code": code, "message": message },
        })
        .to_string()
    };
    let ok = |result: serde_json::Value| {
        serde_json::json!({ "method": method, "env": "pro", "result": result }).to_string()
    };
    match method.as_str() {
        "registration" => Some(ok(serde_json::json!({ "mac": spec.mac, "success": true }))),
        "getSystemConfig" => Some(ok(serde_json::json!({
            "mac": spec.mac,
            "homeId": 653_906,
            "roomId": 989_983,
            "moduleName": spec.module_name,
            "fwVersion": spec.firmware,
            "groupId": 0,
            "ping": 0,
        }))),
        "getModelConfig" => Some(match spec.min_dim_level {
            Some(level) => ok(serde_json::json!({ "minDimLevel": level, "nowc": 1 })),
            None => error(-32601, "Method not found"),
        }),
        "setPilot" => {
            let params = request.get("params").cloned().unwrap_or_default();
            let accepted = match spec.signing_key {
                None => true,
                Some(key) => signature_valid(raw, &request, &key),
            };
            state
                .lock()
                .expect("state lock")
                .pilots
                .push(PilotRecord { params, accepted });
            Some(if accepted {
                ok(serde_json::json!({ "success": true }))
            } else if request.get("hmac").is_none() {
                error(-32602, "Invalid params")
            } else {
                error(-32605, "Security error")
            })
        }
        _ => Some(error(-32601, "Method not found")),
    }
}

/// Check the HMAC over the exact `params` bytes in the raw request.
fn signature_valid(raw: &str, request: &serde_json::Value, key: &[u8; 16]) -> bool {
    let Some(hmac) = request.get("hmac").and_then(serde_json::Value::as_str) else {
        return false;
    };
    if request.get("env").and_then(serde_json::Value::as_str) != Some("pro") {
        return false;
    }
    let Some(start) = raw
        .find("\"params\":")
        .map(|index| index + "\"params\":".len())
    else {
        return false;
    };
    let Some(end) = raw[start..].find('}').map(|index| start + index + 1) else {
        return false;
    };
    let params = &raw[start..end];
    if request["params"].get("sigTs").is_none() {
        return false;
    }
    STANDARD.encode(reference_hmac(key, params.as_bytes())) == hmac
}

fn reference_hmac(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut padded = [0_u8; 64];
    padded[..key.len()].copy_from_slice(key);
    let inner_key: Vec<u8> = padded.iter().map(|byte| byte ^ 0x36).collect();
    let outer_key: Vec<u8> = padded.iter().map(|byte| byte ^ 0x5c).collect();
    let inner = Sha256::digest([inner_key.as_slice(), message].concat());
    let outer = Sha256::digest([outer_key.as_slice(), inner.as_slice()].concat());
    let mut digest = [0_u8; 32];
    digest.copy_from_slice(outer.as_slice());
    digest
}
