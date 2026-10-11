//! API authentication and rate limiting middleware.
//!
//! Security is enabled when API key environment variables are present:
//! - `HYPERCOLOR_API_KEY` (control tier)
//! - `HYPERCOLOR_READ_API_KEY` (read-only tier, optional)
//!
//! or when a downstream build installs a [`CredentialAuthority`], whose
//! credentials are resolved after the environment keys.
//!
//! Read-only keys can call GET/HEAD/OPTIONS endpoints. Mutating endpoints
//! require a control-tier key.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use axum::body::Body;
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderName, HeaderValue, Method, Request, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use if_addrs::IfAddr;
use serde_json::json;
use subtle::ConstantTimeEq;
use tokio::sync::Mutex;
use tracing::warn;

pub use super::credential_authority::{CredentialAuthority, CredentialGrant, CredentialTier};
use crate::domain::DomainError;
use crate::extensions::{ApiExtension, PublicRateClass};
use crate::macos_owner::MacosDaemonSessionAttestation;
use hypercolor_types::config::{
    HypercolorConfig, NetworkAccessMode, NetworkClientScope, NetworkConfig,
};
use hypercolor_types::service::{
    PROTECTED_CONTROL_CREDENTIAL_ENV, ProtectedControlCredential,
    ProtectedControlCredentialParseError,
};

const RATE_WINDOW: Duration = Duration::from_mins(1);
const READ_LIMIT_PER_MIN: u32 = 120;
const WRITE_LIMIT_PER_MIN: u32 = 60;
const DISCOVERY_LIMIT_PER_MIN: u32 = 2;
const PAIRING_LIMIT_PER_MIN: u32 = 6;

const TRUSTED_TAURI_ORIGINS: &[&str] = &[
    "tauri://localhost",
    "http://tauri.localhost",
    "https://tauri.localhost",
];

pub(crate) fn is_trusted_tauri_origin(origin: &HeaderValue) -> bool {
    origin.to_str().is_ok_and(|origin| {
        TRUSTED_TAURI_ORIGINS
            .iter()
            .any(|trusted| origin.eq_ignore_ascii_case(trusted))
    })
}

const HEADER_RATE_LIMIT_LIMIT: HeaderName = HeaderName::from_static("x-ratelimit-limit");
const HEADER_RATE_LIMIT_REMAINING: HeaderName = HeaderName::from_static("x-ratelimit-remaining");
const HEADER_RATE_LIMIT_RESET: HeaderName = HeaderName::from_static("x-ratelimit-reset");
const HEADER_RETRY_AFTER: HeaderName = HeaderName::from_static("retry-after");

#[derive(Clone)]
pub struct SecurityState {
    auth: AuthConfig,
    launcher_session_credential: Option<ProtectedControlCredential>,
    attested_session_credential: Option<ProtectedControlCredential>,
    authority: Option<Arc<dyn CredentialAuthority>>,
    network: NetworkAccessPolicy,
    rate_limiter: Arc<Mutex<RateLimiter>>,
    static_assets: StaticAssetSurface,
    public_routes: PublicRouteTable,
}

/// The paths the bundled web UI is served from.
///
/// The UI mounts as the router's fallback, so its surface is defined by
/// subtraction: every path no dynamic mount claims. Naming the dynamic
/// prefixes rather than the asset paths is what keeps this exemption
/// from ever widening onto an API, MCP, or health route, whatever files
/// the UI build happens to emit.
#[derive(Clone, Debug, Default)]
pub struct StaticAssetSurface {
    mounted: bool,
    dynamic_prefixes: Arc<[String]>,
}

/// The prefixes no static-asset surface may ever swallow.
///
/// Every dynamic route the daemon mounts lives under one of these or
/// under the MCP base path, which callers add. Seeding the list here
/// rather than trusting the caller means an incomplete argument narrows
/// the exemption, never widens it: the failure mode is an asset that
/// needs a key, not an API route that does not.
const ALWAYS_DYNAMIC_PREFIXES: [&str; 2] = ["/api", "/health"];

impl StaticAssetSurface {
    /// Declare a mounted UI directory sitting behind the given dynamic
    /// route prefixes.
    ///
    /// [`ALWAYS_DYNAMIC_PREFIXES`] is always included, so passing an
    /// empty list exempts static assets only, never the API.
    #[must_use]
    pub fn mounted(dynamic_prefixes: impl IntoIterator<Item = String>) -> Self {
        Self {
            mounted: true,
            dynamic_prefixes: ALWAYS_DYNAMIC_PREFIXES
                .iter()
                .map(|prefix| (*prefix).to_owned())
                .chain(dynamic_prefixes)
                .collect(),
        }
    }

    fn serves(&self, path: &str) -> bool {
        self.mounted
            && !self
                .dynamic_prefixes
                .iter()
                .any(|prefix| path_within(path, prefix))
    }
}

/// The security decision the middleware made for one request.
///
/// Every request that reaches a handler carries one as an extension.
/// Handlers mounted by downstream builds read it to apply the same tier
/// and protected-control rules the engine's own routes apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestAuthContext {
    security_enabled: bool,
    granted_tier: Option<AccessTier>,
    protected_control: ProtectedControl,
    locality: RequestLocality,
    presented_credential: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProtectedControl {
    Denied,
    Granted,
}

/// Where a request came from, as the middleware classified it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RequestLocality {
    /// A socket peer on another host, or a loopback proxy forwarding one.
    Network,
    /// A loopback socket peer with no forwarded client.
    Loopback,
    /// The in-process trusted transport.
    InProcess,
}

#[derive(Debug, Clone, Copy)]
struct TrustedLocalControl;

impl RequestAuthContext {
    #[must_use]
    pub(crate) const fn unsecured() -> Self {
        Self {
            security_enabled: false,
            granted_tier: None,
            protected_control: ProtectedControl::Denied,
            locality: RequestLocality::Network,
            presented_credential: false,
        }
    }

    #[must_use]
    pub(crate) const fn preflight() -> Self {
        Self {
            security_enabled: true,
            granted_tier: None,
            protected_control: ProtectedControl::Denied,
            locality: RequestLocality::Network,
            presented_credential: false,
        }
    }

    /// A caller on a public route: no tier, no protected control, and no
    /// loopback locality, whatever it presented and wherever it came from.
    #[must_use]
    const fn anonymous() -> Self {
        Self::preflight()
    }

    #[must_use]
    const fn authenticated(granted_tier: AccessTier) -> Self {
        Self {
            security_enabled: true,
            granted_tier: Some(granted_tier),
            protected_control: match granted_tier {
                AccessTier::Read => ProtectedControl::Denied,
                AccessTier::Control => ProtectedControl::Granted,
            },
            locality: RequestLocality::Network,
            presented_credential: true,
        }
    }

    /// A credential from an installed authority. Client credentials never
    /// carry protected control, whatever their tier.
    #[must_use]
    pub(crate) const fn authority_grant(tier: CredentialTier) -> Self {
        Self {
            security_enabled: true,
            granted_tier: Some(match tier {
                CredentialTier::Read => AccessTier::Read,
                CredentialTier::Control => AccessTier::Control,
            }),
            protected_control: ProtectedControl::Denied,
            locality: RequestLocality::Network,
            presented_credential: true,
        }
    }

    #[must_use]
    const fn daemon_session(security_enabled: bool) -> Self {
        Self {
            security_enabled,
            granted_tier: Some(AccessTier::Control),
            protected_control: ProtectedControl::Granted,
            locality: RequestLocality::Network,
            presented_credential: true,
        }
    }

    #[must_use]
    const fn with_locality(mut self, locality: RequestLocality) -> Self {
        self.locality = locality;
        self
    }

    #[must_use]
    #[cfg(test)]
    pub(crate) const fn read_only() -> Self {
        Self::authenticated(AccessTier::Read)
    }

    #[must_use]
    #[cfg(test)]
    pub(crate) const fn control() -> Self {
        Self::authenticated(AccessTier::Control)
    }

    #[must_use]
    pub(crate) const fn security_enabled(self) -> bool {
        self.security_enabled
    }

    /// Whether the caller may make mutating requests.
    #[must_use]
    pub const fn can_control(self) -> bool {
        !self.security_enabled || matches!(self.granted_tier, Some(AccessTier::Control))
    }

    /// Whether the caller holds an operator credential: the control
    /// environment key, the launcher session, or in-process trusted
    /// control. Authority credentials never do.
    #[must_use]
    pub const fn can_protected_control(self) -> bool {
        matches!(self.protected_control, ProtectedControl::Granted)
    }

    /// Whether the request came from a loopback peer.
    ///
    /// A loopback proxy that forwards a client address makes the request
    /// the forwarded client's, so this is the classification to trust,
    /// not the raw socket address. Always `false` on a public route, which
    /// any web page can reach through a loopback browser.
    #[must_use]
    pub const fn is_loopback(self) -> bool {
        matches!(self.locality, RequestLocality::Loopback)
    }

    /// Whether the request presented a credential that resolved.
    ///
    /// Such a request carries its own authority, so it has nothing
    /// ambient for a cross-origin page to ride.
    #[must_use]
    pub(crate) const fn presented_credential(self) -> bool {
        self.presented_credential
    }

    #[must_use]
    pub(crate) const fn can_read_system_status(self) -> bool {
        !self.security_enabled || self.granted_tier.is_some()
    }

    #[must_use]
    const fn granted_tier(self) -> Option<AccessTier> {
        self.granted_tier
    }
}

impl SecurityState {
    /// The posture of an application state that serves no router.
    ///
    /// Workers and one-shot projections build an `AppState` to reach the
    /// domain graph, never to answer a request. Handing them a state
    /// with no keys and no network policy keeps every enforcement
    /// decision with the one state a router was assembled from.
    #[must_use]
    pub(crate) fn unserved() -> Self {
        Self {
            auth: AuthConfig::default(),
            launcher_session_credential: None,
            attested_session_credential: None,
            authority: None,
            network: NetworkAccessPolicy::default(),
            rate_limiter: Arc::new(Mutex::new(RateLimiter::new())),
            static_assets: StaticAssetSurface::default(),
            public_routes: PublicRouteTable::default(),
        }
    }

    #[must_use]
    pub fn from_env() -> Self {
        if cfg!(test) {
            return Self::unserved();
        }

        let control_key = api_key_from_env("HYPERCOLOR_API_KEY");
        let read_key = api_key_from_env("HYPERCOLOR_READ_API_KEY");
        Self {
            auth: AuthConfig {
                control_key,
                read_key,
            },
            launcher_session_credential: protected_control_credential_from_env(),
            attested_session_credential: None,
            authority: None,
            network: NetworkAccessPolicy::default(),
            rate_limiter: Arc::new(Mutex::new(RateLimiter::new())),
            static_assets: StaticAssetSurface::default(),
            public_routes: PublicRouteTable::default(),
        }
    }

    #[must_use]
    pub fn from_config(config: &HypercolorConfig) -> Self {
        let mut state = Self::from_env();
        state.network = NetworkAccessPolicy::from_config(&config.network);
        state
    }

    /// Declare the static-asset surface this router serves.
    ///
    /// Called once at router assembly, where the mounted UI directory
    /// and every dynamic prefix are both known.
    #[must_use]
    pub fn with_static_assets(mut self, static_assets: StaticAssetSurface) -> Self {
        self.static_assets = static_assets;
        self
    }

    /// Install the downstream credential authority.
    ///
    /// Authentication turns on for every non-loopback request as soon as
    /// an authority is installed, whether or not it holds credentials yet:
    /// an empty authority admits nobody, which is what makes a network
    /// bind safe before the first credential exists.
    #[must_use]
    pub fn with_credential_authority(mut self, authority: Arc<dyn CredentialAuthority>) -> Self {
        self.authority = Some(authority);
        self
    }

    /// Declare the public routes this router serves.
    ///
    /// Called once at router assembly, next to the static-asset surface.
    #[must_use]
    pub(crate) fn with_public_routes(mut self, public_routes: PublicRouteTable) -> Self {
        self.public_routes = public_routes;
        self
    }

