# Baseline builder: compiles the installer CLI and the qualification daemon
# against Ubuntu 24.04's glibc so both run unmodified in the guest.
FROM docker.io/library/ubuntu@sha256:019e8eb29a85e74d64925745884f2ec79aa27e3feab36353d24656f4d6b89467
RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
        build-essential clang lld pkg-config cmake nasm perl python3 git curl \
        ca-certificates libudev-dev libdbus-1-dev libasound2-dev \
        libpipewire-0.3-dev libpulse0 libpulse-dev libfontconfig1-dev libssl-dev libxcb1-dev \
        libxcb-randr0-dev libxcb-shm0-dev libxcb-xfixes0-dev \
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/*
ARG RUST_TOOLCHAIN
ENV RUSTUP_HOME=/opt/rustup \
    CARGO_HOME=/opt/cargo \
    PATH=/opt/cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
RUN test -n "${RUST_TOOLCHAIN}" \
    && curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
        | sh -s -- -y --no-modify-path --profile minimal --default-toolchain "${RUST_TOOLCHAIN}" \
    && rustc --version && ldd --version | head -n 1
WORKDIR /src
