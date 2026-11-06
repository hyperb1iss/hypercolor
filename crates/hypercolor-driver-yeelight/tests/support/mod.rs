//! In-repo fake Yeelight device bound to 127.0.0.1.
//!
//! One fake answers the search on a UDP port, accepts control connections
//! on a TCP port, answers every command, and on `set_music` connects back
//! to the requested host and port and records what arrives there. Replies
//! are written by hand rather than with the crate's encoders.

#![allow(dead_code, reason = "each test binary uses a different subset")]

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::task::JoinHandle;

pub type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub const COLOR_SUPPORT: &str = "get_prop set_default set_power toggle set_bright start_cf stop_cf set_scene cron_add cron_get cron_del set_ct_abx set_rgb set_hsv set_adjust adjust_bright adjust_ct adjust_color set_music set_name";

/// What the fake reports and how it behaves.
#[derive(Debug, Clone)]
pub struct FakeSpec {
    pub id: String,
    pub model: String,
    pub support: String,
    pub name: String,
    /// Answer `client quota exceeded` to this many commands after the
    /// first `quota_after` succeed.
    pub quota_rejections: usize,
    pub quota_after: usize,
    /// Close the control connection after serving this many commands.
    pub close_after: Option<usize>,
    /// Answer `set_music` but never connect back.
    pub music_never_connects: bool,
    /// Before connecting back for music mode, connect from this other
    /// loopback address, as a rogue LAN host racing the light would.
    pub rogue_from: Option<Ipv4Addr>,
    /// Advertise this IP in `Location` instead of the fake's own.
    pub location_ip: Option<Ipv4Addr>,
}

impl FakeSpec {
    pub fn color_bulb() -> Self {
        Self {
            id: "0x000000000015243f".to_owned(),
            model: "color".to_owned(),
            support: COLOR_SUPPORT.to_owned(),
            name: String::new(),
            quota_rejections: 0,
            quota_after: 0,
            close_after: None,
            music_never_connects: false,
            rogue_from: None,
            location_ip: None,
        }
    }
}

#[derive(Debug, Default)]
struct State {
    searches: Vec<String>,
    control_lines: Vec<serde_json::Value>,
    music_lines: Vec<serde_json::Value>,
    control_connections: usize,
    closed_connections: usize,
    music_closed: usize,
    rogue_lines: usize,
}

/// A running fake device.
pub struct FakeYeelight {
    search_addr: SocketAddr,
    control_addr: SocketAddr,
    state: Arc<Mutex<State>>,
    tasks: Arc<Mutex<Vec<JoinHandle<()>>>>,
}

impl Drop for FakeYeelight {
    fn drop(&mut self) {
        for task in self.tasks.lock().expect("tasks lock").iter() {
            task.abort();
        }
    }
}

impl FakeYeelight {
    pub async fn spawn(spec: FakeSpec) -> TestResult<Self> {
        let search = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let control = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let search_addr = search.local_addr()?;
        let control_addr = control.local_addr()?;
        let state = Arc::new(Mutex::new(State::default()));
        let tasks = Arc::new(Mutex::new(Vec::new()));

        let search_state = Arc::clone(&state);
        let search_spec = spec.clone();
        let search_task = tokio::spawn(async move {
            let mut buffer = vec![0_u8; 1024];
            while let Ok((len, source)) = search.recv_from(&mut buffer).await {
                let request = String::from_utf8_lossy(&buffer[..len]).to_string();
                search_state
                    .lock()
                    .expect("state lock")
                    .searches
                    .push(request.clone());
                if !request.starts_with("M-SEARCH * HTTP/1.1\r\n")
                    || !request.contains("ST: wifi_bulb\r\n")
                    || !request.contains("MAN: \"ssdp:discover\"\r\n")
                {
                    continue;
                }
                let location = search_spec.location_ip.map_or(control_addr, |ip| {
                    SocketAddr::from((ip, control_addr.port()))
                });
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nCache-Control: max-age=3600\r\nDate:\r\nExt:\r\nLocation: yeelight://{location}\r\nServer: POSIX UPnP/1.0 YGLC/1\r\nid: {}\r\nmodel: {}\r\nfw_ver: 18\r\nsupport: {}\r\npower: on\r\nbright: 100\r\ncolor_mode: 2\r\nct: 4000\r\nrgb: 16711680\r\nhue: 100\r\nsat: 35\r\nname: {}\r\n",
                    search_spec.id, search_spec.model, search_spec.support, search_spec.name
                );
                let _ = search.send_to(reply.as_bytes(), source).await;
            }
        });