    pub(crate) fn security_enabled(&self) -> bool {
        self.auth.control_key.is_some() || self.auth.read_key.is_some() || self.authority.is_some()
    }

    /// The families in which a remote client can control this API without
    /// presenting a credential: none while any credential is required,
    /// otherwise every family the network policy admits a non-loopback
    /// client in. An allowlist that names only loopback, an invalid entry,
    /// or a client scope that cannot be resolved admits none.
    pub(crate) fn keyless_remote_client_families(&self) -> RemoteClientFamilies {
        if self.security_enabled() {
            RemoteClientFamilies::NONE
        } else {
            self.network.remote_client_families()
        }
    }

    pub(crate) fn install_macos_daemon_session(
        &mut self,
        attestation: &MacosDaemonSessionAttestation,
    ) {
        self.attested_session_credential = Some(attestation.protected_control_credential.clone());
    }

    fn is_session_credential(&self, token: &str) -> bool {
        [
            self.launcher_session_credential.as_ref(),
            self.attested_session_credential.as_ref(),
        ]
        .into_iter()
        .flatten()
        .any(|credential| secret_matches(Some(credential.expose_secret()), token))
    }

    fn resolve_loopback_token(&self, token: &str) -> Option<ResolvedCredential> {
        if self.is_session_credential(token) {
            Some(ResolvedCredential {
                context: RequestAuthContext::daemon_session(self.security_enabled()),
                grant: None,
            })
        } else {
            self.resolve_presented_token(token)
        }
    }

    /// Resolve a bearer against the environment keys, then the authority.
    ///
    /// Every source is consulted before any result is read, so the time a
    /// rejection takes does not report which source the caller came
    /// closest to matching.
    fn resolve_presented_token(&self, token: &str) -> Option<ResolvedCredential> {
        let environment = resolve_token_tier(token, &self.auth);
        let grant = self.authority.as_ref().and_then(|authority| {
            authority
                .authenticate(token)
                .filter(|grant| !grant.revocation().is_cancelled())
                .map(|grant| grant.clamped_to(authority.ceiling()))
        });

        match (environment, grant) {
            (Some(tier), _) => Some(ResolvedCredential {
                context: RequestAuthContext::authenticated(tier),
                grant: None,
            }),
            (None, Some(grant)) => Some(ResolvedCredential {
                context: RequestAuthContext::authority_grant(grant.tier()),
                grant: Some(grant),
            }),
            (None, None) => None,
        }
    }
}

/// A presented bearer that resolved, with the authority grant when one
/// issued it.
struct ResolvedCredential {
    context: RequestAuthContext,
    grant: Option<CredentialGrant>,
}

impl ResolvedCredential {
    fn attach(self, request: &mut Request<Body>, locality: RequestLocality) {
        request
            .extensions_mut()
            .insert(self.context.with_locality(locality));
        if let Some(grant) = self.grant {
            request.extensions_mut().insert(grant);
        }
    }
}

/// The public routes a router serves, resolved to full request paths.
#[derive(Clone, Default)]
pub(crate) struct PublicRouteTable {
    routes: Arc<[PublicRouteEntry]>,
    /// Full paths whose declaration was refused because they sit beneath
    /// the bearer-exempt docs paths. An extension route there would
    /// otherwise answer without a credential, so these paths lose the
    /// exemption and authenticate like any undeclared route.
    withheld_exemptions: Arc<[String]>,
}

#[derive(Debug, Clone)]
struct PublicRouteEntry {
    method: Method,
    path: String,
    class: OperationClass,
}

impl PublicRouteTable {
    /// Resolve every extension's declarations under `api_prefix`.
    ///
    /// `engine_routes` are the path templates the engine itself serves
    /// under the same prefix, and `reserved_prefixes` are full paths of
    /// engine mounts that sit outside its route table, such as the MCP
    /// service. The bearer-exempt API docs paths are always reserved, and
    /// a declaration refused beneath them also withdraws the exemption
    /// from its exact path, so that extension route authenticates instead
    /// of answering anyone.
    /// A declaration that names an engine route, falls within a reserved
    /// prefix, or is not an exact path is dropped with an error, and its
    /// route stays authenticated. Extensions are trusted not to declare one
    /// another's routes; the engine cannot tell them apart.
    pub(crate) fn from_extensions<'a>(
        extensions: impl IntoIterator<Item = &'a Arc<dyn ApiExtension>>,
        api_prefix: &str,
        engine_routes: &[String],
        reserved_prefixes: &[String],
    ) -> Self {
        let mut routes = Vec::new();
        let mut withheld_exemptions = Vec::new();
        for extension in extensions {
            for route in extension.public_routes() {
                if let Err(reason) = validate_public_route(
                    route.path(),
                    api_prefix,
                    engine_routes,
                    reserved_prefixes,
                ) {
                    tracing::error!(
                        extension = extension.name(),
                        method = %route.method(),
                        path = route.path(),
                        reason,
                        "Ignoring public route declaration; the route stays authenticated"
                    );
                    let full_path = format!("{api_prefix}{}", route.path());
                    if is_docs_exempt_path(&full_path) {
                        withheld_exemptions.push(full_path);
                    }
                    continue;
                }
                routes.push(PublicRouteEntry {
                    method: route.method().clone(),
                    path: format!("{api_prefix}{}", route.path()),
                    class: match route.class() {
                        PublicRateClass::Read => OperationClass::Read,
                        PublicRateClass::Write => OperationClass::Write,
                        PublicRateClass::Pairing => OperationClass::Pairing,
                    },
                });
            }
        }
        Self {
            routes: routes.into(),
            withheld_exemptions: withheld_exemptions.into(),
        }
    }

    fn withholds_exemption(&self, path: &str) -> bool {
        self.withheld_exemptions
            .iter()
            .any(|withheld| withheld == path)
    }

    fn class_for(&self, method: &Method, path: &str) -> Option<OperationClass> {
        self.routes
            .iter()
            .find(|route| {
                route.path == path
                    && (route.method == *method
                        || (route.method == Method::GET && *method == Method::HEAD))
            })
            .map(|route| route.class)
    }
}

fn validate_public_route(
    path: &str,
    api_prefix: &str,
    engine_routes: &[String],
    reserved_prefixes: &[String],
) -> Result<(), &'static str> {
    let Some(rest) = path.strip_prefix('/') else {
        return Err("path must start with '/'");
    };
    if rest.is_empty()
        || rest
            .split('/')
            .any(|segment| segment.is_empty() || segment.contains(['{', '}', '*', '?', '#']))
    {
        return Err("path must be exact, with no empty, parameter, or wildcard segments");
    }
    if engine_routes
        .iter()
        .any(|template| template_matches(template, path))
    {
        return Err("path names a route the engine serves");
    }
    let full_path = format!("{api_prefix}{path}");
    if is_docs_exempt_path(&full_path)
        || reserved_prefixes
            .iter()
            .any(|prefix| path_within(&full_path, prefix))
    {
        return Err("path falls within an engine mount");
    }
    Ok(())
}

/// Whether an OpenAPI path template (`/devices/{id}`) matches `path`.
fn template_matches(template: &str, path: &str) -> bool {
    let mut template_segments = template.trim_matches('/').split('/');
    let mut path_segments = path.trim_matches('/').split('/');
    loop {
        match (template_segments.next(), path_segments.next()) {
            (None, None) => return true,
            (Some(expected), Some(actual))
                if expected == actual || (expected.starts_with('{') && expected.ends_with('}')) => {
            }
            _ => return false,
        }
    }
}

#[must_use]
pub fn api_auth_required_from_env() -> bool {
    let control_key = api_key_from_env("HYPERCOLOR_API_KEY");
    let read_key = api_key_from_env("HYPERCOLOR_READ_API_KEY");
    control_key.is_some() || read_key.is_some()
}

#[must_use]
pub fn control_api_key_configured_from_env() -> bool {
    api_key_from_env("HYPERCOLOR_API_KEY").is_some()
}

/// The address families in which a network policy admits some client other
/// than loopback, before authentication runs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct RemoteClientFamilies {
    pub(crate) ipv4: bool,
    pub(crate) ipv6: bool,
}

impl RemoteClientFamilies {
    /// No remote client is admitted in either family.
    pub(crate) const NONE: Self = Self {
        ipv4: false,
        ipv6: false,
    };

    /// Whether a remote client of `address`'s family can be admitted. A
    /// listener only ever accepts clients of its own family, because IPv6
    /// listeners are bound IPv6-only.
    #[must_use]
    pub(crate) const fn admits_family_of(self, address: IpAddr) -> bool {
        match address {
            IpAddr::V4(_) => self.ipv4,
            IpAddr::V6(_) => self.ipv6,
        }
    }
}

fn api_key_from_env(name: &str) -> Option<String> {
    normalize_api_key(std::env::var(name).ok())
}

fn normalize_api_key(value: Option<String>) -> Option<String> {
    value.filter(|key| !key.trim().is_empty())
}

fn protected_control_credential_from_env() -> Option<ProtectedControlCredential> {
    if let Ok(credential) =
        parse_protected_control_credential(std::env::var(PROTECTED_CONTROL_CREDENTIAL_ENV).ok())
    {
        credential
    } else {
        warn!(
            environment = PROTECTED_CONTROL_CREDENTIAL_ENV,
            "Ignoring invalid protected-control credential"
        );
        None
    }
}

fn parse_protected_control_credential(
    value: Option<String>,
) -> Result<Option<ProtectedControlCredential>, ProtectedControlCredentialParseError> {
    value
        .map(|credential| ProtectedControlCredential::parse(&credential))
        .transpose()
}

#[cfg(test)]
impl SecurityState {
    pub(crate) fn with_keys(control_key: Option<&str>, read_key: Option<&str>) -> Self {
        Self {
            auth: AuthConfig {
                control_key: control_key.map(ToOwned::to_owned),
                read_key: read_key.map(ToOwned::to_owned),
            },
            launcher_session_credential: None,
            attested_session_credential: None,
            authority: None,
            network: NetworkAccessPolicy::default(),
            rate_limiter: Arc::new(Mutex::new(RateLimiter::new())),
            static_assets: StaticAssetSurface::default(),
            public_routes: PublicRouteTable::default(),
        }
    }

    pub(crate) fn with_network_config(network: NetworkConfig) -> Self {
        Self {
            auth: AuthConfig::default(),
            launcher_session_credential: None,
            attested_session_credential: None,
            authority: None,
            network: NetworkAccessPolicy::from_config(&network),
            rate_limiter: Arc::new(Mutex::new(RateLimiter::new())),
            static_assets: StaticAssetSurface::default(),
            public_routes: PublicRouteTable::default(),
        }
    }

    fn with_session_credential(credential: ProtectedControlCredential) -> Self {
        let mut state = Self::with_keys(None, None);
        state.launcher_session_credential = Some(credential);
        state
    }

    fn with_network_policy(network: NetworkAccessPolicy) -> Self {
        Self {
            auth: AuthConfig::default(),
            launcher_session_credential: None,
            attested_session_credential: None,
            authority: None,
            network,
            rate_limiter: Arc::new(Mutex::new(RateLimiter::new())),
            static_assets: StaticAssetSurface::default(),
            public_routes: PublicRouteTable::default(),
        }
    }
}

#[derive(Clone, Default)]
struct AuthConfig {
    control_key: Option<String>,
    read_key: Option<String>,
}

#[derive(Clone, Default)]
struct NetworkAccessPolicy {
    allowed_clients: Vec<ClientAddressRule>,
    invalid_rules: Vec<String>,
    scope_errors: Vec<String>,
}

impl NetworkAccessPolicy {
    fn from_config(config: &NetworkConfig) -> Self {
        Self::from_config_with_local_subnets(config, discover_local_subnet_rules())
    }

