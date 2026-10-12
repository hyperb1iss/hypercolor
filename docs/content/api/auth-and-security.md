+++
title = "Auth & security"
description = "Dual-key API auth, protected local controls, CORS, network allowlists, and rate limiting for the Hypercolor daemon on :9420."
weight = 60
template = "page.html"
+++

The daemon ships **open for ordinary control on loopback and closed to the
network**. Local clients on `127.0.0.1` (CLI, TUI, web UI, an MCP client on the
same box) can manage lighting with no credentials. Privacy-bearing capture and
input operations always require an authenticated control credential, including
on loopback. Every off-host request is also gated by API-key authentication, a
per-client allowlist, CORS, and rate limiting. One Axum middleware
(`enforce_security`) establishes both the ordinary API tier and the separate
protected-control authority for the whole `/api/v1` surface.

If you only ever drive Hypercolor from the same machine, you can stop reading
after the loopback section. Everything else matters the moment you bind the
daemon to a LAN address or put it behind a reverse proxy.

{% <callout type="info"> %}
Ordinary API authentication is **opt-in**. With no API-key environment
variables set, loopback lighting control remains credentialless and remote
clients are governed by the network allowlist (default: local-only). Protected
capture and input surfaces remain unavailable until a control credential or
trusted in-process capability is present.
{% </callout> %}

## The model at a glance

A request flows through these checks in order. The first one that fails returns
immediately with a `{ error, meta }` envelope (see
[Envelope & errors](@/api/rest-envelope-and-errors.md)).

{% <mermaid> %}
graph TD
  A[Incoming request] --> B{Allowed by network policy?}
  B -- no --> R1[403 forbidden]
  B -- yes --> C{Bearer-exempt path?}
  C -- yes --> P[Handler]
  C -- no --> C2{Declared public route?}
  C2 -- yes --> L{Under its class limit?}
  L -- no --> R5
  L -- yes --> P
  C2 -- no --> D{Loopback client?}
  D -- yes --> E{Cross-site mutating request?}
  E -- yes --> R2[403 forbidden - CSRF]
  E -- no --> J{Protected capture or input?}
  D -- no --> F{Auth enabled?}
  F -- no --> J
  F -- yes --> G{Valid Bearer token?}
  G -- no --> R3[401 unauthorized]
  G -- yes --> H{Tier satisfies method?}
  H -- no --> R4[403 forbidden]
  H -- yes --> I{Under rate limit?}
  I -- no --> R5[429 rate_limited]
  I -- yes --> J
  J -- no --> P
  J -- yes --> K{Authenticated control authority?}
  K -- no --> R6[403 forbidden]
  K -- yes --> P
{% </mermaid> %}

## Dual-key authentication

Authentication is configured entirely through two environment variables read at
daemon startup:

| Variable | Tier | Grants |
| --- | --- | --- |
| `HYPERCOLOR_API_KEY` | Control | Read **and** write (every method) |
| `HYPERCOLOR_READ_API_KEY` | Read | `GET`, `HEAD`, `OPTIONS` only |

Authentication is **enabled** when either variable holds a non-blank value, or
when an embedding build installs a credential authority (below).
Whitespace-only values are treated as unset. With neither set and no
authority, the API-key gate is bypassed and only the network allowlist
applies.

