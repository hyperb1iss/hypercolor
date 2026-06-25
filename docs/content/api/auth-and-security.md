+++
title = "Auth & security"
description = "Dual-key API auth, the loopback exemption, CORS, network allowlists, and rate limiting for the Hypercolor daemon on :9420."
weight = 60
template = "page.html"
+++

# Auth & security

The daemon ships **open on loopback and closed to the network**. Local clients
on `127.0.0.1` (CLI, TUI, web UI, an MCP client on the same box) work with no
credentials, while every off-host request is gated by the layers on this page:
API-key authentication, a per-client allowlist, CORS, and rate limiting. All of
it is enforced by a single Axum middleware (`enforce_security`) that wraps the
whole `/api/v1` surface.

If you only ever drive Hypercolor from the same machine, you can stop reading
after the loopback section. Everything else matters the moment you bind the
daemon to a LAN address or put it behind a reverse proxy.

{% callout(type="info") %}
Authentication is **opt-in**. With no API-key environment variables set, the
daemon enforces no keys at all — loopback is trusted and remote clients are
governed only by the network allowlist (default: local-only). Setting a key
flips on the full Bearer-token gate.
{% end %}

## The model at a glance

A request flows through these checks in order. The first one that fails returns
immediately with a `{ error, meta }` envelope (see
[Envelope & errors](@/api/rest-envelope-and-errors.md)).

{% mermaid() %}
graph TD
  A[Incoming request] --> B{Allowed by network policy?}
  B -- no --> R1[403 forbidden]
  B -- yes --> C{Exempt path? /health, /api/v1/server}
  C -- yes --> P[Handler]
  C -- no --> D{Loopback client?}
  D -- yes --> E{Cross-site mutating request?}
  E -- yes --> R2[403 forbidden - CSRF]
  E -- no --> P
  D -- no --> F{Auth enabled?}
  F -- no --> P
  F -- yes --> G{Valid Bearer token?}
  G -- no --> R3[401 unauthorized]
  G -- yes --> H{Tier satisfies method?}
  H -- no --> R4[403 forbidden]
  H -- yes --> I{Under rate limit?}
  I -- no --> R5[429 rate_limited]
  I -- yes --> P
{% end %}

## Dual-key authentication

Authentication is configured entirely through two environment variables read at
daemon startup:

| Variable | Tier | Grants |
| --- | --- | --- |
| `HYPERCOLOR_API_KEY` | Control | Read **and** write (every method) |
| `HYPERCOLOR_READ_API_KEY` | Read | `GET`, `HEAD`, `OPTIONS` only |

Authentication is **enabled** when either variable holds a non-blank value.
Whitespace-only values are treated as unset. With neither set, the API-key gate
is bypassed and only the network allowlist applies.

```bash
# Control tier only — one key that can do everything
HYPERCOLOR_API_KEY="hc_ak_super_secret" hypercolor-daemon

# Split tiers — a write key plus a read-only key for dashboards/scripts
HYPERCOLOR_API_KEY="hc_ak_super_secret" \
HYPERCOLOR_READ_API_KEY="hc_ak_r_dashboard_only" \
  hypercolor-daemon
```

### Bearer scheme

Authenticated requests carry the token in a standard `Authorization` header:

```http
GET /api/v1/devices HTTP/1.1
Host: studio.local:9420
Authorization: Bearer hc_ak_super_secret
```

The scheme keyword is case-insensitive (`Bearer`, `bearer`); the token must be
non-empty. The CLI's `--api-key` flag (env `HYPERCOLOR_API_KEY`) sets this
header for you on every request, so a remote CLI session looks like:

```bash
hypercolor --host studio.local --api-key "hc_ak_super_secret" devices list
```

### Tier resolution and the read-prefix rule

The tier a token grants is resolved against the configured keys:

- A token matching `HYPERCOLOR_READ_API_KEY` always grants the **read** tier.
- A token matching `HYPERCOLOR_API_KEY` normally grants the **control** tier —
  **unless** that key string begins with the prefix `hc_ak_r_`, in which case it
  is treated as read-only even though it sits in the control slot. Use the
  `hc_ak_r_` convention to name keys you intend to be read-only and the daemon
  will enforce that intent.

