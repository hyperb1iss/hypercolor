//! Which daemon the CLI talks to when flags, `HYPERCOLOR_*` variables, and a
//! cli.toml profile disagree.
//!
//! Each candidate target is a loopback port this test holds on 127.0.0.1 and,
//! where the host has IPv6 loopback, on ::1, for its whole run. The listeners
//! count connections and close them without answering, so the CLI fails fast,
//! names the URL it tried, and leaves a count that proves which socket it
//! actually dialed. No target is the default port: a live daemon may be
//! listening there.

use std::io::ErrorKind;
use std::net::{Ipv6Addr, TcpListener};
use std::path::Path;
use std::process::{Command, Output};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Bypasses every proxy for hosts and for loopback IP literals, which a
/// bare wildcard does not match.
const NO_PROXY: &str = "*,127.0.0.1,::1";

/// A held loopback port that counts and hangs up on every connection.
struct Target {
    port: u16,
    connections: Arc<AtomicUsize>,
}

impl Target {
    /// Hold one port on both loopback families, so `localhost` resolving to
    /// ::1 first cannot reach some other listener.
    fn bind() -> Self {
        const ATTEMPTS: usize = 32;
        for _ in 0..ATTEMPTS {
            let ipv4 = TcpListener::bind(("127.0.0.1", 0)).expect("bind a target port");
            let port = ipv4.local_addr().expect("target address").port();
            let ipv6 = match TcpListener::bind((Ipv6Addr::LOCALHOST, port)) {
                Ok(listener) => Some(listener),
                // Someone else owns this port on ::1; Windows reports an
                // exclusively owned port as access denied.
                Err(error)
                    if error.kind() == ErrorKind::AddrInUse
                        || (cfg!(windows) && error.kind() == ErrorKind::PermissionDenied) =>
                {
                    continue;
                }
                // No IPv6 loopback here, so nothing can answer on it either.
                Err(error)
                    if matches!(
                        error.kind(),
                        ErrorKind::AddrNotAvailable | ErrorKind::Unsupported
                    ) =>
                {
                    None
                }
                Err(error) => panic!("cannot hold [::1]:{port}: {error}"),
            };
            let connections = Arc::new(AtomicUsize::new(0));
            for listener in std::iter::once(ipv4).chain(ipv6) {
                let counter = Arc::clone(&connections);
                std::thread::spawn(move || {
                    for stream in listener.incoming() {
                        if stream.is_ok() {
                            counter.fetch_add(1, Ordering::SeqCst);
                        }
                    }
                });
            }
            return Self { port, connections };
        }
        panic!("no port was free on both 127.0.0.1 and [::1] after {ATTEMPTS} attempts");
    }

    fn port(&self) -> String {
        self.port.to_string()
    }

    fn connections(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }
}

/// Run `effects list` with only the given flags, variables, and profile.
fn run_cli(root: &Path, profile: &Target, flags: &[&str], env: &[(&str, String)]) -> Output {
    let cli_config = root.join("cli.toml");
    std::fs::write(
        &cli_config,
        format!(
            "[profiles.local]\nhost = \"127.0.0.1\"\nport = {}\n",
            profile.port
        ),
    )
    .expect("write the CLI profile");

    let mut command = Command::new(env!("CARGO_BIN_EXE_hypercolor"));
    for (key, _) in std::env::vars_os() {
        let Some(key) = key.to_str() else { continue };
        if key.starts_with("HYPERCOLOR_") || key.to_ascii_uppercase().ends_with("_PROXY") {
            command.env_remove(key);
        }
    }
    command
        .env("HYPERCOLOR_CLI_CONFIG", &cli_config)
        // Also overrides a proxy the OS configures outside the environment;
        // the wildcard covers names and the literals cover loopback IPs.
        .env("NO_PROXY", NO_PROXY)
        .env("no_proxy", NO_PROXY)
        .env("NO_COLOR", "1")
        .args(flags)
        .args(["--json", "effects", "list"])
        .envs(env.iter().map(|(key, value)| (*key, value.as_str())));
    command.output().expect("run the CLI")
}

/// The URL the CLI tried, read from its failure.
fn attempted(output: &Output) -> String {
    assert!(
        !output.status.success(),
        "a target that hangs up must fail: {output:?}"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    let target = stderr
        .split("Failed to connect to daemon at ")
        .nth(1)
        .and_then(|rest| rest.split("/api/v1").next())
        .unwrap_or_else(|| panic!("no connection failure in stderr:\n{stderr}"))
        .to_owned();
    assert!(
        !target.ends_with(":9420"),
        "a test resolved to the default port: {target}"
    );
    target
}

/// Only `chosen` saw a connection.
fn assert_dialed(chosen: &Target, others: &[&Target]) {
    assert!(
        chosen.connections() > 0,
        "the chosen target was never dialed"
    );
    for other in others {
        assert_eq!(
            other.connections(),
            0,
            "port {} was dialed instead",
            other.port
        );
    }
}

#[test]
fn a_flag_equal_to_the_default_host_beats_the_environment_and_profile() {
    let root = tempfile::tempdir().expect("temp root");
    let (flag, env, profile) = (Target::bind(), Target::bind(), Target::bind());

    let output = run_cli(
        root.path(),
        &profile,
        &["--host", "localhost", "--port", &flag.port()],
        &[
            ("HYPERCOLOR_HOST", "127.0.0.1".to_owned()),
            ("HYPERCOLOR_PORT", env.port()),
        ],
    );

    assert_eq!(
        attempted(&output),
        format!("http://localhost:{}", flag.port)
    );
    assert_dialed(&flag, &[&env, &profile]);
}

#[test]
fn flags_beat_the_environment_and_profile() {
    let root = tempfile::tempdir().expect("temp root");
    let (flag, env, profile) = (Target::bind(), Target::bind(), Target::bind());

    let output = run_cli(
        root.path(),
        &profile,
        &["--host", "127.0.0.1", "--port", &flag.port()],
        &[
            ("HYPERCOLOR_HOST", "localhost".to_owned()),
            ("HYPERCOLOR_PORT", env.port()),
        ],
    );

    assert_eq!(
        attempted(&output),
        format!("http://127.0.0.1:{}", flag.port)
    );
    assert_dialed(&flag, &[&env, &profile]);
}

#[test]
fn the_environment_beats_the_profile() {
    let root = tempfile::tempdir().expect("temp root");
    let (env, profile) = (Target::bind(), Target::bind());

    let output = run_cli(
        root.path(),
        &profile,
        &[],
        &[
            ("HYPERCOLOR_HOST", "localhost".to_owned()),
            ("HYPERCOLOR_PORT", env.port()),
        ],
    );

    assert_eq!(attempted(&output), format!("http://localhost:{}", env.port));
    assert_dialed(&env, &[&profile]);
}

#[test]
fn the_profile_fills_what_nothing_names() {
    let root = tempfile::tempdir().expect("temp root");
    let profile = Target::bind();

    let output = run_cli(root.path(), &profile, &[], &[]);

    assert_eq!(
        attempted(&output),
        format!("http://127.0.0.1:{}", profile.port)
    );
    assert_dialed(&profile, &[]);
}
