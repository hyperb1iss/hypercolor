//! Qualification daemon for the Linux release installer guest proofs.
//!
//! It stands in for `bin/hypercolor-daemon` inside a release unit and speaks
//! exactly the contract the installer proves: `Type=notify` readiness, the
//! systemd watchdog, `/health` and `/api/v1/system` identity on
//! 127.0.0.1:9420, and a graceful stop on SIGTERM. Its version is the
//! `version` of the unit's own `manifest.json`, found through
//! `/proc/self/exe`, so every qualification release reports itself.
//!
//! Faults are read once per start from `$HOME/hc-qual/faults/<version>`,
//! one `key=value` per line:
//!
//! - `ready_delay_ms`: wait before serving HTTP and sending `READY=1`.
//! - `exit_before_ready`: exit with this status after the ready delay,
//!   without ever sending `READY=1`.
//! - `health_delay_ms`: wait before answering each `/health` request.
//! - `stop_delay_ms`: wait after SIGTERM before exiting.
//! - `crash_after_ready_ms`: abort this long after `READY=1`.
//! - `hang_http_after_ready_ms`: stop answering HTTP this long after
//!   `READY=1` while the watchdog keeps pinging.
//! - `report_version`: answer HTTP with this version instead.
//! - `probe_writes`: at start, try to write a file into each directory the
//!   generated service's sandbox allows or denies, and report the results.
//!
//! `GET /qual/launch` reports how this process was started: its resolved
//! executable, its arguments, the XDG variables its unit set, and the
//! write probes. A probe leaves nothing behind except
//! `/tmp/hc-qual-private-<pid>`, which shows whether `/tmp` is private.
//!
//! Built by `guest-proof.sh` with plain `rustc` in the baseline builder; it
//! uses only the standard library plus libc's `signal`.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::{BufRead as _, BufReader, Write as _};
use std::net::{TcpListener, TcpStream};
use std::os::linux::net::SocketAddrExt as _;
use std::os::unix::net::{SocketAddr, UnixDatagram};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const LISTEN: &str = "127.0.0.1:9420";
const SIGTERM: i32 = 15;

static TERMINATE: AtomicBool = AtomicBool::new(false);

unsafe extern "C" {
    fn signal(signum: i32, handler: extern "C" fn(i32)) -> usize;
}

extern "C" fn on_terminate(_signum: i32) {
    TERMINATE.store(true, Ordering::SeqCst);
}

#[derive(Default)]
struct Faults {
    values: HashMap<String, String>,
}

impl Faults {
    fn load(version: &str) -> Self {
        let Some(home) = std::env::var_os("HOME") else {
            return Self::default();
        };
        let path = Path::new(&home).join("hc-qual/faults").join(version);
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        let values = text
            .lines()
            .filter_map(|line| line.split_once('='))
            .map(|(key, value)| (key.trim().to_owned(), value.trim().to_owned()))
            .collect();
        Self { values }
    }

    fn millis(&self, key: &str) -> Option<Duration> {
        self.values
            .get(key)
            .and_then(|value| value.parse::<u64>().ok())
            .map(Duration::from_millis)
    }

    fn text(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }

    fn describe(&self) -> String {
        let mut pairs: Vec<_> = self
            .values
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect();
        pairs.sort();
        if pairs.is_empty() {
            "none".to_owned()
        } else {
            pairs.join(",")
        }
    }
}

fn log(message: &str) {
    eprintln!("hc-qual-daemon[{}]: {message}", std::process::id());
}

fn unit_version() -> String {
    let exe = std::fs::read_link("/proc/self/exe").unwrap_or_else(|_| PathBuf::from("/"));
    let manifest = exe
        .parent()
        .and_then(Path::parent)
        .map(|root| root.join("manifest.json"));
    manifest
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| manifest_version(&text))
        .unwrap_or_else(|| "unknown".to_owned())
}

/// The top-level `"version"` string of a release manifest.
///
/// A small scanner tracks strings and nesting, so only a key of the
/// outermost object counts, whatever order the keys come in.
fn manifest_version(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut depth = 0_usize;
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'{' | b'[' => depth += 1,
            b'}' | b']' => depth = depth.saturating_sub(1),
            b'"' => {
                let (string, next) = json_string(bytes, index)?;
                index = next;
                if depth == 1 && string == "version" {
                    let rest = text[index..].trim_start();
                    if let Some(value) = rest.strip_prefix(':') {
                        let value = value.trim_start();
                        if value.starts_with('"') {
                            let start = text.len() - value.len();
                            return json_string(bytes, start).map(|(version, _)| version);
                        }
                    }
                }
                continue;
            }
            _ => {}
        }
        index += 1;
    }
    None
}

