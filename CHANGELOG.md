# Changelog

All notable changes to Hypercolor will be documented here.

This project follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)
and uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Launch hardening branch for v0.1.0 release readiness.

## [0.1.0] - Unreleased

### Added

- Linux-first RGB lighting daemon with REST, WebSocket, and MCP control surfaces.
- Servo HTML effect renderer for Canvas, WebGL, and GLSL effects.
- Native wgpu render path and SparkleFlinger frame compositor.
- Web UI, terminal UI, CLI, tray applet, and Tauri desktop shell.
- TypeScript effect SDK with built-in HTML effect packs.
- Hardware support for 175 devices across 11 driver families.
- Network drivers for Hue, Nanoleaf, and WLED.
- Release tarballs with shell completions, systemd/launchd assets, udev rules,
  bundled UI assets, bundled effects, and checksum verification.

### Security

- Fail-closed daemon startup for unauthenticated non-loopback control binds.
- Credential store seed and encrypted payload permission hardening.
- Documented unsafe-code boundary for audited platform interop crates.

### Notes

- Linux is the supported launch runtime and install path.
- macOS and Windows artifacts are experimental until their installer and runtime
  gates match Linux.
- SDK packages and the Python client are source-only until their package
  registries are published.