```bash
# Control tier only: one key that can do everything
HYPERCOLOR_API_KEY="hc_ak_super_secret" hypercolor-daemon

# Split tiers: a write key plus a read-only key for dashboards/scripts
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
- A token matching `HYPERCOLOR_API_KEY` normally grants the **control** tier,
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

### Credential authorities

A build that embeds the daemon can install a credential authority
(`hypercolor_daemon::api::security::CredentialAuthority`, supplied through
its `DaemonExtensionInstaller`) to issue credentials beyond the two
variables. The middleware checks a presented token against the environment
keys first, then against the authority, on loopback and network requests
alike. Authority credentials grant read or control, never protected control,
which stays with the control key, the launcher session, and trusted
in-process callers.

An installed authority turns authentication on for every non-loopback request
even before it holds a single credential, so it admits nobody until it issues
one. An authority that can grant control satisfies the network bind rule the
way `HYPERCOLOR_API_KEY` does (see
[Fail-closed startup binds](#fail-closed-startup-binds)). Revoking one of its
credentials closes every WebSocket that credential opened with close code
1008, reason `credential revoked`.

## The loopback exemption

Requests that arrive straight from a loopback peer (`127.0.0.0/8`, `::1`) skip
the API-key requirement for ordinary lighting control. This is why the CLI,
TUI, web UI, and a local MCP client all work with no key on a default install.
The daemon judges locality from the peer socket. A request a loopback reverse
proxy relayed is never local, whatever address it claims (see
[Behind a reverse proxy](#behind-a-reverse-proxy)), and forwarding headers from
a **non-loopback** peer are ignored, so neither path spoofs its way to the
loopback exemption.

Loopback locality is not a user identity. These surfaces require protected
control authority even when the TCP peer is loopback:

- `POST /api/v1/input/authorize`
- `POST /api/v1/capture/authorize`
- `PUT /api/v1/capture/source`
- `GET /api/v1/capture/monitors`
- WebSocket subscriptions to `screen_canvas`, `screen_zones`, or `input_events`

The system endpoint remains available. Its public identity needs no key, its
status block needs read authority, and capture selection IDs stay redacted
unless the request has protected control authority.

An authenticated control-tier key grants that authority. A read key, a missing
key, an `Origin` header, Fetch Metadata, and the peer IP do not. Trusted
in-process transports receive the same authority only after their own
authentication boundary has succeeded.

### CSRF protection on loopback

Trusting loopback would let a malicious web page in the user's browser issue
drive-by writes to `http://localhost:9420`. To block that, **cross-site
mutating requests to the loopback API are rejected** with `403 forbidden`. The
daemon keys this on the browser-set `Sec-Fetch-Site: cross-site` header and, for
browsers that send no Fetch Metadata (they send none to a plain-HTTP LAN
address), on `Origin`: a write whose `Origin` is neither a loopback origin nor
the daemon's own origin is denied. The bundled web UI (same-origin) and
non-browser clients (CLI, SDK, which send neither header) are unaffected.

