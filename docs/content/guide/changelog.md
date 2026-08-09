+++
title = "Changelog & versions"
description = "How Hypercolor versioning works, where release notes live, and the channels every release ships through."
weight = 170
+++

Hypercolor is pre-1.0 software under active development. This page explains the
version numbering rules and points you at the canonical release notes. It does
not mirror per-release notes; those live with the releases themselves.

## How to check your installed version

```bash
hypercolor --version
```

The daemon prints its version on startup and in the status response:

```bash
hypercolor status
```

The REST API surfaces it in every response envelope under `meta.api_version`.

## Version numbering

Hypercolor uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html): `MAJOR.MINOR.PATCH`.

Because the project is pre-1.0, the minor version carries breaking-change
weight. A bump from `0.2` to `0.3` may change the REST API envelope, the config
file schema, or the effect SDK surface without a deprecation window; the
release notes call out every breaking change explicitly. Patch releases
(`0.3.x`) are safe to apply: they contain bug fixes and security hardening
only.

## Where release notes live

Release notes have two canonical homes:

- [GitHub Releases](https://github.com/hyperb1iss/hypercolor/releases) carries
  the user-facing notes for every tagged version, alongside the downloadable
  artifacts.
- `CHANGELOG.md` at the repository root follows the
  [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) format and is the
  machine-readable authority.

## Release channels

Every channel below is updated automatically when a release is tagged. There is
no manual publishing lag between them.

| Channel | What ships there |
|---|---|
| [GitHub Releases](https://github.com/hyperb1iss/hypercolor/releases) | Linux tarballs and `.deb` packages, macOS DMGs, Windows NSIS installer, checksums |
| Homebrew | `hypercolor` formula (CLI and daemon, with `brew services`) and `hypercolor-app` cask (desktop app) |
| AUR | `hypercolor-bin` prebuilt package |
| npm | [`hypercolor`](https://www.npmjs.com/package/hypercolor) effect SDK and [`create-hypercolor`](https://www.npmjs.com/package/create-hypercolor) scaffolder |
| PyPI | [`hypercolor`](https://pypi.org/project/hypercolor/) Python client |

The `main` branch is the working development branch; build it from source
(`just build`) if you want unreleased work. Tagged releases are the stable
surface. There is no separate nightly or beta channel.