/// The JSON string whose opening quote is at `start`, and the index just
/// past its closing quote. An escape keeps the escaped byte verbatim, which
/// is exact for the ASCII identities a manifest carries.
fn json_string(bytes: &[u8], start: usize) -> Option<(String, usize)> {
    let mut index = start + 1;
    let mut value = String::new();
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => {
                value.push(char::from(*bytes.get(index + 1)?));
                index += 2;
            }
            b'"' => return Some((value, index + 1)),
            byte => {
                value.push(char::from(byte));
                index += 1;
            }
        }
    }
    None
}

fn notify(state: &str) {
    let Some(target) = std::env::var_os("NOTIFY_SOCKET") else {
        return;
    };
    let Ok(socket) = UnixDatagram::unbound() else {
        return;
    };
    let target = target.to_string_lossy().into_owned();
    let sent = if let Some(name) = target.strip_prefix('@') {
        SocketAddr::from_abstract_name(name.as_bytes())
            .and_then(|address| socket.send_to_addr(state.as_bytes(), &address))
    } else {
        socket.send_to(state.as_bytes(), &target)
    };
    if let Err(error) = sent {
        log(&format!("sd_notify {state:?} failed: {error}"));
    }
}

fn start_watchdog() {
    let Some(interval) = std::env::var("WATCHDOG_USEC")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|usec| *usec > 0)
        .map(|usec| Duration::from_micros(usec / 2))
    else {
        return;
    };
    if std::env::var("WATCHDOG_PID")
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .is_some_and(|pid| pid != std::process::id())
    {
        return;
    }
    std::thread::spawn(move || {
        loop {
            notify("WATCHDOG=1");
            std::thread::sleep(interval);
        }
    });
}

struct Identity {
    version: String,
    instance_id: String,
    health_delay: Option<Duration>,
    hang_at: Option<Instant>,
    launch: String,
}

fn json_text(value: &str) -> String {
    let mut quoted = String::from("\"");
    for character in value.chars() {
        match character {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            character if character.is_control() => {
                let _ = write!(quoted, "\\u{:04x}", u32::from(character));
            }
            character => quoted.push(character),
        }
    }
    quoted.push('"');
    quoted
}

fn env_path(name: &str, fallback: impl FnOnce() -> PathBuf) -> PathBuf {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map_or_else(fallback, PathBuf::from)
}

/// Try to create and remove one file in `directory`.
fn probe_write(directory: &Path) -> String {
    let file = directory.join(format!(".hc-qual-probe-{}", std::process::id()));
    match std::fs::write(&file, b"probe") {
        Ok(()) => {
            let _ = std::fs::remove_file(&file);
            "ok".to_owned()
        }
        Err(error) => format!("denied: {:?}", error.kind()),
    }
}

/// How this process was started, and what its sandbox lets it write.
fn launch_report(probe: bool) -> String {
    let exe = std::fs::read_link("/proc/self/exe")
        .map_or_else(|_| "unknown".to_owned(), |path| path.display().to_string());
    let argv: Vec<String> = std::env::args().collect();
    let variables = [
        "HOME",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_STATE_HOME",
        "XDG_CACHE_HOME",
    ];
    let env = variables
        .iter()
        .map(|name| {
            let value = std::env::var(name).unwrap_or_default();
            format!("{}:{}", json_text(name), json_text(&value))
        })
        .collect::<Vec<_>>()
        .join(",");
    let mut writes = Vec::new();
    if probe {
        let home = env_path("HOME", || PathBuf::from("/"));
        let config = env_path("XDG_CONFIG_HOME", || home.join(".config")).join("hypercolor");
        let data = env_path("XDG_DATA_HOME", || home.join(".local/share")).join("hypercolor");
        let state = env_path("XDG_STATE_HOME", || home.join(".local/state")).join("hypercolor");
        let cache = env_path("XDG_CACHE_HOME", || home.join(".cache")).join("hypercolor");
        let _ = std::fs::create_dir_all(&cache);
        let private = PathBuf::from(format!("/tmp/hc-qual-private-{}", std::process::id()));
        let _ = std::fs::write(&private, b"private");
        let targets: [(&str, PathBuf); 14] = [
            ("config", config),
            ("data", data.clone()),
            ("daemon_state", state.clone()),
            ("coordinator", state.join("update/coordinator")),
            ("cache", cache),
            ("tmp", PathBuf::from("/tmp")),
            ("releases", data.join("releases")),
            ("update_state", state.join("update")),
            ("activator", state.join("update/activator")),
            ("local_bin", home.join(".local/bin")),
            ("legacy_lib", home.join(".local/lib/hypercolor")),
            ("user_units", home.join(".config/systemd/user")),
            ("home", home.clone()),
            ("var_tmp", PathBuf::from("/var/tmp")),
        ];
        for (label, directory) in targets {
            writes.push(format!(
                "{}:{}",
                json_text(label),
                json_text(&probe_write(&directory))
            ));
        }
    }
    format!(
        "{{\"exe\":{},\"argv\":[{}],\"env\":{{{env}}},\"writes\":{{{}}}}}",
        json_text(&exe),
        argv.iter()
            .map(|argument| json_text(argument))
            .collect::<Vec<_>>()
            .join(","),
        writes.join(",")
    )
}

