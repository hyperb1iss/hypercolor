//! The names a request may call the daemon by when it carries no
//! credential.
//!
//! A DNS-rebinding page reaches the daemon under the attacker's own name:
//! the browser resolves `rebind.example` to `127.0.0.1` (or the daemon's
//! LAN address) and treats the daemon as that page's origin, so `Origin`
//! and Fetch Metadata both look same-origin. Only the `Host` header still
//! carries the attacker's name. A request that relies on ambient
//! authority, loopback trust or a keyless network listener, must
//! therefore name the daemon by something an attacker cannot point at
//! it: an IP literal, `localhost`, one of the machine's own names, or a
//! name the operator listed.
//!
//! A request that presents a credential carries its own authority and is
//! never checked here.

use std::collections::HashSet;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};

use axum::http::Uri;
use axum::http::uri::Authority;
use tracing::warn;

use hypercolor_types::config::HypercolorConfig;
use hypercolor_types::server::ServerIdentity;

/// Refused names logged per process, so a rebinding page that retries in
/// a loop cannot flood the log.
const MAX_LOGGED_REFUSALS: usize = 32;

/// The `network.allowed_hosts` entry that turns the check off.
const ANY_HOST: &str = "*";

#[derive(Clone, Default)]
pub(crate) struct HostAllowlist {
    names: Arc<[String]>,
    any_host: bool,
    logged_refusals: Arc<Mutex<HashSet<String>>>,
}

impl HostAllowlist {
    /// An allowlist of `names` on top of the names every daemon answers
    /// to. `*` admits any host.
    pub(crate) fn new(names: impl IntoIterator<Item = String>) -> Self {
        let mut any_host = false;
        let mut normalized = Vec::new();
        for name in names {
            let name = name.trim();
            if name == ANY_HOST {
                any_host = true;
            } else if let Some(host) = authority_host(name) {
                normalized.push(host);
            }
        }
        normalized.sort_unstable();
        normalized.dedup();
        Self {
            names: normalized.into(),
            any_host,
            logged_refusals: Arc::default(),
        }
    }

    /// The allowlist a serving daemon enforces: `network.allowed_hosts`,
    /// the hosts of `web.cors_origins`, the machine's host names with
    /// their `.local` forms, and the name the daemon advertises over mDNS.
    pub(crate) fn for_daemon(
        config: &HypercolorConfig,
        identity: &ServerIdentity,
        system_host_names: impl IntoIterator<Item = String>,
    ) -> Self {
        let mut names = Vec::new();
        for entry in &config.network.allowed_hosts {
            let entry = entry.trim();
            if entry.is_empty() {
                continue;
            }
            if entry != ANY_HOST && authority_host(entry).is_none() {
                warn!(entry, "Ignoring invalid network.allowed_hosts entry");
                continue;
            }
            names.push(entry.to_owned());
        }
        names.extend(config.web.cors_origins.iter().filter_map(|origin| {
            origin
                .trim()
                .parse::<Uri>()
                .ok()
                .and_then(|uri| uri.host().map(str::to_owned))
        }));
        for name in system_host_names {
            let label = name.split('.').next().unwrap_or(&name).to_owned();
            names.push(format!("{label}.local"));
            names.push(label);
            names.push(name);
        }
        names.push(crate::mdns::advertised_host_name(identity));
        Self::new(names)
    }

    /// Whether `authority`, a `Host` header or a request URI's authority,
    /// names the daemon by an admitted name.
    pub(crate) fn admits(&self, authority: &str) -> bool {
        if self.any_host {
            return true;
        }
        let Some(host) = authority_host(authority) else {
            return false;
        };
        host.parse::<IpAddr>().is_ok()
            || host == "localhost"
            || host.ends_with(".localhost")
            || self.names.contains(&host)
    }