{% <callout type="warning"> %}
The CSRF guard fires for **any** mutating loopback request marked cross-site,
even when no API key is configured. A page on another origin cannot `POST` to
your local daemon to install an effect or change a scene. A keyless LAN
listener applies the same guard to every client it admits, since it grants
them the same ambient control.
{% </callout> %}

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
| `network.allowed_hosts` | list | `[]` | Extra `Host` names a request without a key may use, e.g. `["hypercolor.home"]` (see [Host names](#host-names)) |
| `network.allow_unauthenticated_remote_access` | bool | `false` | Permit remote clients without a key (see warning); implied by `lan_trusted`, ignored by `lan_protected` |
| `network.mdns_publish` | bool | `true` | Advertise the daemon over mDNS |

How the scopes resolve when remote access is on:

- **`local_subnets`** trusts the CIDR of every non-loopback interface the host
  currently has. This is the narrowest "let my LAN in" option.
- **`private_ranges`** trusts the RFC 1918 / link-local / ULA ranges:
  `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`, `169.254.0.0/16`, `fc00::/7`,
  `fe80::/10`.
- **`custom`** trusts only the explicit `allowed_clients` you list; no built-in
  scope is added. Listing none blocks every remote client.

The `custom` access mode ignores `client_scope` and trusts only
`allowed_clients`. In the other modes, `allowed_clients` entries are layered
on top of the resolved scope. A non-loopback client whose IP matches no rule
gets `403 forbidden` with a detail body naming the rejected `client_ip`.
A direct loopback peer is exempt from the allowlist in every mode; a request a
loopback proxy relays is judged by the address the proxy names.

The allowlist applies only when it holds at least one rule. When the resolved
scope and `allowed_clients` are both empty, as in `local_only` mode with
`remote_access` off or the `custom` access mode with nothing listed, the
policy admits every client that reaches the daemon, and only an API key, if
one is set, stands between those clients and the API. An `allowed_clients`
entry that fails to parse, a `local_subnets` scope that finds no non-loopback
subnet, or a `custom` client scope that lists nothing blocks every remote
client with `403` instead.

```toml
[network]
access_mode = "lan_trusted"
client_scope = "local_subnets"
allowed_clients = ["192.168.1.0/24"]
```

{% <callout type="danger"> %}
`access_mode = "lan_trusted"` allows **unauthenticated** remote access by
design: any client on the trusted subnets can control your lights without a
key. For a network-reachable daemon you usually want `lan_protected` (remote
allowed, but a key is required) plus `HYPERCOLOR_API_KEY`. Only set
`allow_unauthenticated_remote_access = true` if you genuinely want keyless LAN
control and understand the exposure.
{% </callout> %}

### Fail-closed startup binds

The bind address itself is enforced at startup, so a network-reachable daemon
without auth never comes up by accident. The validation
(`validate_network_bind_auth` in the daemon) refuses to bind the control API
to any non-loopback address unless `HYPERCOLOR_API_KEY` is configured, a
credential authority that can grant control is installed, or the network
config allows keyless remote access: `access_mode = "lan_trusted"`, or
`allow_unauthenticated_remote_access = true` under `local_only` or `custom`
(`lan_protected` ignores that flag). An explicit non-loopback bind flag
(`--bind`, `--listen`, `--listen-all`) without one of these aborts startup
with an error. `--port` is not a bind flag: it moves the port and leaves the
interfaces to config.

A non-loopback target that comes from **config** rather than a flag degrades
instead of aborting: the daemon falls back to loopback at startup and logs a
warning naming the requested and effective targets. Either way the daemon
fails closed. It serves loopback or it serves an authenticated network bind,
never an open one.

Keyless network access you chose on purpose (`lan_trusted`, or
`allow_unauthenticated_remote_access = true` with no API key) still gets
called out. At startup the daemon logs a warning naming every non-loopback
address it listens on while no API key or credential authority is configured
and the network policy admits a remote client of that address's family,
because any device the policy admits can control it.

## Behind a reverse proxy

A reverse proxy on the same host (Caddy, nginx, Traefik) reaches the daemon
from a loopback address, so the daemon reads the client from the proxy's
forwarding headers instead. Any of `X-Forwarded-For`, `X-Real-IP`, or
`Forwarded` on a loopback connection marks the request as relayed, and a
relayed request is treated as a network request:

- It never gets the loopback exemption, even when it claims `127.0.0.1`.
  Without a credential it is refused unless the config opens keyless remote
  access (`access_mode = "lan_trusted"`, or
  `allow_unauthenticated_remote_access = true`), the same rule a network
  listener follows. A local browser that goes through the proxy therefore
  needs an API key; use `http://127.0.0.1:9420` directly to stay keyless.
- The allowlist judges the address the nearest hop wrote: the **last**
  `X-Forwarded-For` entry, `X-Real-IP`, and the last `Forwarded` element's
  `for=`. A proxy appends to `X-Forwarded-For` and passes earlier entries
  through as the client sent them, so only the last one is the proxy's word.
- The daemon cannot tell which header your proxy owns, so when several are
  present, every address they claim must pass the allowlist. A relayed request
  that names no readable address is refused while an allowlist is active.

Configure the proxy to send `X-Forwarded-For` (Caddy and Traefik do by default;
nginx needs `proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;`).
A proxy that names no client (no forwarding headers, or only
`X-Forwarded-Host` and `X-Forwarded-Proto`) is indistinguishable from a local
process: every client it relays gets loopback trust, whether or not an API key
is set. Don't put one in front of the daemon for remote access.

## Host names

A request that presents no credential, whether it rides the loopback
exemption or reaches a keyless LAN listener, must call the daemon by a name an
attacker cannot point at it. This stops DNS rebinding: a page on
`rebind.example` can make the browser resolve its own name to `127.0.0.1` or
the daemon's LAN address, and the browser then treats the daemon as that
page's origin, so `Origin` and `Sec-Fetch-Site` both look same-origin. The
`Host` header still says `rebind.example`, and the daemon refuses it with
`403 forbidden` naming the host.

These names always work:

- Any IP address, such as `127.0.0.1`, `[::1]`, or `192.168.1.10`.
- `localhost` and any name under `.localhost`.
- The machine's host names (from `HOSTNAME`, `COMPUTERNAME`, or
  `/etc/hostname`), each with its `.local` form.
- The name the daemon advertises over mDNS.
- The hosts of `web.cors_origins` entries.

Add any other name you use, such as one your router's DNS hands out, to
`network.allowed_hosts`. Entries match the exact name, ignoring case and port.
`"*"` turns the check off. On macOS the daemon may not learn the machine's
host name, so a `.local` name typed into the browser can need an entry.

A proxy that rewrites `Host` and keeps the browser's name in
`X-Forwarded-Host`, as the UI dev server's proxy does, is checked through that
header as well: every name listed there must pass.

A request that presents an API key or session credential carries its own
authority and is not checked, and neither are the exempt paths (`/health`,
the API docs, and the bundled UI's assets). A request with no `Host` header
at all comes from no browser and passes.

## CORS

The daemon always sends CORS headers for **loopback origins** (`http` or
`https` on `localhost`, a `127.0.0.0/8` address, or `[::1]`, on any port) and
for the desktop shell's Tauri origins. Additional browser origins are honored
only when **API authentication is enabled** and the origin is listed in
`web.cors_origins`:

```toml
[web]
cors_origins = ["https://studio.example.com"]
```

Each configured origin must be a bare `scheme://host[:port]` with an `http` or
`https` scheme and no path; malformed entries are logged and dropped. The
allowed methods are `GET`, `HEAD`, `OPTIONS`, `POST`, `PUT`, `PATCH`, `DELETE`,
and the allowed request headers are `Accept`, `Authorization`, `Content-Type`,
and `If-Match`. Responses expose `ETag` to scripts. When auth is **off**,
configured origins are ignored; only loopback and Tauri origins get CORS,
matching the local-only posture.

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

Some paths do not require a bearer token. Every one of them still passes the
network access policy first, so an exemption is from presenting a key, never
from being allowed to reach the daemon at all. Exempt paths are not rate
limited, which is deliberate: a single page load pulls more asset requests
than the read limit allows.

The health route needs no key in any configuration, so liveness checks work
however authentication is set up. Once startup finishes it passes the network
access policy like every exempt path; while the daemon is still starting, a
remote peer gets a bare `503` and only a loopback peer sees startup progress:

- `GET /health`: liveness probe.

`GET /api/v1/system` still passes network policy and read rate limiting. On a
keyed daemon, an anonymous remote request receives only the public `identity`;
a valid read or control key adds the `status` block. Any supplied invalid key
returns `401 unauthorized`.

Two more are exempt because a browser cannot authenticate them. A page loads
its scripts, stylesheets, and fonts through tags that carry no `Authorization`
header, so a keyed daemon that guarded them would serve an interface no
browser could render:

- The bundled web UI and its assets, whenever a UI directory is configured.
  The UI is served as the router's fallback, so this covers the shell, its
  hashed asset bundles, and every SPA deep link.
- `GET /api/v1/docs` (Swagger UI) and `GET /api/v1/openapi.json`, which the
  docs page fetches as a second request.

The asset exemption is defined by subtraction: a path is an asset when no
dynamic mount claims it. The daemon derives that list where the routes are
mounted, so the API surface, `/health`, and the MCP mount at its configured
base path are never inside it. The UI shell loads without a key; every API
call the UI then makes still needs one.

An embedding build may also declare routes it mounts as public
(`ApiExtension::public_routes`). A declared route needs no key and grants
none: its handler sees an anonymous caller whatever token arrives, and never
a loopback one, because any web page can reach a public route through a
browser on the same machine. It still passes the network policy, a launcher
session credential presented from the network is still refused, and every
caller, loopback included, spends the declared rate class (read, write, or
pairing). A declaration that names a route the engine serves, or falls within
the API docs paths or the MCP mount when that is configured under `/api/v1`,
is ignored at startup with an error, and that route keeps its authentication:
a refused declaration beneath the docs paths also withdraws their exemption
from its exact path. The engine cannot tell one extension's routes from
another's, so an extension must declare only routes it mounts, and must not
mount undeclared routes beneath the docs paths, which the exemption covers
whole.

The MCP server (mounted at `/mcp` when `mcp.enabled` is true) lives outside
`/api/v1` but behind the same security layer: it passes the network policy,
and a remote client of a keyed daemon needs a key. MCP is **off by default**;
enable it before using any agent integration. See [MCP setup](@/api/mcp.md) for the transport and
client configuration.

## WebSocket authentication

The `/api/v1/ws` upgrade is the one endpoint that accepts a token in the query
string, because browsers cannot set custom headers on a WebSocket handshake:

```
ws://studio.local:9420/api/v1/ws?token=hc_ak_super_secret
```

Query-string tokens are accepted **only** on the `GET` WebSocket upgrade. Plain
HTTP endpoints reject `?token=` and demand the `Authorization` header, so a
token never leaks into an ordinary request URL or access log. A loopback socket
needs no token for ordinary channels. The three sensitive channels require a
control token in the upgrade URL or a trusted in-process connection. For the
channel and frame protocol once connected, see
[WebSocket protocol](@/api/websocket.md).

The upgrade also checks the browser `Origin`. An upgrade that sends no
`Origin` header, as native clients do, skips the check. An anonymous upgrade is
admitted only from loopback origins, the desktop shell's Tauri origins, the
daemon's own origin, and, on a keyed daemon, the origins in
`web.cors_origins`: the allowlist keeps other pages from riding the
credentialless loopback tier. A keyed daemon refuses a remote upgrade without a
credential with `401` before the `Origin` check, so `cors_origins` only
matters for browsers on the daemon's own machine.

The daemon's own origin is the one the upgrade was sent to (its `Origin`
matches its `Host`), so the bundled UI opened at a keyless daemon's LAN
address goes live. A rebinding page is same-origin too, but its `Host` is not
one of the daemon's names, so [the Host check](#host-names) refuses it first.

A WebSocket upgrade that presents a valid API key is no longer subject to the
browser origin allowlist. This is a behavior change for every key holder,
environment keys included: such an upgrade is admitted from any `Origin`,
including the daemon's own LAN address when the bundled UI was loaded from
there, where earlier releases refused it with `403`. The key carries its own
authority, so there is no ambient credential for another page to ride.

## Hardening checklist

For a daemon you intend to reach over the network:

1. Set `HYPERCOLOR_API_KEY` to a long random secret, and name read-only keys
   with the `hc_ak_r_` prefix or put them in `HYPERCOLOR_READ_API_KEY`.
2. Choose `access_mode = "lan_protected"` so remote clients must authenticate;
   reserve `lan_trusted` for genuinely keyless LANs.
3. Narrow `client_scope` (prefer `local_subnets`) and add explicit
   `allowed_clients` CIDRs for the hosts that should reach the daemon.
4. List only the exact browser origins you trust in `web.cors_origins`.
5. Terminate TLS at a reverse proxy on the same host that sends
   `X-Forwarded-For`, so the daemon judges the real client (see
   [Behind a reverse proxy](#behind-a-reverse-proxy)).

For the rest of the contract this auth layer guards, see the
[REST API reference](@/api/rest.md), the
[envelope and error codes](@/api/rest-envelope-and-errors.md), and the
[CLI reference](@/api/cli.md).
