+++
title = "Docker"
description = "Run Hypercolor on a Linux server with Docker, the web UI, HTML effects, and network lighting."
weight = 35
template = "page.html"
+++

Run Hypercolor in Docker when your lights connect over the network and you want
to keep the daemon on a Linux server or NAS. The image includes the daemon,
CLI, web UI, and bundled HTML effects. Linux amd64 and arm64 use the same
release binaries as the native packages.

## Start with Compose

Docker images are published with releases that include container support.
Check the [available images](https://github.com/hyperb1iss/hypercolor/pkgs/container/hypercolor)
before starting. If no public image is available, use the
[local build instructions](#build-and-verify-an-image-locally).

Install Docker Engine and the Compose plugin on a Linux host, then download
the Compose file into its own directory:

```bash
mkdir -p hypercolor-docker
cd hypercolor-docker
curl -fsSLo compose.yaml \
  https://raw.githubusercontent.com/hyperb1iss/hypercolor/main/packaging/docker/compose.yaml
umask 077
printf 'HYPERCOLOR_API_KEY=%s\n' "$(openssl rand -hex 32)" > .env
docker compose up -d
```

Open `http://<server-ip>:9420` and enter the key from `.env` when the web UI
asks for a **Network API Key**. The daemon listens on the host's network
interfaces; the key protects control requests. Keep access on your trusted
LAN, or use a VPN or an authenticated HTTPS proxy for remote access.

The image is `ghcr.io/hyperb1iss/hypercolor:latest`. Pin a release by adding
`HYPERCOLOR_VERSION=X.Y.Z` to `.env` (replace `X.Y.Z` with an available
release). Prerelease images use their full version, such as `X.Y.Z-rc.1`, and
never replace `latest`.

Check startup and health:

```bash
docker compose logs -f hypercolor
docker compose ps
curl -fsS http://localhost:9420/health
```

## Connect WLED

The Compose setup uses Linux
[host networking](https://docs.docker.com/engine/network/drivers/host/), so
Hypercolor can receive LAN discovery traffic and stream directly to your
controllers. No port mappings are needed. Port 9420 must be free on the host;
stop an existing native Hypercolor service before starting the container.

WLED normally appears through mDNS discovery. If your network blocks mDNS,
open **Settings → Discovery → WLED** and add the controller to **Known IPs**.
Allow HTTP traffic to the controller and UDP port 4048 for DDP streaming
(UDP 5568 if you select E1.31). Enable WLED's realtime receiver, then follow
the [WLED guide](@/hardware/wled.md) and
[First launch](@/guide/first-launch.md) to choose an effect and layout.

Host networking shares the host's ports and network namespace. A Docker
bridge setup can reach known WLED IPs, but publishing port 9420 alone does
not make mDNS discovery work across the bridge. Docker Desktop has different
host-network behavior; the supported Compose setup targets Docker Engine on
Linux.

## Rendering and host integration

HTML effects run headlessly without a display server. The default container
has no GPU device mapping and uses software rendering. Software rendering
costs CPU, especially for complex effects, large canvases, and several
layers. The normal FPS and canvas controls still apply.

To give Hypercolor access to a Linux GPU, download the optional overlay:

```bash
curl -fsSLo compose.gpu.yaml \
  https://raw.githubusercontent.com/hyperb1iss/hypercolor/main/packaging/docker/compose.gpu.yaml
```

Check the device group IDs on the Docker host:

```bash
ls -l /dev/dri
stat -c '%g' /dev/dri/renderD128
stat -c '%g' /dev/dri/card0
```

Add the render node's group ID as `HOST_RENDER_GID` in `.env`, and the card
node's group ID as `HOST_VIDEO_GID`. If your host names the nodes differently,
use those nodes when checking the IDs. Add this line to `.env` as well:

```dotenv
COMPOSE_FILE=compose.yaml:compose.gpu.yaml
```

Compose will then use both files for startup, upgrades, and shutdown. Start
the container:

```bash
docker compose up -d
```

The overlay passes `/dev/dri` and the host's device groups to the nonroot
container. Hardware acceleration still depends on a compatible GPU and
driver; exposing a device does not guarantee an accelerated renderer.

The supplied container setup provides network lighting. Host screen capture,
system audio capture, keyboard and mouse input, desktop tray integration,
USB lighting, and motherboard RGB need host devices or desktop services that
the Compose setup does not expose. Choose a
[native install](@/guide/choose-your-install.md) when those features matter.
The host's USB devices can still appear through sysfs discovery, but they
cannot connect without access to their device nodes.

## Persistent data and upgrades

The named volume holds `/var/lib/hypercolor`, including configuration,
scenes, layouts, driver credentials, state history, and cache. The daemon runs
as UID and GID `10001`; Docker initializes a fresh named volume with the image
directory's ownership. If you replace the volume with a bind mount, create
the directory with matching ownership first.

The config file is
`/var/lib/hypercolor/config/hypercolor/hypercolor.toml`. Changes made through
the web UI persist in the volume. The `.env` file stays outside the container
and must also be kept if you want to retain the same API key.

Upgrade the image and recreate the container:

```bash
docker compose pull
docker compose up -d
```

For a pinned deployment, change `HYPERCOLOR_VERSION` before pulling. Use the
same Compose directory and volume when upgrading. Docker does not update a
running container automatically.

Stop and remove the container with `docker compose down`; the named volume
remains. Back up that volume before migrating or restoring a deployment.
Running `docker compose down --volumes` deletes the stored configuration and
lighting state.

## Build and verify an image locally

The Dockerfile takes an extracted Linux release bundle as its build context.
It packages the existing binaries and assets without building Rust inside
Docker. From a source checkout, extract the matching release tarball and run:

```bash
mkdir -p docker-context
tar -xzf hypercolor-X.Y.Z-linux-amd64.tar.gz \
  --strip-components=1 -C docker-context
docker build -f packaging/docker/Dockerfile \
  --build-arg VERSION=X.Y.Z \
  -t hypercolor-container:proof docker-context
node scripts/tests/docker-smoke.mjs hypercolor-container:proof
```

Use the arm64 tarball on an arm64 host. The smoke test starts an isolated
WLED emulator, checks HTML rendering and UDP pixel output, restarts the
container to check saved state, and verifies graceful shutdown. Release CI
runs the same check on both native architectures before publishing the
multiarch image.
