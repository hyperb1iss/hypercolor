FROM ubuntu:24.04

ARG VERSION=dev
ARG REVISION=unknown
LABEL org.opencontainers.image.title="Hypercolor" \
      org.opencontainers.image.description="RGB lighting daemon with web UI and HTML effects" \
      org.opencontainers.image.source="https://github.com/hyperb1iss/hypercolor" \
      org.opencontainers.image.licenses="Apache-2.0" \
      org.opencontainers.image.version="${VERSION}" \
      org.opencontainers.image.revision="${REVISION}"

# Mesa's surfaceless EGL backend runs Servo without a desktop or GPU.
# Hardware adapters remain available when the host exposes its render nodes.
RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
        ca-certificates curl fonts-dejavu-core \
        libasound2t64 libdbus-1-3 libegl1 libfontconfig1 libfreetype6 \
        libgl1 libgl1-mesa-dri libgcc-s1 libpipewire-0.3-0t64 libpulse0 \
        libssl3t64 libstdc++6 libudev1 libxcb1 libxkbcommon0 \
        mesa-vulkan-drivers \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 hypercolor \
    && useradd --uid 10001 --gid hypercolor --home-dir /var/lib/hypercolor hypercolor \
    && mkdir -p /var/lib/hypercolor/config /var/lib/hypercolor/data \
        /var/lib/hypercolor/state /var/lib/hypercolor/cache /tmp/hypercolor-runtime \
    && chown -R hypercolor:hypercolor /var/lib/hypercolor /tmp/hypercolor-runtime \
    && chmod 700 /tmp/hypercolor-runtime

# The build context is the extracted, verified Linux release distribution.
COPY bin/hypercolor-daemon bin/hypercolor /usr/local/bin/
COPY share/hypercolor/ /usr/share/hypercolor/

ENV XDG_CONFIG_HOME=/var/lib/hypercolor/config \
    XDG_DATA_HOME=/var/lib/hypercolor/data \
    XDG_STATE_HOME=/var/lib/hypercolor/state \
    XDG_CACHE_HOME=/var/lib/hypercolor/cache \
    XDG_RUNTIME_DIR=/tmp/hypercolor-runtime \
    HYPERCOLOR_EFFECTS_DIR=/usr/share/hypercolor/effects \
    HYPERCOLOR_HEALTH_PORT=9420

USER 10001:10001
WORKDIR /var/lib/hypercolor
EXPOSE 9420/tcp
VOLUME ["/var/lib/hypercolor"]
STOPSIGNAL SIGTERM
HEALTHCHECK --interval=30s --timeout=5s --start-period=30s --retries=3 \
    CMD curl --fail --silent "http://127.0.0.1:${HYPERCOLOR_HEALTH_PORT}/health" > /dev/null || exit 1

ENTRYPOINT ["/usr/local/bin/hypercolor-daemon"]
CMD ["--listen-all", "--ui-dir", "/usr/share/hypercolor/ui"]