fn serve(listener: &TcpListener, identity: &Arc<Identity>) {
    for stream in listener.incoming().flatten() {
        let identity = Arc::clone(identity);
        std::thread::spawn(move || answer(stream, &identity));
    }
}

fn answer(mut stream: TcpStream, identity: &Identity) {
    let mut request_line = String::new();
    {
        let mut reader = BufReader::new(&stream);
        if reader.read_line(&mut request_line).is_err() {
            return;
        }
        let mut header = String::new();
        while reader.read_line(&mut header).is_ok_and(|read| read > 2) {
            header.clear();
        }
    }
    if identity.hang_at.is_some_and(|at| Instant::now() >= at) {
        std::thread::sleep(Duration::from_hours(1));
        return;
    }
    let path = request_line.split_whitespace().nth(1).unwrap_or("/");
    let (status, body) = match path {
        "/health" => {
            if let Some(delay) = identity.health_delay {
                std::thread::sleep(delay);
            }
            (
                "200 OK",
                format!(
                    "{{\"status\":\"healthy\",\"version\":\"{}\"}}",
                    identity.version
                ),
            )
        }
        "/api/v1/system" => (
            "200 OK",
            format!(
                "{{\"data\":{{\"identity\":{{\"version\":\"{}\",\"instance_id\":\"{}\",\"instance_name\":\"hc-qual\"}}}}}}",
                identity.version, identity.instance_id
            ),
        ),
        "/qual/launch" => ("200 OK", identity.launch.clone()),
        _ => ("404 Not Found", "{}".to_owned()),
    };
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

fn main() {
    // SAFETY: the handler only stores to an atomic, which is async-signal-safe.
    unsafe {
        signal(SIGTERM, on_terminate);
    }
    let version = unit_version();
    let faults = Faults::load(&version);
    log(&format!(
        "start version={version} faults={}",
        faults.describe()
    ));

    if let Some(delay) = faults.millis("ready_delay_ms") {
        let deadline = Instant::now() + delay;
        while Instant::now() < deadline && !TERMINATE.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    if TERMINATE.load(Ordering::SeqCst) {
        log("terminated before ready");
        std::process::exit(0);
    }
    if let Some(status) = faults
        .text("exit_before_ready")
        .and_then(|value| value.parse::<i32>().ok())
    {
        log(&format!("exit before ready with status {status}"));
        std::process::exit(status);
    }

    let listener = match TcpListener::bind(LISTEN) {
        Ok(listener) => listener,
        Err(error) => {
            log(&format!("bind {LISTEN} failed: {error}"));
            std::process::exit(1);
        }
    };
    let ready_at = Instant::now();
    let identity = Arc::new(Identity {
        version: faults
            .text("report_version")
            .map_or_else(|| version.clone(), str::to_owned),
        instance_id: format!("hc-qual-{}", std::process::id()),
        health_delay: faults.millis("health_delay_ms"),
        hang_at: faults
            .millis("hang_http_after_ready_ms")
            .map(|delay| ready_at + delay),
        launch: launch_report(faults.text("probe_writes") == Some("1")),
    });
    std::thread::spawn(move || serve(&listener, &identity));
    start_watchdog();
    notify("READY=1");
    log("ready");

    let crash_at = faults
        .millis("crash_after_ready_ms")
        .map(|delay| ready_at + delay);
    while !TERMINATE.load(Ordering::SeqCst) {
        if crash_at.is_some_and(|at| Instant::now() >= at) {
            log("crash after ready");
            std::process::abort();
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    notify("STOPPING=1");
    log("stopping");
    if let Some(delay) = faults.millis("stop_delay_ms") {
        std::thread::sleep(delay);
    }
    log("stopped");
}

#[cfg(test)]
mod tests {
    use super::manifest_version;

    #[test]
    fn version_is_the_top_level_key_in_any_key_order() {
        let cases = [
            (
                r#"{"members":[{"path":"version","version":"x"}],"version":"1.2-qual.3"}"#,
                Some("1.2-qual.3"),
            ),
            (r#"{"version": "0.5.1", "name":"version"}"#, Some("0.5.1")),
            (
                r#"{"name":"version","assets":{"version":"no"},"version":"9"}"#,
                Some("9"),
            ),
            (r#"{"members":[]}"#, None),
        ];
        for (manifest, expected) in cases {
            assert_eq!(
                manifest_version(manifest).as_deref(),
                expected,
                "{manifest}"
            );
        }
    }
}