    fn from_config_with_local_subnets(
        config: &NetworkConfig,
        local_subnet_rules: Result<Vec<ClientAddressRule>, String>,
    ) -> Self {
        let mut allowed_clients = Vec::new();
        let mut invalid_rules = Vec::new();
        let mut scope_errors = Vec::new();

        if config.remote_access_enabled() && config.access_mode != NetworkAccessMode::Custom {
            match config.client_scope {
                NetworkClientScope::LocalSubnets => match local_subnet_rules {
                    Ok(rules) if !rules.is_empty() => allowed_clients.extend(rules),
                    Ok(_) => scope_errors.push("no non-loopback local subnets found".to_owned()),
                    Err(error) => scope_errors.push(error),
                },
                NetworkClientScope::PrivateRanges => {
                    allowed_clients.extend(private_network_rules());
                }
                NetworkClientScope::Custom => {}
            }
        }

        for raw_rule in &config.allowed_clients {
            let trimmed = raw_rule.trim();
            if trimmed.is_empty() {
                continue;
            }

            match ClientAddressRule::parse(trimmed) {
                Some(rule) => allowed_clients.push(rule),
                None => invalid_rules.push(trimmed.to_owned()),
            }
        }

        Self {
            allowed_clients,
            invalid_rules,
            scope_errors,
        }
    }

    fn remote_client_families(&self) -> RemoteClientFamilies {
        if self.allowed_clients.is_empty()
            && self.invalid_rules.is_empty()
            && self.scope_errors.is_empty()
        {
            return RemoteClientFamilies {
                ipv4: true,
                ipv6: true,
            };
        }
        if !self.invalid_rules.is_empty() || !self.scope_errors.is_empty() {
            return RemoteClientFamilies::NONE;
        }
        let admits = |ipv6: bool| {
            self.allowed_clients
                .iter()
                .any(|rule| !rule.is_loopback_only() && rule.network().is_ipv6() == ipv6)
        };
        RemoteClientFamilies {
            ipv4: admits(false),
            ipv6: admits(true),
        }
    }

    fn reject_request(&self, request: &Request<Body>) -> Option<Response> {
        if self.allowed_clients.is_empty()
            && self.invalid_rules.is_empty()
            && self.scope_errors.is_empty()
        {
            return None;
        }

        let Some(client_ip) = client_ip(request) else {
            return Some(
                DomainError::forbidden("Client IP is required by network.allowed_clients")
                    .into_response(),
            );
        };

        if client_ip.is_loopback() {
            return None;
        }

        if !self.invalid_rules.is_empty() {
            return Some(
                DomainError::forbidden_details(
                    "Invalid network.allowed_clients entries; remote clients are blocked",
                    json!({ "invalid_rules": &self.invalid_rules }),
                )
                .into_response(),
            );
        }

        if !self.scope_errors.is_empty() {
            return Some(
                DomainError::forbidden_details(
                    "Network client scope is unavailable; remote clients are blocked",
                    json!({ "scope_errors": &self.scope_errors }),
                )
                .into_response(),
            );
        }

        if self
            .allowed_clients
            .iter()
            .any(|rule| rule.matches(client_ip))
        {
            return None;
        }

        Some(
            DomainError::forbidden_details(
                "Client IP is not allowed by network.allowed_clients",
                json!({ "client_ip": client_ip.to_string() }),
            )
            .into_response(),
        )
    }
}

fn discover_local_subnet_rules() -> Result<Vec<ClientAddressRule>, String> {
    let interfaces = if_addrs::get_if_addrs()
        .map_err(|error| format!("failed to enumerate local interfaces: {error}"))?;
    let mut rules = Vec::new();

    for interface in interfaces {
        if interface.is_loopback() {
            continue;
        }

        match interface.addr {
            IfAddr::V4(addr) if !addr.ip.is_unspecified() => {
                rules.push(ClientAddressRule::Cidr {
                    network: IpAddr::V4(addr.ip),
                    prefix: addr.prefixlen.min(32),
                });
            }
            IfAddr::V6(addr) if !addr.ip.is_unspecified() => {
                rules.push(ClientAddressRule::Cidr {
                    network: IpAddr::V6(addr.ip),
                    prefix: addr.prefixlen.min(128),
                });
            }
            IfAddr::V4(_) | IfAddr::V6(_) => {}
        }
    }

    if rules.is_empty() {
        warn!("No non-loopback interfaces available for local subnet API allowlist");
    }

    Ok(rules)
}

fn private_network_rules() -> Vec<ClientAddressRule> {
    vec![
        ClientAddressRule::Cidr {
            network: IpAddr::V4(Ipv4Addr::new(10, 0, 0, 0)),
            prefix: 8,
        },
        ClientAddressRule::Cidr {
            network: IpAddr::V4(Ipv4Addr::new(172, 16, 0, 0)),
            prefix: 12,
        },
        ClientAddressRule::Cidr {
            network: IpAddr::V4(Ipv4Addr::new(192, 168, 0, 0)),
            prefix: 16,
        },
        ClientAddressRule::Cidr {
            network: IpAddr::V4(Ipv4Addr::new(169, 254, 0, 0)),
            prefix: 16,
        },
        ClientAddressRule::Cidr {
            network: IpAddr::V6(Ipv6Addr::new(0xfc00, 0, 0, 0, 0, 0, 0, 0)),
            prefix: 7,
        },
        ClientAddressRule::Cidr {
            network: IpAddr::V6(Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 0)),
            prefix: 10,
        },
    ]
}

#[derive(Clone)]
enum ClientAddressRule {
    Exact(IpAddr),
    Cidr { network: IpAddr, prefix: u8 },
}

impl ClientAddressRule {
    fn parse(raw: &str) -> Option<Self> {
        if let Some((network, prefix)) = raw.split_once('/') {
            let network = network.parse::<IpAddr>().ok()?;
            let prefix = prefix.parse::<u8>().ok()?;
            if cidr_prefix_valid(network, prefix) {
                return Some(Self::Cidr { network, prefix });
            }
            return None;
        }

        raw.parse::<IpAddr>().ok().map(Self::Exact)
    }

    fn matches(&self, client: IpAddr) -> bool {
        match *self {
            Self::Exact(ip) => ip == client,
            Self::Cidr { network, prefix } => cidr_contains(network, prefix, client),
        }
    }

    fn network(&self) -> IpAddr {
        match *self {
            Self::Exact(ip) | Self::Cidr { network: ip, .. } => ip,
        }
    }

