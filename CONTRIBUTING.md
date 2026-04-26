# Contributing to Hypercolor

Thank you for your interest in Hypercolor! We welcome contributions of all kinds: new device
drivers, effects, UI improvements, documentation, bug reports, and ideas.

## Getting Started

```bash
# Clone and build
git clone https://github.com/hyperb1iss/hypercolor.git
cd hypercolor
just verify        # fmt + lint + test — run this after every change
```

**Requirements:**

- Rust 1.94+ (edition 2024)
- [just](https://github.com/casey/just) command runner
- [Bun](https://bun.sh/) for the effect SDK
- Linux recommended for full functionality (USB/HID, screen capture, audio)

## What to Work On

**Effects** are the easiest way to contribute. The SDK makes it straightforward to create something
beautiful without touching Rust. See the [effects documentation](docs/content/effects/_index.md) for
the authoring paths, setup, dev workflow, and API references.

**Device drivers** are where we need the most help. If you own RGB hardware that Hypercolor
doesn't support yet, you're in a unique position to contribute. We provide AI-assisted driver
development skills in `.agents/skills/hal-driver-development/` and
`.agents/skills/protocol-research/` to help you get started.

**Bug fixes and improvements** are always welcome. Check the issue tracker for things tagged
`good first issue`.

## Development Workflow

```bash
just verify          # Format, lint, and test everything
just check           # Quick type-check
just test            # Run tests only
just test-crate hypercolor-hal   # Test a specific crate
just daemon          # Run the daemon locally
just ui-dev          # Leptos UI dev server
just sdk-dev         # SDK dev server with HMR
```

## Code Standards

- **No `unsafe` code.** It's forbidden workspace-wide.
- **No `unwrap()`.** Use `?`, `.ok()`, or `expect("reason")`.
- **Clippy pedantic** is enforced at deny level. Run `just lint` before submitting.
- **Tests go in `tests/` directories**, not inline `#[cfg(test)]` blocks.
- **Conventional commits**: `feat(scope):`, `fix(scope):`, `refactor(scope):`, etc.
- **Emoji in docs/UI**: expressive, not excessive. Prefer 💜 🔮 ⚡ 💎 🌈 🌊 🎯.
  Avoid 🚀 ✨ 💯. One per heading max, never in body text.

## Submitting Changes

1. Fork the repository and create your branch from `main`.
2. Write your code, add tests, and make sure `just verify` passes.
3. Write a clear PR description explaining what changed and why.
4. If you're adding a new device driver, note whether you tested on real hardware.

## Driver Contributions

Writing a driver usually means:

1. Researching the device's USB/HID protocol (we have tools and guides for this)
2. Implementing the `Protocol` trait in `hypercolor-hal`
3. Adding device descriptors so Hypercolor can detect the hardware
4. Writing encoding tests to verify the wire format

If you have the hardware but aren't sure where to start, open an issue and we'll help you
figure out the protocol.

**Testing on real hardware matters.** We mark PRs with whether they've been validated on actual
devices. If you can test, please do. If you can't, say so — someone else may be able to help.

## Reporting Issues

- **Bugs:** Include your OS, Rust version, device model, and steps to reproduce.
- **Feature requests:** Describe the use case, not just the feature.
- **Security vulnerabilities:** See [`SECURITY.md`](SECURITY.md) for responsible disclosure.

## Code of Conduct

This project follows our [Code of Conduct](CODE_OF_CONDUCT.md). Be kind, be respectful, build
cool stuff together.

## License

By contributing, you agree that your contributions will be licensed under Apache-2.0, the same
license as the project.