A read-tier token that hits a mutating method (`POST`/`PUT`/`PATCH`/`DELETE`)
gets `403 forbidden` with a detail body naming the required and current tiers:

```json
{
  "error": {
    "code": "forbidden",
    "message": "Read-only API key cannot perform write operations",
    "details": { "required_tier": "control", "current_tier": "read" }
  },
  "meta": { "api_version": "1.0", "request_id": "req_…", "timestamp": "…Z" }
}
```

A missing or unparseable token on a non-loopback request returns
`401 unauthorized`.

## The loopback exemption

Requests whose client IP is loopback (`127.0.0.0/8`, `::1`) skip the API-key
requirement entirely. This is why the CLI, TUI, web UI, and a local MCP client
all work with no key on a default install. The daemon derives the client IP
from the peer socket; when the peer is itself loopback (a reverse proxy on the
same host) it honors `X-Forwarded-For` / `X-Real-IP` so the real remote IP is
used for auth and allowlisting. Forwarded headers from a **non-loopback** peer
are ignored — you cannot spoof your way to a loopback exemption.

### CSRF protection on loopback

Trusting loopback would let a malicious web page in the user's browser issue
drive-by writes to `http://localhost:9420`. To block that, **cross-site
mutating requests to the loopback API are rejected** with `403 forbidden`. The
daemon keys this on the browser-set `Sec-Fetch-Site: cross-site` header: the
bundled web UI (same-origin) and non-browser clients (CLI, SDK, which omit the
header) are unaffected; only a browser explicitly marking the request
cross-site is denied.

{% callout(type="warning") %}
The CSRF guard fires for **any** mutating loopback request marked cross-site,
even when no API key is configured. A page on another origin cannot `POST` to
your local daemon to install an effect or change a scene.
{% end %}

## Network access policy

Before authentication, every request passes the network allowlist. By default
the daemon is **local-only**: loopback is always allowed, and there is no
allowlist entry for anything else, so off-host requests are simply unreachable
because the daemon binds loopback. Opening the daemon to the LAN is a deliberate
config change under `[network]`.

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `network.access_mode` | enum | `local_only` | `local_only`, `lan_trusted`, `lan_protected`, `custom` |
| `network.client_scope` | enum | `local_subnets` | Which built-in IP scope to trust: `local_subnets`, `private_ranges`, `custom` |
| `network.remote_access` | bool | `false` | Force remote access on without changing `access_mode` |
| `network.allowed_clients` | list | `[]` | Extra exact IPs or CIDR rules, e.g. `["192.168.1.0/24", "10.0.0.5"]` |
| `network.allow_unauthenticated_remote_access` | bool | `false` | Permit remote clients without a key (see warning) |
| `network.mdns_publish` | bool | `true` | Advertise the daemon over mDNS |

How the scopes resolve when remote access is on:

- **`local_subnets`** trusts the CIDR of every non-loopback interface the host
  currently has. This is the narrowest "let my LAN in" option.
- **`private_ranges`** trusts the RFC 1918 / link-local / ULA ranges:
  `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`, `169.254.0.0/16`, `fc00::/7`,
  `fe80::/10`.
- **`custom`** trusts only the explicit `allowed_clients` you list — no built-in
  scope is added.

`allowed_clients` entries are always layered on top of the resolved scope. A
non-loopback client whose IP matches no rule gets `403 forbidden` with a detail
body naming the rejected `client_ip`. Loopback is exempt from the allowlist in
every mode.

```toml
[network]
access_mode = "lan_trusted"
client_scope = "local_subnets"
allowed_clients = ["192.168.1.0/24"]
```

{% callout(type="danger") %}
`access_mode = "lan_trusted"` allows **unauthenticated** remote access by
design — any client on the trusted subnets can control your lights without a
key. For a network-reachable daemon you usually want `lan_protected` (remote
allowed, but a key is required) plus `HYPERCOLOR_API_KEY`. Only set
`allow_unauthenticated_remote_access = true` if you genuinely want keyless LAN
control and understand the exposure.
{% end %}

## CORS