    /// Whether every address the rule matches is a loopback address.
    fn is_loopback_only(&self) -> bool {
        match *self {
            Self::Exact(ip) => ip.is_loopback(),
            Self::Cidr {
                network: IpAddr::V4(network),
                prefix,
            } => prefix >= 8 && network.octets()[0] == 127,
            Self::Cidr {
                network: IpAddr::V6(network),
                prefix,
            } => prefix == 128 && network.is_loopback(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AccessTier {
    Read,
    Control,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OperationClass {
    Read,
    Write,
    Discovery,
    Pairing,
}

struct RateLimiter {
    clients: HashMap<String, ClientWindow>,
    discovery_window_start: Instant,
    discovery_count: u32,
}

struct ClientWindow {
    window_start: Instant,
    read_count: u32,
    write_count: u32,
    pairing_count: u32,
}

struct RateDecision {
    allowed: bool,
    limit: u32,
    remaining: u32,
    reset_epoch_secs: u64,
    retry_after_secs: u64,
}

impl RateLimiter {
    fn new() -> Self {
        Self {
            clients: HashMap::new(),
            discovery_window_start: Instant::now(),
            discovery_count: 0,
        }
    }

    fn check_and_record(&mut self, client_id: &str, class: OperationClass) -> RateDecision {
        let now = Instant::now();
        let now_epoch = unix_now_secs();
        let now_epoch_plus_window = now_epoch.saturating_add(RATE_WINDOW.as_secs());

        self.clients
            .retain(|_, window| now.saturating_duration_since(window.window_start) < RATE_WINDOW);

        if self.discovery_window_start.elapsed() >= RATE_WINDOW {
            self.discovery_window_start = now;
            self.discovery_count = 0;
        }

        if class == OperationClass::Discovery {
            if self.discovery_count >= DISCOVERY_LIMIT_PER_MIN {
                let retry_after = remaining_window_secs(self.discovery_window_start, now);
                return RateDecision {
                    allowed: false,
                    limit: DISCOVERY_LIMIT_PER_MIN,
                    remaining: 0,
                    reset_epoch_secs: now_epoch.saturating_add(retry_after),
                    retry_after_secs: retry_after,
                };
            }
            self.discovery_count = self.discovery_count.saturating_add(1);
        }

        let window = self
            .clients
            .entry(client_id.to_owned())
            .or_insert_with(|| ClientWindow {
                window_start: now,
                read_count: 0,
                write_count: 0,
                pairing_count: 0,
            });

        if window.window_start.elapsed() >= RATE_WINDOW {
            window.window_start = now;
            window.read_count = 0;
            window.write_count = 0;
            window.pairing_count = 0;
        }

        let (count_ref, limit) = match class {
            OperationClass::Read => (&mut window.read_count, READ_LIMIT_PER_MIN),
            OperationClass::Write | OperationClass::Discovery => {
                (&mut window.write_count, WRITE_LIMIT_PER_MIN)
            }
            OperationClass::Pairing => (&mut window.pairing_count, PAIRING_LIMIT_PER_MIN),
        };

        let retry_after = remaining_window_secs(window.window_start, now);
        if *count_ref >= limit {
            return RateDecision {
                allowed: false,
                limit,
                remaining: 0,
                reset_epoch_secs: now_epoch.saturating_add(retry_after),
                retry_after_secs: retry_after,
            };
        }

        *count_ref = count_ref.saturating_add(1);
        let remaining = limit.saturating_sub(*count_ref);
        let reset_epoch_secs = if retry_after == 0 {
            now_epoch_plus_window
        } else {
            now_epoch.saturating_add(retry_after)
        };

        RateDecision {
            allowed: true,
            limit,
            remaining,
            reset_epoch_secs,
            retry_after_secs: retry_after,
        }
    }
}

pub async fn enforce_security(
    State(state): State<SecurityState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let mut request = request;
    // A grant attached before this layer, as on a replayed WebSocket
    // command or an in-process call, was resolved earlier and is never
    // resolved again, so its revocation is checked here, ahead of every
    // other branch.
    if request
        .extensions()
        .get::<CredentialGrant>()
        .is_some_and(|grant| grant.revocation().is_cancelled())
    {
        return DomainError::unauthorized("Invalid API key").into_response();
    }

    if request
        .extensions_mut()
        .remove::<TrustedLocalControl>()
        .is_some()
    {
        request.extensions_mut().insert(
            RequestAuthContext::authenticated(AccessTier::Control)
                .with_locality(RequestLocality::InProcess),
        );
        return next.run(request).await;
    }

    if let Some(response) = state.network.reject_request(&request) {
        return response;
    }

    let locality = if request_is_loopback(&request) {
        RequestLocality::Loopback
    } else {
        RequestLocality::Network
    };

    if locality == RequestLocality::Network
        && extract_token(&request).is_some_and(|token| state.is_session_credential(&token))
    {
        return DomainError::unauthorized("Invalid API key").into_response();
    }

    // Exempt paths, like public routes, run ahead of the loopback
    // cross-site gate, so they never confer locality or carry a grant.
    if is_bearer_exempt(request.uri().path(), &state.static_assets)
        && !state
            .public_routes
            .withholds_exemption(request.uri().path())
    {
        request.extensions_mut().remove::<CredentialGrant>();
        request
            .extensions_mut()
            .insert(RequestAuthContext::unsecured());
        return next.run(request).await;
    }

    // A public route needs no credential and confers none, locality
    // included: it runs ahead of the loopback cross-site gate, so a page
    // on any origin can reach it through the browser, and its handler must
    // never see such a caller as local. A grant attached upstream, as on a
    // replayed WebSocket command, is dropped for the same reason. Locality
    // buys no rate exemption either; the budget exists for those pages.
    // In-process trusted calls were admitted above and keep their
    // authority: they are the daemon's own process, not a request.
    if let Some(class) = state
        .public_routes
        .class_for(request.method(), request.uri().path())
    {
        request.extensions_mut().remove::<CredentialGrant>();
        request
            .extensions_mut()
            .insert(RequestAuthContext::anonymous());
        return rate_limited(&state, request, next, class).await;
    }

    let optional_system_auth = is_optional_system_auth(request.method(), request.uri().path());

    if locality == RequestLocality::Loopback {
        if is_mutating_request(request.method())
            && is_cross_site_request(&request)
            && !has_trusted_tauri_session(&state, &request)
        {
            return DomainError::forbidden(
                "Cross-site mutating requests to the loopback API are blocked to prevent CSRF.",
            )
            .into_response();
        }

        let resolved = extract_token(&request).map(|token| state.resolve_loopback_token(&token));
        match resolved {
            Some(Some(resolved)) => resolved.attach(&mut request, locality),
            Some(None) if optional_system_auth => {
                return DomainError::unauthorized("Invalid API key").into_response();
            }
            Some(None) | None => {
                request
                    .extensions_mut()
                    .insert(RequestAuthContext::unsecured().with_locality(locality));
            }
        }
        return next.run(request).await;
    }

    if !state.security_enabled() {
        if optional_system_auth && extract_token(&request).is_some() {
            return DomainError::unauthorized("Invalid API key").into_response();
        }
        if request.extensions().get::<RequestAuthContext>().is_none() {
            request
                .extensions_mut()
                .insert(RequestAuthContext::unsecured().with_locality(locality));
        }
        return next.run(request).await;
    }

    let method = request.method().clone();
    let path = request.uri().path().to_owned();
    let mut resolved = request
        .extensions()
        .get::<RequestAuthContext>()
        .copied()
        .filter(|context| context.security_enabled() && context.granted_tier().is_some())
        .map(|context| ResolvedCredential {
            context,
            grant: request.extensions().get::<CredentialGrant>().cloned(),
        });

    if method != Method::OPTIONS {
        if resolved.is_none() {
            if let Some(token) = extract_token(&request) {
                let Some(found) = state.resolve_presented_token(&token) else {
                    return DomainError::unauthorized("Invalid API key").into_response();
                };
                resolved = Some(found);
            } else if !optional_system_auth {
                return DomainError::unauthorized(
                    "Missing API key. Use Authorization: Bearer <token>.",
                )
                .into_response();
            }
        }

        if let Some(granted) = resolved
            .as_ref()
            .and_then(|resolved| resolved.context.granted_tier())
            && !tier_satisfies(granted, required_tier_for_method(&method))
        {
            return DomainError::forbidden_details(
                "Read-only API key cannot perform write operations",
                json!({
                    "required_tier": "control",
                    "current_tier": "read"
                }),
            )
            .into_response();
        }
    }

    match resolved {
        Some(resolved) => resolved.attach(&mut request, locality),
        None => {
            request
                .extensions_mut()
                .insert(RequestAuthContext::preflight());
        }
    }

    let operation = classify_operation(&method, &path);
    rate_limited(&state, request, next, operation).await
}

/// Spend one unit of `operation`'s budget for this client, then serve.
async fn rate_limited(
    state: &SecurityState,
    request: Request<Body>,
    next: Next,
    operation: OperationClass,
) -> Response {
    let client_id = client_identity(&request);

    let decision = {
        let mut limiter = state.rate_limiter.lock().await;
        limiter.check_and_record(&client_id, operation)
    };

    if !decision.allowed {
        let mut response = DomainError::RateLimited {
            message: rate_limit_message(operation, decision.retry_after_secs),
            limit: decision.limit,
            window_seconds: RATE_WINDOW.as_secs(),
            retry_after_secs: decision.retry_after_secs,
        }
        .into_response();
        apply_rate_headers(&mut response, &decision);
        return response;
    }

    let mut response = next.run(request).await;
    apply_rate_headers(&mut response, &decision);
    response
}

pub(crate) fn mark_trusted_local_control(request: &mut Request<Body>) {
    request.extensions_mut().insert(TrustedLocalControl);
}

pub(crate) const fn trusted_local_control_context() -> RequestAuthContext {
    RequestAuthContext::authenticated(AccessTier::Control).with_locality(RequestLocality::InProcess)
}

/// Swagger UI's mount and the document it fetches.
///
/// The page loads its own bundle and then its OpenAPI document from a
/// second request, and a browser attaches no `Authorization` header to
/// either. Without this, a keyed daemon serves an API-docs page that
/// cannot fetch the API docs.
const SWAGGER_UI_PREFIX: &str = "/api/v1/docs";
const OPENAPI_DOCUMENT_PATH: &str = "/api/v1/openapi.json";

/// Whether bearer auth applies to a request path.
///
/// Exempt paths still pass through the network access policy above
/// this check; the exemption is from presenting a key, not from being
/// allowed to reach the daemon at all.
fn is_bearer_exempt(path: &str, static_assets: &StaticAssetSurface) -> bool {
    path == "/health" || is_docs_exempt_path(path) || static_assets.serves(path)
}

/// The API docs paths the bearer exemption covers.
fn is_docs_exempt_path(path: &str) -> bool {
    path == OPENAPI_DOCUMENT_PATH || path_within(path, SWAGGER_UI_PREFIX)
}

fn is_optional_system_auth(method: &Method, path: &str) -> bool {
    matches!(*method, Method::GET | Method::HEAD) && path == "/api/v1/system"
}

/// `true` when `path` is `prefix` itself or sits beneath it.
///
/// Segment-aware on purpose: `/api/v1/docsearch` is not inside
/// `/api/v1/docs`, and a plain `starts_with` would say it is.
fn path_within(path: &str, prefix: &str) -> bool {
    path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn required_tier_for_method(method: &Method) -> AccessTier {
    if matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS) {
        AccessTier::Read
    } else {
        AccessTier::Control
    }
}

fn is_mutating_request(method: &Method) -> bool {
    !matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

/// Returns `true` only when the browser explicitly marks the request as
/// cross-site. Ordinary same-origin/same-site requests and non-browser clients
/// omit or set a non-`cross-site` value. The bundled Tauri UI is cross-site and
/// passes only through the separate exact-origin plus session-credential gate.
fn is_cross_site_request(request: &Request<Body>) -> bool {
    request
        .headers()
        .get("sec-fetch-site")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|site| site == "cross-site")
}

/// Compare a presented token against a configured key without leaking
/// how far the two agreed.
///
/// `str`'s `PartialEq` stops at the first differing byte, which lets a
/// caller with a timer recover a key one byte at a time. Byte length is
/// still observable (constant-time comparison of different-length inputs
/// is not a thing), and that is the accepted residual.
fn secret_matches(configured: Option<&str>, presented: &str) -> bool {
    configured.is_some_and(|configured| {
        configured
            .as_bytes()
            .ct_eq(presented.as_bytes())
            .unwrap_u8()
            == 1
    })
}

fn has_trusted_tauri_session(state: &SecurityState, request: &Request<Body>) -> bool {
    request
        .headers()
        .get(header::ORIGIN)
        .is_some_and(is_trusted_tauri_origin)
        && extract_token(request).is_some_and(|token| state.is_session_credential(&token))
}

fn resolve_token_tier(token: &str, auth: &AuthConfig) -> Option<AccessTier> {
    // Both comparisons run before either result is read, so the time a
    // rejection takes does not report which key the caller came closest
    // to matching.
    let control_matches = secret_matches(auth.control_key.as_deref(), token);
    let read_matches = secret_matches(auth.read_key.as_deref(), token);

    if control_matches {
        if token.starts_with("hc_ak_r_") {
            Some(AccessTier::Read)
        } else {
            Some(AccessTier::Control)
        }
    } else if read_matches {
        Some(AccessTier::Read)
    } else {
        None
    }
}

fn tier_satisfies(granted: AccessTier, required: AccessTier) -> bool {
    matches!(
        (granted, required),
        (AccessTier::Control, _) | (AccessTier::Read, AccessTier::Read)
    )
}

fn classify_operation(method: &Method, path: &str) -> OperationClass {
    if *method == Method::POST && path == "/api/v1/devices/discover" {
        OperationClass::Discovery
    } else if is_pairing_path(path) && matches!(*method, Method::POST | Method::DELETE) {
        OperationClass::Pairing
    } else if matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS) {
        OperationClass::Read
    } else {
        OperationClass::Write
    }
}

fn is_pairing_path(path: &str) -> bool {
    let mut segments = path.trim_matches('/').split('/');
    matches!(
        (
            segments.next(),
            segments.next(),
            segments.next(),
            segments.next(),
            segments.next(),
            segments.next(),
        ),
        (
            Some("api"),
            Some("v1"),
            Some("devices"),
            Some(_),
            Some("pair"),
            None,
        )
    )
}

fn rate_limit_message(class: OperationClass, retry_after: u64) -> String {
    let scope = match class {
        OperationClass::Read => "Read operation",
        OperationClass::Write => "Write operation",
        OperationClass::Discovery => "Discovery operation",
        OperationClass::Pairing => "Pairing operation",
    };
    format!("{scope} rate limit exceeded. Retry in {retry_after} seconds.")
}

fn extract_token(request: &Request<Body>) -> Option<String> {
    if let Some(raw_header) = request.headers().get(axum::http::header::AUTHORIZATION) {
        let header_value = raw_header.to_str().ok()?;
        if let Some(token) = parse_bearer_header(header_value) {
            return Some(token.to_owned());
        }
    }

    if allows_query_token(request) {
        return token_from_query(request.uri().query());
    }

    None
}

fn parse_bearer_header(value: &str) -> Option<&str> {
    let (scheme, token) = value.split_once(' ')?;
    if scheme.eq_ignore_ascii_case("bearer") && !token.is_empty() {
        Some(token)
    } else {
        None
    }
}

fn token_from_query(query: Option<&str>) -> Option<String> {
    let query = query?;
    for pair in query.split('&') {
        let (raw_key, raw_value) = pair.split_once('=').unwrap_or((pair, ""));
        if raw_key == "token" && !raw_value.is_empty() {
            return Some(raw_value.to_owned());
        }
    }
    None
}

fn allows_query_token(request: &Request<Body>) -> bool {
    matches!(request.uri().path(), "/api/v1/ws")
        && request.method() == Method::GET
        && request
            .headers()
            .get(header::UPGRADE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.eq_ignore_ascii_case("websocket"))
}

fn client_identity(request: &Request<Body>) -> String {
    client_ip(request).map_or_else(|| "unknown".to_owned(), |ip| ip.to_string())
}

pub(crate) fn request_is_loopback(request: &Request<Body>) -> bool {
    client_ip(request).is_some_and(|ip| ip.is_loopback())
}

fn client_ip(request: &Request<Body>) -> Option<IpAddr> {
    if let Some(socket_addr) = peer_socket_addr(request) {
        if socket_addr.ip().is_loopback() && forwarded_client_header_present(request) {
            return forwarded_client_ip(request)?.parse::<IpAddr>().ok();
        }
        return Some(socket_addr.ip());
    }

    None
}

fn peer_socket_addr(request: &Request<Body>) -> Option<std::net::SocketAddr> {
    request
        .extensions()
        .get::<ConnectInfo<std::net::SocketAddr>>()
        .map(|ConnectInfo(socket_addr)| *socket_addr)
}

fn forwarded_client_ip(request: &Request<Body>) -> Option<String> {
    if let Some(forwarded) = request.headers().get("x-forwarded-for") {
        let value = forwarded.to_str().ok()?;
        let first = value.split(',').next()?;
        let trimmed = first.trim();
        return (!trimmed.is_empty()).then(|| trimmed.to_owned());
    }

    if let Some(real_ip) = request.headers().get("x-real-ip") {
        let value = real_ip.to_str().ok()?;
        let trimmed = value.trim();
        return (!trimmed.is_empty()).then(|| trimmed.to_owned());
    }

    None
}

fn forwarded_client_header_present(request: &Request<Body>) -> bool {
    request.headers().contains_key("x-forwarded-for") || request.headers().contains_key("x-real-ip")
}

fn apply_rate_headers(response: &mut Response, decision: &RateDecision) {
    let headers = response.headers_mut();
    insert_header(headers, HEADER_RATE_LIMIT_LIMIT, u64::from(decision.limit));
    insert_header(
        headers,
        HEADER_RATE_LIMIT_REMAINING,
        u64::from(decision.remaining),
    );
    insert_header(headers, HEADER_RATE_LIMIT_RESET, decision.reset_epoch_secs);
    if !decision.allowed {
        insert_header(headers, HEADER_RETRY_AFTER, decision.retry_after_secs);
    }
}

fn insert_header(headers: &mut axum::http::HeaderMap, name: HeaderName, value: u64) {
    if let Ok(header_value) = HeaderValue::from_str(&value.to_string()) {
        headers.insert(name, header_value);
    }
}

fn remaining_window_secs(window_start: Instant, now: Instant) -> u64 {
    let elapsed = now.saturating_duration_since(window_start);
    RATE_WINDOW.saturating_sub(elapsed).as_secs()
}

fn unix_now_secs() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn cidr_prefix_valid(network: IpAddr, prefix: u8) -> bool {
    match network {
        IpAddr::V4(_) => prefix <= 32,
        IpAddr::V6(_) => prefix <= 128,
    }
}

fn cidr_contains(network: IpAddr, prefix: u8, client: IpAddr) -> bool {
    match (network, client) {
        (IpAddr::V4(network), IpAddr::V4(client)) => {
            masked_v4(network, prefix) == masked_v4(client, prefix)
        }
        (IpAddr::V6(network), IpAddr::V6(client)) => {
            masked_v6(network, prefix) == masked_v6(client, prefix)
        }
        _ => false,
    }
}

fn masked_v4(address: Ipv4Addr, prefix: u8) -> u32 {
    let bits = u32::from(address);
    let mask = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix)
    };
    bits & mask
}

fn masked_v6(address: Ipv6Addr, prefix: u8) -> u128 {
    let bits = u128::from_be_bytes(address.octets());
    let mask = if prefix == 0 {
        0
    } else {
        u128::MAX << (128 - prefix)
    };
    bits & mask
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    use axum::extract::{ConnectInfo, Extension};
    use axum::http::header::AUTHORIZATION;
    use axum::routing::{get, post};
    use axum::{Router, body::Body};
    use http::{Method, Request, StatusCode};
    use serde_json::Value;
    use tower::ServiceExt;

    use hypercolor_types::config::{NetworkAccessMode, NetworkClientScope, NetworkConfig};
    use hypercolor_types::service::ProtectedControlCredential;

    use super::{
        AccessTier, AuthConfig, ClientAddressRule, NetworkAccessPolicy, RemoteClientFamilies,
        RequestAuthContext, SecurityState, StaticAssetSurface, enforce_security, normalize_api_key,
        parse_protected_control_credential, path_within, resolve_token_tier,
    };
    use crate::macos_owner::{
        MACOS_DAEMON_SESSION_ATTESTATION_SCHEMA_VERSION, MacosDaemonOwner,
        MacosDaemonSessionAttestation, MacosOwnerIdentity, MacosServerSessionId,
    };
    const CONTROL_KEY: &str = "hc_ak_control_test";
    const READ_KEY: &str = "hc_ak_r_read_test";

    fn secured_test_router() -> Router {
        let state = SecurityState::with_keys(Some(CONTROL_KEY), Some(READ_KEY));
        router_with_security_state(state)
    }

    fn router_with_security_state(state: SecurityState) -> Router {
        Router::new()
            .route("/health", get(|| async { StatusCode::OK }))
            .route("/api/v1/devices", get(|| async { StatusCode::OK }))
            .route(
                "/api/v1/system",
                get(
                    |Extension(context): Extension<RequestAuthContext>| async move {
                        axum::Json(serde_json::json!({
                            "identity": true,
                            "status": context.can_read_system_status(),
                            "protected_selection_ids": context.can_protected_control(),
                        }))
                    },
                ),
            )
            .route(
                "/api/v1/ws",
                get(
                    |Extension(context): Extension<RequestAuthContext>| async move {
                        if context.can_protected_control() {
                            StatusCode::OK
                        } else {
                            StatusCode::FORBIDDEN
                        }
                    },
                ),
            )
            .route(
                "/api/v1/protected-control",
                get(
                    |Extension(context): Extension<RequestAuthContext>| async move {
                        if context.can_protected_control() {
                            StatusCode::OK
                        } else {
                            StatusCode::FORBIDDEN
                        }
                    },
                ),
            )
            .route("/api/v1/scenes", post(|| async { StatusCode::CREATED }))
            .route(
                "/api/v1/effects/install",
                post(|| async { StatusCode::CREATED }),
            )
            .route(
                "/api/v1/devices/discover",
                post(|| async { StatusCode::ACCEPTED }),
            )
            .route(
                "/api/v1/devices/device-1/pair",
                post(|| async { StatusCode::OK }).delete(|| async { StatusCode::NO_CONTENT }),
            )
            .layer(axum::middleware::from_fn_with_state(
                state,
                enforce_security,
            ))
    }

    fn allowlist_test_router(allowed_clients: Vec<String>) -> Router {
        router_with_security_state(SecurityState::with_network_config(NetworkConfig {
            allowed_clients,
            ..NetworkConfig::default()
        }))
    }

    fn network_policy_test_router(
        config: &NetworkConfig,
        local_subnet_rules: Vec<ClientAddressRule>,
    ) -> Router {
        router_with_security_state(SecurityState::with_network_policy(
            NetworkAccessPolicy::from_config_with_local_subnets(config, Ok(local_subnet_rules)),
        ))
    }

    #[test]
    fn network_policy_reports_the_families_remote_clients_are_admitted_in() {
        let both = RemoteClientFamilies {
            ipv4: true,
            ipv6: true,
        };
        let ipv4_only = RemoteClientFamilies {
            ipv4: true,
            ipv6: false,
        };
        let ipv6_only = RemoteClientFamilies {
            ipv4: false,
            ipv6: true,
        };
        let subnet = ClientAddressRule::parse("192.168.1.0/24").expect("rule should parse");
        let families = |config: &NetworkConfig, subnets: Result<Vec<ClientAddressRule>, String>| {
            NetworkAccessPolicy::from_config_with_local_subnets(config, subnets)
                .remote_client_families()
        };
        let lan_trusted = NetworkConfig {
            access_mode: NetworkAccessMode::LanTrusted,
            ..NetworkConfig::default()
        };

        assert_eq!(families(&lan_trusted, Ok(vec![subnet.clone()])), ipv4_only);
        assert_eq!(
            families(&NetworkConfig::default(), Ok(Vec::new())),
            both,
            "no allowlist admits every client"
        );
        assert_eq!(
            families(&lan_trusted, Ok(Vec::new())),
            RemoteClientFamilies::NONE,
            "an unresolvable local-subnet scope blocks remote clients"
        );
        assert_eq!(
            families(
                &NetworkConfig {
                    allowed_clients: vec!["invalid".to_owned()],
                    ..lan_trusted.clone()
                },
                Ok(vec![subnet])
            ),
            RemoteClientFamilies::NONE
        );
        let custom = |allowed: &[&str]| NetworkConfig {
            access_mode: NetworkAccessMode::Custom,
            allow_unauthenticated_remote_access: true,
            allowed_clients: allowed.iter().map(|rule| (*rule).to_owned()).collect(),
            ..NetworkConfig::default()
        };
        assert_eq!(
            families(
                &custom(&["127.0.0.1", "127.0.0.0/8", "::1", "::1/128"]),
                Ok(Vec::new())
            ),
            RemoteClientFamilies::NONE
        );
        assert_eq!(
            families(&custom(&["127.0.0.1", "10.0.0.5"]), Ok(Vec::new())),
            ipv4_only
        );
        assert_eq!(families(&custom(&["fd00::/8"]), Ok(Vec::new())), ipv6_only);
        assert_eq!(
            families(&custom(&["0.0.0.0/0", "::/0"]), Ok(Vec::new())),
            both
        );

        let v4: IpAddr = "192.168.1.42"
            .parse()
            .expect("fixture address should parse");
        let v6: IpAddr = "fd00::42".parse().expect("fixture address should parse");
        assert!(ipv4_only.admits_family_of(v4));
        assert!(!ipv4_only.admits_family_of(v6));
        assert!(ipv6_only.admits_family_of(v6));
        assert!(!ipv6_only.admits_family_of(v4));
    }

    #[test]
    fn keyless_remote_families_come_from_the_serving_state() {
        let subnet = ClientAddressRule::parse("192.168.1.0/24").expect("rule should parse");
        let lan_trusted = NetworkConfig {
            access_mode: NetworkAccessMode::LanTrusted,
            ..NetworkConfig::default()
        };
        let policy = || {
            NetworkAccessPolicy::from_config_with_local_subnets(
                &lan_trusted,
                Ok(vec![subnet.clone()]),
            )
        };

        assert_eq!(
            SecurityState::with_network_policy(policy()).keyless_remote_client_families(),
            RemoteClientFamilies {
                ipv4: true,
                ipv6: false,
            }
        );
        for keys in [(Some(CONTROL_KEY), None), (None, Some(READ_KEY))] {
            let mut state = SecurityState::with_keys(keys.0, keys.1);
            state.network = policy();
            assert_eq!(
                state.keyless_remote_client_families(),
                RemoteClientFamilies::NONE,
                "any configured key makes every remote client present a credential"
            );
        }
    }

    async fn response_json(response: axum::response::Response) -> Value {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("failed to read body");
        serde_json::from_slice(&bytes).expect("failed to parse JSON body")
    }

    fn with_bearer(request: http::request::Builder, token: &str) -> http::request::Builder {
        request.header(AUTHORIZATION, format!("Bearer {token}"))
    }

    fn with_connect_info(mut request: Request<Body>, ip: IpAddr, port: u16) -> Request<Body> {
        request
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::new(ip, port)));
        request
    }

