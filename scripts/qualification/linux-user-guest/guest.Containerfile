# Ordinary-user systemd guest for the Linux release installer.
#
# Ubuntu 24.04 is the release runtime baseline: glibc 2.39 and systemd 255.
# The guest boots systemd as PID 1 under rootless podman; uid 1100 lingers,
# so its user manager starts at boot and autostarts enabled user services
# the way a login would.
FROM docker.io/library/ubuntu@sha256:019e8eb29a85e74d64925745884f2ec79aa27e3feab36353d24656f4d6b89467
ENV container=podman
RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
        systemd systemd-sysv dbus dbus-user-session libpam-systemd \
        python3 curl ca-certificates procps passwd \
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/*
# Principal lookups must enumerate completely; keep them on local files.
RUN sed -i -E 's/^(passwd|group|shadow|gshadow):.*/\1: files/' /etc/nsswitch.conf
RUN useradd --create-home --uid 1100 --shell /bin/bash qualification \
    && mkdir -p /var/lib/systemd/linger \
    && touch /var/lib/systemd/linger/qualification
STOPSIGNAL SIGRTMIN+3
CMD ["/sbin/init"]