The daemon sends permissive CORS headers for **loopback origins always**
(`http://localhost:*`, `http://127.0.0.1:*`, and the IPv6 loopback). Additional
browser origins are honored only when **API authentication is enabled** and the
origin is listed in `web.cors_origins`:

```toml
[web]
cors_origins = ["https://studio.example.com"]
```

Each configured origin must be a bare `scheme://host[:port]` with an `http` or
`https` scheme and no path; malformed entries are logged and dropped. The
allowed methods are `GET`, `HEAD`, `OPTIONS`, `POST`, `PUT`, `PATCH`, `DELETE`,
and the allowed request headers are `Accept`, `Authorization`, and
`Content-Type`. When auth is **off**, configured origins are ignored — only
loopback gets CORS, matching the local-only posture.

## Rate limiting

Authenticated, non-loopback traffic is rate-limited per client IP over a rolling
60-second window. Limits are tracked separately by operation class:

| Class | Limit / window | Scope |
| --- | --- | --- |
| Read (`GET`/`HEAD`/`OPTIONS`) | 120 | Per client |
| Write (other mutating routes) | 60 | Per client |
| Pairing (`/api/v1/devices/{id}/pair`) | 6 | Per client |
| Discovery (`POST /api/v1/devices/discover`) | 2 | **Global** |

Discovery is intentionally a single global budget: a network rescan is
expensive, so two per minute is the ceiling across all clients combined, not
per client.

Every rate-limited response carries the standard headers, and a rejection adds
`Retry-After`:

```http
X-RateLimit-Limit: 60
X-RateLimit-Remaining: 0
X-RateLimit-Reset: 1750800000
Retry-After: 37
```

Exceeding a limit returns `429` with the `rate_limited` error code:

```json
{
  "error": {
    "code": "rate_limited",
    "message": "Write operation rate limit exceeded. Retry in 37 seconds.",
    "details": { "limit": 60, "window_seconds": 60, "retry_after": 37 }
  },
  "meta": { "api_version": "1.0", "request_id": "req_…", "timestamp": "…Z" }
}
```

## Exempt paths

Two routes bypass the entire security stack — no auth, no rate limiting — so
health checks and instance discovery work regardless of configuration:

- `GET /health` — liveness probe.
- `GET /api/v1/server` — server identity for multi-daemon clients.

The MCP server (mounted at `/mcp` when `mcp.enabled` is true) sits outside the
`/api/v1` middleware stack. MCP is **off by default**; enable it before using
any agent integration. See [MCP setup](@/api/mcp.md) for the transport and
client configuration.

## WebSocket authentication

The `/api/v1/ws` upgrade is the one endpoint that accepts a token in the query
string, because browsers cannot set custom headers on a WebSocket handshake:

```
ws://studio.local:9420/api/v1/ws?token=hc_ak_super_secret
```

Query-string tokens are accepted **only** on the `GET` WebSocket upgrade. Plain
HTTP endpoints reject `?token=` and demand the `Authorization` header, so a
token never leaks into an ordinary request URL or access log. On loopback the
socket needs no token at all. For the channel and frame protocol once
connected, see [WebSocket protocol](@/api/websocket.md).

## Hardening checklist

For a daemon you intend to reach over the network:

1. Set `HYPERCOLOR_API_KEY` to a long random secret, and name read-only keys
   with the `hc_ak_r_` prefix or put them in `HYPERCOLOR_READ_API_KEY`.
2. Choose `access_mode = "lan_protected"` so remote clients must authenticate;
   reserve `lan_trusted` for genuinely keyless LANs.
3. Narrow `client_scope` (prefer `local_subnets`) and add explicit
   `allowed_clients` CIDRs for the hosts that should reach the daemon.
4. List only the exact browser origins you trust in `web.cors_origins`.
5. Terminate TLS at a reverse proxy on the same host so the daemon sees a
   loopback peer and reads the real client IP from `X-Forwarded-For`.

For the rest of the contract this auth layer guards, see the
[REST API reference](@/api/rest.md), the
[envelope and error codes](@/api/rest-envelope-and-errors.md), and the
[CLI reference](@/api/cli.md).