    /// Log the first refusals of each distinct name, then stay quiet.
    pub(crate) fn log_refusal(&self, host: &str) {
        let Ok(mut logged) = self.logged_refusals.lock() else {
            return;
        };
        if logged.len() < MAX_LOGGED_REFUSALS && logged.insert(host.to_owned()) {
            warn!(
                host,
                "Refused a request without an API key that named the daemon by a foreign host; \
                 add the name to network.allowed_hosts if it is yours"
            );
        }
    }
}

/// The host of a `Host` header or URI authority: lowercase, without its
/// port, IPv6 brackets, userinfo-free, and with one trailing dot removed.
fn authority_host(raw: &str) -> Option<String> {
    let authority = raw.trim().parse::<Authority>().ok()?;
    if authority.as_str().contains('@') {
        return None;
    }
    let host = authority.host();
    let host = host
        .strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
        .unwrap_or(host);
    let host = host.strip_suffix('.').unwrap_or(host);
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use hypercolor_types::config::HypercolorConfig;
    use hypercolor_types::server::ServerIdentity;

    use super::{HostAllowlist, authority_host};

    #[test]
    fn a_daemon_answers_to_its_configured_and_own_names() {
        let mut config = HypercolorConfig::default();
        config.network.allowed_hosts = vec![
            "hypercolor.home".to_owned(),
            "not a host".to_owned(),
            String::new(),
        ];
        config.web.cors_origins = vec!["https://lights.example:8443".to_owned()];
        let identity = ServerIdentity {
            instance_id: "0192abcd-0000-7000-8000-000000000000".to_owned(),
            instance_name: "Studio Rig".to_owned(),
            version: "0.7.0-test".to_owned(),
        };
        let hosts = HostAllowlist::for_daemon(&config, &identity, ["mybox.lan".to_owned()]);

        for admitted in [
            "hypercolor.home:9420",
            "lights.example",
            "mybox",
            "mybox.lan",
            "mybox.local:9420",
            "studio-rig-0192abcd.local",
        ] {
            assert!(hosts.admits(admitted), "{admitted}");
        }
        assert!(!hosts.admits("rebind.example"));
    }

    #[test]
    fn hosts_normalize_ports_brackets_case_and_trailing_dots() {
        assert_eq!(
            authority_host("LocalHost:9420").as_deref(),
            Some("localhost")
        );
        assert_eq!(authority_host("[::1]:9420").as_deref(), Some("::1"));
        assert_eq!(
            authority_host("mybox.local.").as_deref(),
            Some("mybox.local")
        );
        assert_eq!(
            authority_host(" 192.168.1.10 ").as_deref(),
            Some("192.168.1.10")
        );
        for invalid in ["", "user@localhost", "local host", "a/b"] {
            assert_eq!(authority_host(invalid), None, "{invalid:?}");
        }
    }

    #[test]
    fn literals_and_localhost_names_need_no_configuration() {
        let hosts = HostAllowlist::default();
        for admitted in [
            "127.0.0.1:9420",
            "[::1]:9420",
            "192.168.1.10:9420",
            "[fd00::5]",
            "localhost:9420",
            "LOCALHOST",
            "app.localhost:9430",
            "localhost.",
        ] {
            assert!(hosts.admits(admitted), "{admitted}");
        }
        for refused in [
            "rebind.example:9420",
            "localhost.rebind.example",
            "127.0.0.1.rebind.example",
            "mybox.local",
            "",
        ] {
            assert!(!hosts.admits(refused), "{refused}");
        }
    }

    #[test]
    fn configured_names_are_matched_exactly_and_star_admits_any() {
        let hosts = HostAllowlist::new(["Hypercolor.Home:9420".to_owned()]);
        assert!(hosts.admits("hypercolor.home"));
        assert!(hosts.admits("HYPERCOLOR.HOME:80"));
        assert!(!hosts.admits("evil.hypercolor.home"));

        let open = HostAllowlist::new(["*".to_owned()]);
        assert!(open.admits("rebind.example"));
    }
}
