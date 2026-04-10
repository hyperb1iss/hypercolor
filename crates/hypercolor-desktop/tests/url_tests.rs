//! Tests around the default daemon URL contract.
//!
//! The actual URL parsing lives inline in `main.rs` (it reads `HYPERCOLOR_URL`
//! and falls back to a default constant), so these tests validate the shape
//! of that default and the `url` crate's ability to parse it. When main.rs
//! grows a dedicated config module, these should be expanded to cover the
//! env-var override path directly.

const DEFAULT_DAEMON_URL: &str = "http://127.0.0.1:9420";

#[test]
fn default_daemon_url_parses() {
    let url: url::Url = DEFAULT_DAEMON_URL
        .parse()
        .expect("default daemon URL should be parseable");
    assert_eq!(url.scheme(), "http");
}

#[test]
fn default_daemon_url_targets_loopback() {
    let url: url::Url = DEFAULT_DAEMON_URL.parse().expect("parse default URL");
    assert_eq!(
        url.host_str(),
        Some("127.0.0.1"),
        "default must point at loopback so a stock desktop build never reaches the network"
    );
}

#[test]
fn default_daemon_url_uses_daemon_port() {
    let url: url::Url = DEFAULT_DAEMON_URL.parse().expect("parse default URL");
    assert_eq!(
        url.port(),
        Some(9420),
        "default port must match the daemon's listener"
    );
}

#[test]
fn arbitrary_http_url_is_accepted() {
    let url: url::Url = "http://hypercolor.lan:9420"
        .parse()
        .expect("hostnames should be valid HYPERCOLOR_URL values");
    assert_eq!(url.scheme(), "http");
    assert_eq!(url.host_str(), Some("hypercolor.lan"));
    assert_eq!(url.port(), Some(9420));
}

#[test]
fn https_url_is_accepted() {
    let url: url::Url = "https://hypercolor.example/"
        .parse()
        .expect("https URLs should parse for reverse-proxied deployments");
    assert_eq!(url.scheme(), "https");
}

#[test]
fn garbage_string_is_rejected() {
    let result: Result<url::Url, _> = "not a url".parse();
    assert!(
        result.is_err(),
        "parser must reject malformed HYPERCOLOR_URL values so main.rs can panic fast"
    );
}