        let control_state = Arc::clone(&state);
        let control_tasks = Arc::clone(&tasks);
        let control_task = tokio::spawn(async move {
            while let Ok((stream, _)) = control.accept().await {
                control_state
                    .lock()
                    .expect("state lock")
                    .control_connections += 1;
                let state = Arc::clone(&control_state);
                let spec = spec.clone();
                let tasks = Arc::clone(&control_tasks);
                let handle = tokio::spawn(async move {
                    serve_control(stream, &spec, &state, &tasks).await;
                });
                control_tasks.lock().expect("tasks lock").push(handle);
            }
        });
        tasks
            .lock()
            .expect("tasks lock")
            .extend([search_task, control_task]);
        Ok(Self {
            search_addr,
            control_addr,
            state,
            tasks,
        })
    }

    pub const fn search_addr(&self) -> SocketAddr {
        self.search_addr
    }

    pub const fn control_addr(&self) -> SocketAddr {
        self.control_addr
    }

    pub fn searches(&self) -> Vec<String> {
        self.state.lock().expect("state lock").searches.clone()
    }

    pub fn control_lines(&self) -> Vec<serde_json::Value> {
        self.state.lock().expect("state lock").control_lines.clone()
    }

    pub fn music_lines(&self) -> Vec<serde_json::Value> {
        self.state.lock().expect("state lock").music_lines.clone()
    }

    pub fn control_connections(&self) -> usize {
        self.state.lock().expect("state lock").control_connections
    }

    /// Lines that reached the rogue connection.
    pub fn rogue_lines(&self) -> usize {
        self.state.lock().expect("state lock").rogue_lines
    }

    /// Control and music connections the peer has seen close.
    pub fn closed_connections(&self) -> (usize, usize) {
        let state = self.state.lock().expect("state lock");
        (state.closed_connections, state.music_closed)
    }

    pub async fn wait_for_music(&self, count: usize) -> Vec<serde_json::Value> {
        for _ in 0..200 {
            let lines = self.music_lines();
            if lines.len() >= count {
                return lines;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        self.music_lines()
    }

    pub async fn wait_for_control(&self, count: usize) -> Vec<serde_json::Value> {
        for _ in 0..200 {
            let lines = self.control_lines();
            if lines.len() >= count {
                return lines;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        self.control_lines()
    }
}

async fn serve_control(
    stream: TcpStream,
    spec: &FakeSpec,
    state: &Arc<Mutex<State>>,
    tasks: &Mutex<Vec<JoinHandle<()>>>,
) {
    serve_commands(stream, spec, state, tasks).await;
    state.lock().expect("state lock").closed_connections += 1;
}

async fn serve_commands(
    stream: TcpStream,
    spec: &FakeSpec,
    state: &Arc<Mutex<State>>,
    tasks: &Mutex<Vec<JoinHandle<()>>>,
) {
    let (read_half, mut write_half) = stream.into_split();
    let mut lines = BufReader::new(read_half).lines();
    let mut served = 0_usize;
    while let Ok(Some(line)) = lines.next_line().await {
        let Ok(command) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        state
            .lock()
            .expect("state lock")
            .control_lines
            .push(command.clone());
        served += 1;
        let id = command["id"].clone();
        let rejected =
            served > spec.quota_after && served <= spec.quota_after + spec.quota_rejections;
        let reply = if rejected {
            serde_json::json!({ "id": id, "error": { "code": -1, "message": "client quota exceeded" } })
        } else {
            serde_json::json!({ "id": id, "result": ["ok"] })
        };
        if write_half
            .write_all(format!("{reply}\r\n").as_bytes())
            .await
            .is_err()
        {
            return;
        }
        if spec.close_after.is_some_and(|limit| served >= limit) {
            return;
        }
        let params = &command["params"];
        if command["method"] == "set_music" && params[0] == 1 && !spec.music_never_connects {
            let host = params[1].as_str().unwrap_or_default().to_owned();
            let port = params[2].as_u64().unwrap_or_default();
            if let Some(rogue_ip) = spec.rogue_from
                && let Ok(target) = format!("{host}:{port}").parse::<SocketAddr>()
                && let Ok(socket) = tokio::net::TcpSocket::new_v4()
                && socket.bind(SocketAddr::from((rogue_ip, 0))).is_ok()
                && let Ok(rogue) = socket.connect(target).await
            {
                let rogue_state = Arc::clone(state);
                let reader = tokio::spawn(async move {
                    let mut rogue_lines = BufReader::new(rogue).lines();
                    while let Ok(Some(_)) = rogue_lines.next_line().await {
                        rogue_state.lock().expect("state lock").rogue_lines += 1;
                    }
                });
                tasks.lock().expect("tasks lock").push(reader);
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            let Ok(music) = TcpStream::connect(format!("{host}:{port}")).await else {
                continue;
            };
            let music_state = Arc::clone(state);
            let reader = tokio::spawn(async move {
                // Music mode sends no replies; keep the write half alive.
                let (music_read, _music_write) = music.into_split();
                let mut music_lines = BufReader::new(music_read).lines();
                while let Ok(Some(line)) = music_lines.next_line().await {
                    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) {
                        music_state
                            .lock()
                            .expect("state lock")
                            .music_lines
                            .push(value);
                    }
                }
                music_state.lock().expect("state lock").music_closed += 1;
            });
            tasks.lock().expect("tasks lock").push(reader);
        }
    }
}