    async fn loopback_cross_site_mutation(
        state: SecurityState,
        origin: Option<&str>,
        token: Option<&str>,
    ) -> StatusCode {
        let mut builder = Request::builder()
            .method("POST")
            .uri("/api/v1/scenes")
            .header("sec-fetch-site", "cross-site");
        if let Some(origin) = origin {
            builder = builder.header(http::header::ORIGIN, origin);
        }
        if let Some(token) = token {
            builder = with_bearer(builder, token);
        }
        router_with_security_state(state)
            .oneshot(with_connect_info(
                builder.body(Body::empty()).expect("request should build"),
                IpAddr::V4(Ipv4Addr::LOCALHOST),
                1042,
            ))
            .await
            .expect("request failed")
            .status()
    }

    #[test]
    fn normalize_api_key_ignores_missing_or_blank_values() {
        assert_eq!(normalize_api_key(None), None);
        assert_eq!(normalize_api_key(Some(String::new())), None);
        assert_eq!(normalize_api_key(Some("   ".to_owned())), None);
    }

    #[test]
    fn normalize_api_key_keeps_configured_value() {
        assert_eq!(
            normalize_api_key(Some(CONTROL_KEY.to_owned())),
            Some(CONTROL_KEY.to_owned())
        );
    }

    #[test]
    fn protected_control_environment_value_requires_a_canonical_token() {
        assert_eq!(
            parse_protected_control_credential(None).expect("missing credential is valid"),
            None
        );
        assert!(parse_protected_control_credential(Some("not-a-token".to_owned())).is_err());

        let expected = ProtectedControlCredential::from_bytes([0x52; 32]);
        let parsed = parse_protected_control_credential(Some(expected.expose_secret().to_owned()))
            .expect("canonical credential parses");
        assert_eq!(parsed, Some(expected));
    }

    #[test]
    fn attested_session_preserves_launcher_session_authority() {
        let launcher = ProtectedControlCredential::from_bytes([0x41; 32]);
        let attested = ProtectedControlCredential::from_bytes([0x42; 32]);
        let attestation = MacosDaemonSessionAttestation {
            schema_version: MACOS_DAEMON_SESSION_ATTESTATION_SCHEMA_VERSION,
            owner: MacosDaemonOwner::AppSidecar,
            owner_epoch: 1,
            owner_identity: MacosOwnerIdentity::new(
                "audit-test",
                "/Applications/Hypercolor.app/Contents/MacOS/hypercolor-daemon",
                "requirement-test",
                4242,
            )
            .expect("fixture identity should be valid"),
            server_session_id: MacosServerSessionId::from_bytes([0x43; 16]),
            protected_control_credential: attested.clone(),
        };
        let mut state = SecurityState::with_session_credential(launcher.clone());

        state.install_macos_daemon_session(&attestation);

        assert!(state.is_session_credential(launcher.expose_secret()));
        assert!(state.is_session_credential(attested.expose_secret()));
    }

    #[tokio::test]
    async fn health_endpoint_remains_open_when_security_is_enabled() {
        let app = secured_test_router();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .expect("failed to build request"),
            )
            .await
            .expect("request failed");

        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().get("x-ratelimit-limit").is_none());
    }

    /// The UI shell, its assets, and Swagger UI are reachable without a
    /// key; every API route behind the same middleware still is not.
    fn ui_serving_test_router() -> Router {
        let state = SecurityState::with_keys(Some(CONTROL_KEY), Some(READ_KEY)).with_static_assets(
            StaticAssetSurface::mounted([
                "/api".to_owned(),
                "/health".to_owned(),
                "/mcp".to_owned(),
            ]),
        );
        Router::new()
            .route("/api/v1/devices", get(|| async { StatusCode::OK }))
            .route("/api/v1/scenes", post(|| async { StatusCode::CREATED }))
            .route("/api/v1/docs", get(|| async { StatusCode::OK }))
            .route("/api/v1/docs/{*rest}", get(|| async { StatusCode::OK }))
            .route("/api/v1/openapi.json", get(|| async { StatusCode::OK }))
            .route("/mcp", post(|| async { StatusCode::OK }))
            .fallback(|| async { StatusCode::OK })
            .layer(axum::middleware::from_fn_with_state(
                state,
                enforce_security,
            ))
    }

    async fn status_for(app: &Router, method: &str, uri: &str) -> StatusCode {
        app.clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .body(Body::empty())
                    .expect("failed to build request"),
            )
            .await
            .expect("request failed")
            .status()
    }

    #[tokio::test]
    async fn static_assets_and_swagger_ui_do_not_need_an_api_key() {
        let app = ui_serving_test_router();

        // A browser attaches no Authorization header to the document it
        // was told to load, nor to any subresource that document pulls.
        for path in [
            "/",
            "/index.html",
            "/assets/index-a1b2c3.js",
            "/assets/index-a1b2c3.css",
            "/studio/zones",
            "/api/v1/docs",
            "/api/v1/docs/swagger-ui.css",
            "/api/v1/openapi.json",
        ] {
            assert_eq!(
                status_for(&app, "GET", path).await,
                StatusCode::OK,
                "{path} should be served without a key"
            );
        }
    }

    #[tokio::test]
    async fn the_asset_exemption_does_not_widen_onto_dynamic_routes() {
        let app = ui_serving_test_router();

        for (method, path) in [
            ("GET", "/api/v1/devices"),
            ("POST", "/api/v1/scenes"),
            ("POST", "/mcp"),
            // Segment-aware matching: a path that merely starts with the
            // Swagger prefix is not inside it.
            ("GET", "/api/v1/docsearch"),
        ] {
            assert_eq!(
                status_for(&app, method, path).await,
                StatusCode::UNAUTHORIZED,
                "{method} {path} must still require a key"
            );
        }
    }

    #[tokio::test]
    async fn assets_stay_keyed_when_no_ui_directory_is_mounted() {
        // A headless daemon serves no shell, so nothing falls outside the
        // API surface and the blanket exemption never applies.
        let app = secured_test_router();

        assert_eq!(
            status_for(&app, "GET", "/index.html").await,
            StatusCode::UNAUTHORIZED
        );
    }

    #[test]
    fn an_empty_prefix_list_still_protects_the_api() {
        // The exemption is security-critical, so the type refuses to
        // build a surface that swallows the API even when the caller
        // supplies nothing.
        let surface = StaticAssetSurface::mounted([]);

        assert!(!surface.serves("/api/v1/devices"));
        assert!(!surface.serves("/health"));
        assert!(surface.serves("/index.html"));
    }

    #[test]
    fn path_within_is_segment_aware() {
        assert!(path_within("/api/v1/docs", "/api/v1/docs"));
        assert!(path_within("/api/v1/docs/", "/api/v1/docs"));
        assert!(path_within("/api/v1/docs/index.html", "/api/v1/docs"));
        assert!(!path_within("/api/v1/docsearch", "/api/v1/docs"));
        assert!(!path_within("/api/v1/doc", "/api/v1/docs"));
    }

    #[test]
    fn token_comparison_still_resolves_the_right_tier() {
        let auth = AuthConfig {
            control_key: Some(CONTROL_KEY.to_owned()),
            read_key: Some(READ_KEY.to_owned()),
        };

        assert_eq!(
            resolve_token_tier(CONTROL_KEY, &auth),
            Some(AccessTier::Control)
        );
        assert_eq!(resolve_token_tier(READ_KEY, &auth), Some(AccessTier::Read));
        assert_eq!(resolve_token_tier("hc_ak_control_tes", &auth), None);
        assert_eq!(resolve_token_tier("hc_ak_control_testx", &auth), None);
        assert_eq!(resolve_token_tier("", &auth), None);
        assert_eq!(
            resolve_token_tier(CONTROL_KEY, &AuthConfig::default()),
            None
        );
    }

    #[test]
    fn a_read_prefixed_control_key_still_grants_only_read() {
        let auth = AuthConfig {
            control_key: Some("hc_ak_r_dual_purpose".to_owned()),
            read_key: None,
        };

        assert_eq!(
            resolve_token_tier("hc_ak_r_dual_purpose", &auth),
            Some(AccessTier::Read)
        );
    }

    #[tokio::test]
    async fn rejects_missing_token_when_security_enabled() {
        let app = secured_test_router();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/devices")
                    .body(Body::empty())
                    .expect("failed to build request"),
            )
            .await
            .expect("request failed");

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let json = response_json(response).await;
        assert_eq!(json["error"]["code"], "unauthorized");
    }

    #[tokio::test]
    async fn anonymous_system_request_gets_public_identity_at_the_read_limit() {
        let response = secured_test_router()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/system")
                    .body(Body::empty())
                    .expect("failed to build request"),
            )
            .await
            .expect("request failed");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["x-ratelimit-limit"], "120");
        let json = response_json(response).await;
        assert_eq!(json["identity"], true);
        assert_eq!(json["status"], false);
        assert_eq!(json["protected_selection_ids"], false);
    }

    #[tokio::test]
    async fn system_rejects_invalid_supplied_credentials() {
        let response = secured_test_router()
            .oneshot(
                with_bearer(Request::builder().uri("/api/v1/system"), "invalid")
                    .body(Body::empty())
                    .expect("failed to build request"),
            )
            .await
            .expect("request failed");

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let json = response_json(response).await;
        assert_eq!(json["error"]["code"], "unauthorized");
    }

    #[tokio::test]
    async fn system_read_and_control_keys_receive_their_status_projection() {
        let app = secured_test_router();
        let read = app
            .clone()
            .oneshot(
                with_bearer(Request::builder().uri("/api/v1/system"), READ_KEY)
                    .body(Body::empty())
                    .expect("failed to build request"),
            )
            .await
            .expect("request failed");
        let control = app
            .oneshot(
                with_bearer(Request::builder().uri("/api/v1/system"), CONTROL_KEY)
                    .body(Body::empty())
                    .expect("failed to build request"),
            )
            .await
            .expect("request failed");

        assert_eq!(read.status(), StatusCode::OK);
        assert_eq!(control.status(), StatusCode::OK);
        let read = response_json(read).await;
        let control = response_json(control).await;
        assert_eq!(read["status"], true);
        assert_eq!(read["protected_selection_ids"], false);
        assert_eq!(control["status"], true);
        assert_eq!(control["protected_selection_ids"], true);
    }

    #[tokio::test]
    async fn loopback_system_is_full_but_rejects_an_invalid_bearer() {
        let app = secured_test_router();
        let local = app
            .clone()
            .oneshot(with_connect_info(
                Request::builder()
                    .uri("/api/v1/system")
                    .body(Body::empty())
                    .expect("failed to build request"),
                IpAddr::V4(Ipv4Addr::LOCALHOST),
                1042,
            ))
            .await
            .expect("request failed");
        let invalid = app
            .oneshot(with_connect_info(
                with_bearer(Request::builder().uri("/api/v1/system"), "invalid")
                    .body(Body::empty())
                    .expect("failed to build request"),
                IpAddr::V4(Ipv4Addr::LOCALHOST),
                1042,
            ))
            .await
            .expect("request failed");

        assert_eq!(local.status(), StatusCode::OK);
        assert_eq!(response_json(local).await["status"], true);
        assert_eq!(invalid.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn loopback_clients_do_not_need_api_key() {
        let app = secured_test_router();
        let response = app
            .oneshot(with_connect_info(
                Request::builder()
                    .uri("/api/v1/devices")
                    .body(Body::empty())
                    .expect("failed to build request"),
                IpAddr::V4(Ipv4Addr::LOCALHOST),
                1042,
            ))
            .await
            .expect("request failed");

        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().get("x-ratelimit-limit").is_none());
    }

    #[tokio::test]
    async fn loopback_locality_does_not_grant_protected_control() {
        let response = secured_test_router()
            .oneshot(with_connect_info(
                Request::builder()
                    .uri("/api/v1/protected-control")
                    .body(Body::empty())
                    .expect("failed to build request"),
                IpAddr::V4(Ipv4Addr::LOCALHOST),
                1042,
            ))
            .await
            .expect("request failed");

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn loopback_read_key_does_not_grant_protected_control() {
        let response = secured_test_router()
            .oneshot(with_connect_info(
                with_bearer(
                    Request::builder().uri("/api/v1/protected-control"),
                    READ_KEY,
                )
                .body(Body::empty())
                .expect("failed to build request"),
                IpAddr::V4(Ipv4Addr::LOCALHOST),
                1042,
            ))
            .await
            .expect("request failed");

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn loopback_control_key_grants_protected_control() {
        let response = secured_test_router()
            .oneshot(with_connect_info(
                with_bearer(
                    Request::builder().uri("/api/v1/protected-control"),
                    CONTROL_KEY,
                )
                .body(Body::empty())
                .expect("failed to build request"),
                IpAddr::V4(Ipv4Addr::LOCALHOST),
                1042,
            ))
            .await
            .expect("request failed");

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn loopback_session_credential_grants_control_without_enabling_public_auth() {
        let credential = ProtectedControlCredential::from_bytes([0x42; 32]);
        let state = SecurityState::with_session_credential(credential.clone());
        assert!(!state.security_enabled());
        let context = state
            .resolve_loopback_token(credential.expose_secret())
            .expect("session credential should resolve")
            .context;
        assert!(context.can_control());
        assert!(context.can_protected_control());
        assert!(!context.security_enabled());

        let response = router_with_security_state(state)
            .oneshot(with_connect_info(
                with_bearer(
                    Request::builder().uri("/api/v1/protected-control"),
                    credential.expose_secret(),
                )
                .body(Body::empty())
                .expect("failed to build request"),
                IpAddr::V4(Ipv4Addr::LOCALHOST),
                1042,
            ))
            .await
            .expect("request failed");

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn nonloopback_session_credential_is_rejected_when_public_auth_is_disabled() {
        let credential = ProtectedControlCredential::from_bytes([0x24; 32]);
        let state = SecurityState::with_session_credential(credential.clone());
        let response = router_with_security_state(state)
            .oneshot(with_connect_info(
                with_bearer(
                    Request::builder().uri("/api/v1/protected-control"),
                    credential.expose_secret(),
                )
                .body(Body::empty())
                .expect("failed to build request"),
                IpAddr::V4(Ipv4Addr::new(203, 0, 113, 9)),
                1042,
            ))
            .await
            .expect("request failed");

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn loopback_proxy_with_forwarded_remote_ip_requires_authentication() {
        let app = secured_test_router();
        let response = app
            .oneshot(with_connect_info(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/scenes")
                    .header("x-forwarded-for", "203.0.113.77")
                    .body(Body::empty())
                    .expect("failed to build request"),
                IpAddr::V4(Ipv4Addr::LOCALHOST),
                1042,
            ))
            .await
            .expect("request failed");

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn forwarded_loopback_header_does_not_bypass_remote_auth() {
        let app = secured_test_router();
        let response = app
            .oneshot(with_connect_info(
                Request::builder()
                    .uri("/api/v1/devices")
                    .header("x-forwarded-for", "127.0.0.1")
                    .body(Body::empty())
                    .expect("failed to build request"),
                IpAddr::V4(Ipv4Addr::new(203, 0, 113, 9)),
                1042,
            ))
            .await
            .expect("request failed");

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn unsecured_loopback_cross_site_effect_installs_are_rejected() {
        let app = router_with_security_state(SecurityState::with_keys(None, None));
        let response = app
            .oneshot(with_connect_info(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/effects/install")
                    .header("sec-fetch-site", "cross-site")
                    .body(Body::empty())
                    .expect("failed to build request"),
                IpAddr::V4(Ipv4Addr::LOCALHOST),
                1042,
            ))
            .await
            .expect("request failed");

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let json = response_json(response).await;
        assert_eq!(json["error"]["code"], "forbidden");
    }

    #[tokio::test]
    async fn loopback_cross_site_mutating_requests_are_rejected() {
        let app = secured_test_router();
        let response = app
            .oneshot(with_connect_info(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/scenes")
                    .header("sec-fetch-site", "cross-site")
                    .body(Body::empty())
                    .expect("failed to build request"),
                IpAddr::V4(Ipv4Addr::LOCALHOST),
                1042,
            ))
            .await
            .expect("request failed");

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let json = response_json(response).await;
        assert_eq!(json["error"]["code"], "forbidden");
    }

    #[tokio::test]
    async fn tauri_cross_site_bypass_requires_exact_origin_and_current_session() {
        let credential = ProtectedControlCredential::from_bytes([0x63; 32]);
        let session = credential.expose_secret();
        let session_state = || SecurityState::with_session_credential(credential.clone());

        assert_eq!(
            loopback_cross_site_mutation(
                session_state(),
                Some("tauri://localhost"),
                Some(session),
            )
            .await,
            StatusCode::CREATED
        );
        assert_eq!(
            loopback_cross_site_mutation(session_state(), Some("tauri://localhost"), None).await,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            loopback_cross_site_mutation(
                session_state(),
                Some("tauri://attacker.example"),
                Some(session),
            )
            .await,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            loopback_cross_site_mutation(
                session_state(),
                Some("https://tauri.localhost.evil"),
                Some(session),
            )
            .await,
            StatusCode::FORBIDDEN
        );

        let response = router_with_security_state(session_state())
            .oneshot(with_connect_info(
                with_bearer(
                    Request::builder().method("POST").uri("/api/v1/scenes"),
                    session,
                )
                .body(Body::empty())
                .expect("native request should build"),
                IpAddr::V4(Ipv4Addr::LOCALHOST),
                1042,
            ))
            .await
            .expect("native request failed");
        assert_eq!(response.status(), StatusCode::CREATED);

        let mut public_key_state = SecurityState::with_keys(Some(CONTROL_KEY), None);
        public_key_state.launcher_session_credential = Some(credential);
        assert_eq!(
            loopback_cross_site_mutation(
                public_key_state,
                Some("tauri://localhost"),
                Some(CONTROL_KEY),
            )
            .await,
            StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn loopback_same_site_mutating_requests_are_allowed() {
        let app = secured_test_router();
        let response = app
            .oneshot(with_connect_info(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/scenes")
                    .header("sec-fetch-site", "same-origin")
                    .body(Body::empty())
                    .expect("failed to build request"),
                IpAddr::V4(Ipv4Addr::LOCALHOST),
                1042,
            ))
            .await
            .expect("request failed");

        assert_ne!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn read_key_can_access_read_endpoint() {
        let app = secured_test_router();
        let response = app
            .oneshot(
                with_bearer(Request::builder().uri("/api/v1/devices"), READ_KEY)
                    .body(Body::empty())
                    .expect("failed to build request"),
            )
            .await
            .expect("request failed");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["x-ratelimit-limit"], "120");
        assert_eq!(response.headers()["x-ratelimit-remaining"], "119");
        assert!(response.headers().contains_key("x-ratelimit-reset"));
    }

    #[tokio::test]
    async fn read_key_cannot_access_write_endpoint() {
        let app = secured_test_router();
        let response = app
            .oneshot(
                with_bearer(
                    Request::builder().method("POST").uri("/api/v1/scenes"),
                    READ_KEY,
                )
                .body(Body::empty())
                .expect("failed to build request"),
            )
            .await
            .expect("request failed");

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let json = response_json(response).await;
        assert_eq!(json["error"]["code"], "forbidden");
    }

    #[tokio::test]
    async fn control_key_can_access_write_endpoint() {
        let app = secured_test_router();
        let response = app
            .oneshot(
                with_bearer(
                    Request::builder().method("POST").uri("/api/v1/scenes"),
                    CONTROL_KEY,
                )
                .body(Body::empty())
                .expect("failed to build request"),
            )
            .await
            .expect("request failed");

        assert_eq!(response.status(), StatusCode::CREATED);
        assert_eq!(response.headers()["x-ratelimit-limit"], "60");
        assert_eq!(response.headers()["x-ratelimit-remaining"], "59");
    }

    #[tokio::test]
    async fn rejects_query_token_authentication_for_http_endpoints() {
        let app = secured_test_router();
        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/devices?token={READ_KEY}"))
                    .body(Body::empty())
                    .expect("failed to build request"),
            )
            .await
            .expect("request failed");

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn websocket_upgrade_read_query_lacks_protected_control() {
        let app = secured_test_router();
        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/ws?token={READ_KEY}"))
                    .header("upgrade", "websocket")
                    .body(Body::empty())
                    .expect("failed to build request"),
            )
            .await
            .expect("request failed");

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert!(response.headers().contains_key("x-ratelimit-limit"));
    }

    #[tokio::test]
    async fn websocket_upgrade_control_query_grants_protected_control() {
        let response = secured_test_router()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/ws?token={CONTROL_KEY}"))
                    .header("upgrade", "websocket")
                    .body(Body::empty())
                    .expect("failed to build request"),
            )
            .await
            .expect("request failed");

        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().contains_key("x-ratelimit-limit"));
    }

    #[tokio::test]
    async fn loopback_websocket_control_query_grants_protected_control() {
        let response = secured_test_router()
            .oneshot(with_connect_info(
                Request::builder()
                    .uri(format!("/api/v1/ws?token={CONTROL_KEY}"))
                    .header("upgrade", "websocket")
                    .body(Body::empty())
                    .expect("failed to build request"),
                IpAddr::V4(Ipv4Addr::LOCALHOST),
                1042,
            ))
            .await
            .expect("request failed");

        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().get("x-ratelimit-limit").is_none());
    }

    #[tokio::test]
    async fn loopback_websocket_session_query_grants_protected_control() {
        let credential = ProtectedControlCredential::from_bytes([0x81; 32]);
        let response =
            router_with_security_state(SecurityState::with_session_credential(credential.clone()))
                .oneshot(with_connect_info(
                    Request::builder()
                        .uri(format!("/api/v1/ws?token={}", credential.expose_secret()))
                        .header("upgrade", "websocket")
                        .body(Body::empty())
                        .expect("failed to build request"),
                    IpAddr::V4(Ipv4Addr::LOCALHOST),
                    1042,
                ))
                .await
                .expect("request failed");

        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().get("x-ratelimit-limit").is_none());
    }

    #[tokio::test]
    async fn network_allowlist_allows_configured_cidr() {
        let app = allowlist_test_router(vec!["192.168.1.0/24".to_owned()]);
        let response = app
            .oneshot(with_connect_info(
                Request::builder()
                    .uri("/api/v1/devices")
                    .body(Body::empty())
                    .expect("failed to build request"),
                IpAddr::V4(Ipv4Addr::new(192, 168, 1, 42)),
                1042,
            ))
            .await
            .expect("request failed");

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn network_allowlist_rejects_clients_outside_configured_cidr() {
        let app = allowlist_test_router(vec!["192.168.1.0/24".to_owned()]);
        let response = app
            .oneshot(with_connect_info(
                Request::builder()
                    .uri("/api/v1/devices")
                    .body(Body::empty())
                    .expect("failed to build request"),
                IpAddr::V4(Ipv4Addr::new(192, 168, 2, 42)),
                2042,
            ))
            .await
            .expect("request failed");

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let json = response_json(response).await;
        assert_eq!(json["error"]["code"], "forbidden");
    }

    #[tokio::test]
    async fn network_allowlist_uses_derived_client_ip_not_forwarded_header_from_remote_peers() {
        let app = allowlist_test_router(vec!["203.0.113.5".to_owned()]);
        let response = app
            .oneshot(with_connect_info(
                Request::builder()
                    .uri("/api/v1/devices")
                    .header("x-forwarded-for", "203.0.113.5")
                    .body(Body::empty())
                    .expect("failed to build request"),
                IpAddr::V4(Ipv4Addr::new(203, 0, 113, 99)),
                1042,
            ))
            .await
            .expect("request failed");

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let json = response_json(response).await;
        assert_eq!(json["error"]["details"]["client_ip"], "203.0.113.99");
    }

    #[tokio::test]
    async fn lan_trusted_mode_allows_only_local_subnet_clients() {
        let config = NetworkConfig {
            access_mode: NetworkAccessMode::LanTrusted,
            client_scope: NetworkClientScope::LocalSubnets,
            ..NetworkConfig::default()
        };
        let app = network_policy_test_router(
            &config,
            vec![ClientAddressRule::Cidr {
                network: IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10)),
                prefix: 24,
            }],
        );

        let allowed = app
            .clone()
            .oneshot(with_connect_info(
                Request::builder()
                    .uri("/api/v1/devices")
                    .body(Body::empty())
                    .expect("failed to build request"),
                IpAddr::V4(Ipv4Addr::new(192, 168, 1, 42)),
                3042,
            ))
            .await
            .expect("request failed");
        let rejected = app
            .oneshot(with_connect_info(
                Request::builder()
                    .uri("/api/v1/devices")
                    .body(Body::empty())
                    .expect("failed to build request"),
                IpAddr::V4(Ipv4Addr::new(192, 168, 2, 42)),
                3043,
            ))
            .await
            .expect("request failed");

        assert_eq!(allowed.status(), StatusCode::OK);
        assert_eq!(rejected.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn private_range_scope_rejects_public_clients() {
        let config = NetworkConfig {
            access_mode: NetworkAccessMode::LanTrusted,
            client_scope: NetworkClientScope::PrivateRanges,
            ..NetworkConfig::default()
        };
        let app = network_policy_test_router(&config, Vec::new());

        let allowed = app
            .clone()
            .oneshot(with_connect_info(
                Request::builder()
                    .uri("/api/v1/devices")
                    .body(Body::empty())
                    .expect("failed to build request"),
                IpAddr::V4(Ipv4Addr::new(10, 0, 0, 42)),
                4042,
            ))
            .await
            .expect("request failed");
        let rejected = app
            .oneshot(with_connect_info(
                Request::builder()
                    .uri("/api/v1/devices")
                    .body(Body::empty())
                    .expect("failed to build request"),
                IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)),
                4043,
            ))
            .await
            .expect("request failed");

        assert_eq!(allowed.status(), StatusCode::OK);
        assert_eq!(rejected.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn custom_mode_uses_explicit_allowed_clients_only() {
        let config = NetworkConfig {
            access_mode: NetworkAccessMode::Custom,
            client_scope: NetworkClientScope::PrivateRanges,
            allowed_clients: vec!["203.0.113.0/24".to_owned()],
            ..NetworkConfig::default()
        };
        let app = network_policy_test_router(&config, Vec::new());

        let rejected = app
            .oneshot(with_connect_info(
                Request::builder()
                    .uri("/api/v1/devices")
                    .body(Body::empty())
                    .expect("failed to build request"),
                IpAddr::V4(Ipv4Addr::new(10, 0, 0, 42)),
                5042,
            ))
            .await
            .expect("request failed");

        assert_eq!(rejected.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn discovery_rate_limit_is_global() {
        let app = secured_test_router();

        let first = app
            .clone()
            .oneshot(with_connect_info(
                with_bearer(
                    Request::builder()
                        .method("POST")
                        .uri("/api/v1/devices/discover"),
                    CONTROL_KEY,
                )
                .body(Body::empty())
                .expect("failed to build request"),
                IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
                1001,
            ))
            .await
            .expect("first request failed");

        let second = app
            .clone()
            .oneshot(with_connect_info(
                with_bearer(
                    Request::builder()
                        .method("POST")
                        .uri("/api/v1/devices/discover"),
                    CONTROL_KEY,
                )
                .body(Body::empty())
                .expect("failed to build request"),
                IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)),
                1002,
            ))
            .await
            .expect("second request failed");

        let third = app
            .oneshot(with_connect_info(
                with_bearer(
                    Request::builder()
                        .method("POST")
                        .uri("/api/v1/devices/discover"),
                    CONTROL_KEY,
                )
                .body(Body::empty())
                .expect("failed to build request"),
                IpAddr::V4(Ipv4Addr::new(10, 0, 0, 3)),
                1003,
            ))
            .await
            .expect("third request failed");

        assert_eq!(first.status(), StatusCode::ACCEPTED);
        assert_eq!(second.status(), StatusCode::ACCEPTED);
        assert_eq!(third.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(third.headers()["x-ratelimit-limit"], "2");
        assert_eq!(third.headers()["x-ratelimit-remaining"], "0");
        assert!(third.headers().contains_key("retry-after"));

        let json = response_json(third).await;
        assert_eq!(json["error"]["code"], "rate_limited");
    }

    #[tokio::test]
    async fn pairing_rate_limit_is_scoped_per_client() {
        let app = secured_test_router();

        for _ in 0..super::PAIRING_LIMIT_PER_MIN {
            let response = app
                .clone()
                .oneshot(with_connect_info(
                    with_bearer(
                        Request::builder()
                            .method("POST")
                            .uri("/api/v1/devices/device-1/pair"),
                        CONTROL_KEY,
                    )
                    .body(Body::empty())
                    .expect("failed to build request"),
                    IpAddr::V4(Ipv4Addr::new(10, 0, 0, 10)),
                    1010,
                ))
                .await
                .expect("pairing request failed");
            assert_eq!(response.status(), StatusCode::OK);
        }

        let limited = app
            .oneshot(with_connect_info(
                with_bearer(
                    Request::builder()
                        .method("DELETE")
                        .uri("/api/v1/devices/device-1/pair"),
                    CONTROL_KEY,
                )
                .body(Body::empty())
                .expect("failed to build request"),
                IpAddr::V4(Ipv4Addr::new(10, 0, 0, 10)),
                1010,
            ))
            .await
            .expect("limited pairing request failed");

        assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(limited.headers()["x-ratelimit-limit"], "6");
        assert_eq!(limited.headers()["x-ratelimit-remaining"], "0");
    }

    #[test]
    fn rate_limiter_evicts_stale_clients() {
        let mut limiter = super::RateLimiter::new();
        limiter.clients.insert(
            "stale".to_owned(),
            super::ClientWindow {
                window_start: std::time::Instant::now()
                    .checked_sub(super::RATE_WINDOW + std::time::Duration::from_secs(1))
                    .expect("duration should be representable"),
                read_count: 1,
                write_count: 0,
                pairing_count: 0,
            },
        );

        let decision = limiter.check_and_record("fresh", super::OperationClass::Read);

        assert!(decision.allowed);
        assert!(!limiter.clients.contains_key("stale"));
        assert!(limiter.clients.contains_key("fresh"));
    }

    #[test]
    fn nonexistent_bulk_route_is_treated_as_write() {
        assert_eq!(
            super::classify_operation(&Method::POST, "/api/v1/bulk"),
            super::OperationClass::Write
        );
    }

    #[test]
    fn generic_pair_route_is_classified_as_pairing() {
        assert_eq!(
            super::classify_operation(&Method::POST, "/api/v1/devices/abc123/pair"),
            super::OperationClass::Pairing
        );
        assert_eq!(
            super::classify_operation(&Method::DELETE, "/api/v1/devices/abc123/pair"),
            super::OperationClass::Pairing
        );
    }

    #[test]
    fn forwarded_headers_are_ignored_for_non_loopback_peers() {
        let request = Request::builder()
            .uri("/api/v1/devices")
            .header("x-forwarded-for", "203.0.113.50")
            .body(Body::empty())
            .expect("failed to build request");
        let request = with_connect_info(request, IpAddr::V4(Ipv4Addr::new(10, 1, 2, 3)), 9420);

        assert_eq!(super::client_identity(&request), "10.1.2.3");
    }

    #[test]
    fn forwarded_headers_are_honored_for_loopback_proxy_peers() {
        let request = Request::builder()
            .uri("/api/v1/devices")
            .header("x-forwarded-for", "203.0.113.50")
            .body(Body::empty())
            .expect("failed to build request");
        let request = with_connect_info(request, IpAddr::V4(Ipv4Addr::LOCALHOST), 9420);

        assert_eq!(super::client_identity(&request), "203.0.113.50");
    }

    struct PublicPairExtension;

    impl crate::extensions::ApiExtension for PublicPairExtension {
        fn name(&self) -> &'static str {
            "public-pair"
        }

        fn mount_api_routes(
            &self,
            router: utoipa_axum::router::OpenApiRouter<std::sync::Arc<crate::app_state::AppState>>,
        ) -> utoipa_axum::router::OpenApiRouter<std::sync::Arc<crate::app_state::AppState>>
        {
            router
        }

        fn public_routes(&self) -> Vec<crate::extensions::PublicRoute> {
            vec![crate::extensions::PublicRoute::new(
                Method::POST,
                "/ext/pair",
                crate::extensions::PublicRateClass::Pairing,
            )]
        }
    }

    #[tokio::test]
    async fn a_public_route_still_rejects_a_network_session_credential() {
        let credential = ProtectedControlCredential::from_bytes([0x42; 32]);
        let extensions: Vec<std::sync::Arc<dyn crate::extensions::ApiExtension>> =
            vec![std::sync::Arc::new(PublicPairExtension)];
        let state = SecurityState::with_session_credential(credential.clone()).with_public_routes(
            super::PublicRouteTable::from_extensions(&extensions, "/api/v1", &[], &[]),
        );
        let app = Router::new()
            .route("/api/v1/ext/pair", post(|| async { StatusCode::OK }))
            .layer(axum::middleware::from_fn_with_state(
                state,
                enforce_security,
            ));
        let lan = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20));

        let anonymous = app
            .clone()
            .oneshot(with_connect_info(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/ext/pair")
                    .body(Body::empty())
                    .expect("failed to build request"),
                lan,
                41_000,
            ))
            .await
            .expect("request should complete");
        assert_eq!(anonymous.status(), StatusCode::OK);

        let session = app
            .oneshot(with_connect_info(
                with_bearer(
                    Request::builder()
                        .method(Method::POST)
                        .uri("/api/v1/ext/pair"),
                    credential.expose_secret(),
                )
                .body(Body::empty())
                .expect("failed to build request"),
                lan,
                41_000,
            ))
            .await
            .expect("request should complete");
        assert_eq!(session.status(), StatusCode::UNAUTHORIZED);
    }

    struct MountProbeExtension;

    impl crate::extensions::ApiExtension for MountProbeExtension {
        fn name(&self) -> &'static str {
            "mount-probe"
        }

        fn mount_api_routes(
            &self,
            router: utoipa_axum::router::OpenApiRouter<std::sync::Arc<crate::app_state::AppState>>,
        ) -> utoipa_axum::router::OpenApiRouter<std::sync::Arc<crate::app_state::AppState>>
        {
            router
        }

        fn public_routes(&self) -> Vec<crate::extensions::PublicRoute> {
            [
                "/agents",
                "/agents/sse",
                "/agentsx",
                "/docs",
                "/docs/exchange",
                "/openapi.json",
            ]
            .into_iter()
            .map(|path| {
                crate::extensions::PublicRoute::new(
                    Method::GET,
                    path,
                    crate::extensions::PublicRateClass::Read,
                )
            })
            .collect()
        }
    }

    #[test]
    fn public_declarations_inside_a_reserved_engine_mount_are_dropped() {
        let extensions: Vec<std::sync::Arc<dyn crate::extensions::ApiExtension>> =
            vec![std::sync::Arc::new(MountProbeExtension)];
        let table = super::PublicRouteTable::from_extensions(
            &extensions,
            "/api/v1",
            &[],
            &["/api/v1/agents".to_owned()],
        );

        assert_eq!(table.class_for(&Method::GET, "/api/v1/agents"), None);
        assert_eq!(table.class_for(&Method::GET, "/api/v1/agents/sse"), None);
        assert_eq!(
            table.class_for(&Method::GET, "/api/v1/agentsx"),
            Some(super::OperationClass::Read),
            "the reserved prefix is segment-aware"
        );
        // The bearer-exempt docs paths are reserved even when unlisted.
        for path in [
            "/api/v1/docs",
            "/api/v1/docs/exchange",
            "/api/v1/openapi.json",
        ] {
            assert_eq!(table.class_for(&Method::GET, path), None, "{path}");
        }
    }

    #[tokio::test]
    async fn a_bearer_exempt_path_drops_an_attached_grant() {
        let app = Router::new()
            .route(
                "/health",
                get(
                    |grant: Option<Extension<super::CredentialGrant>>| async move {
                        axum::Json(serde_json::json!({ "grant": grant.is_some() }))
                    },
                ),
            )
            .layer(axum::middleware::from_fn_with_state(
                SecurityState::with_keys(Some(CONTROL_KEY), None),
                enforce_security,
            ));
        let mut request = Request::builder()
            .uri("/health")
            .body(Body::empty())
            .expect("failed to build request");
        request.extensions_mut().insert(super::CredentialGrant::new(
            super::CredentialTier::Control,
            "replayed",
            tokio_util::sync::CancellationToken::new(),
        ));

        let response = app
            .oneshot(with_connect_info(
                request,
                IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20)),
                41_000,
            ))
            .await
            .expect("request should complete");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response_json(response).await["grant"], false);
    }

    #[tokio::test]
    async fn a_bearer_exempt_path_never_confers_loopback_locality() {
        let app = Router::new()
            .route(
                "/health",
                get(
                    |Extension(context): Extension<RequestAuthContext>| async move {
                        axum::Json(serde_json::json!({ "is_loopback": context.is_loopback() }))
                    },
                ),
            )
            .layer(axum::middleware::from_fn_with_state(
                SecurityState::with_keys(Some(CONTROL_KEY), None),
                enforce_security,
            ));

        let response = app
            .oneshot(with_connect_info(
                Request::builder()
                    .uri("/health")
                    .header("sec-fetch-site", "cross-site")
                    .body(Body::empty())
                    .expect("failed to build request"),
                IpAddr::V4(Ipv4Addr::LOCALHOST),
                41_000,
            ))
            .await
            .expect("request should complete");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response_json(response).await["is_loopback"], false);
    }
}
