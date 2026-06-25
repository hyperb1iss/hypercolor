+++
title = "Audio not reacting"
description = "Fix audio-reactive effects that don't respond: monitor vs. capture source, wrong device name in config, or audio disabled."
weight = 20
+++

Audio-reactive effects go silent for three reasons: the wrong audio source is configured, the `device` name in config doesn't match what PipeWire or PulseAudio exposes, or audio capture is disabled entirely. Work through the checklist below in order.

<!-- TODO screenshot: TUI audio panel showing spectrum bars active -->

## Quick checklist

1. Confirm `audio.enabled = true` in `~/.config/hypercolor/hypercolor.toml`.
2. Run `pactl list sources short` and find a source whose name ends in `.monitor`.
3. Set `audio.device` to that exact source name, or use `"default"` to let Hypercolor pick automatically.
4. Restart the daemon and watch the spectrum panel in the TUI or web UI.

---

## Step 1: Verify audio is enabled in config ⚡

Open `~/.config/hypercolor/hypercolor.toml`. The `[audio]` section defaults to enabled, but if it was ever explicitly disabled you will see:

```toml
[audio]
enabled = false
```

Change it to:

```toml
[audio]
enabled = true
```

If the `[audio]` section is absent entirely, audio is on by default. Move on to step 2.

---

## Step 2: Understand monitor sources vs. microphone sources

This is the most common cause. Audio-reactive effects need to hear what your speakers are playing, not your microphone.

On Linux, PipeWire and PulseAudio expose every output sink as a *monitor source*, a loopback that captures the audio being sent to that sink. Monitor source names end in `.monitor`, like:

```
alsa_output.pci-0000_00_1f.3.analog-stereo.monitor
```

If `audio.device` points at your microphone or a regular input source, effects will only react to sounds the mic picks up, or nothing at all when the room is quiet.

To list every audio source your system exposes:

```bash
pactl list sources short
```

Output looks like:

```
0  alsa_output.pci-0000_00_1f.3.analog-stereo.monitor  PipeWire  s32le 2ch 48000Hz  IDLE
1  alsa_input.pci-0000_00_1f.3.analog-stereo           PipeWire  s32le 2ch 48000Hz  SUSPENDED
```

The source on line 0, ending in `.monitor`, is the one you want for music-reactive lighting.

{% callout(type="tip") %}
To react to music through your speakers, pick the `.monitor` source for your active output sink. To react to sounds in the room via a microphone, pick a regular input source. The two behave very differently, and most users want the monitor.
{% end %}

---

## Step 3: Set the correct device in config

Hypercolor's `audio.device` field accepts either `"default"` or an exact source name.

**Using `"default"` (recommended):**

```toml
[audio]
enabled = true
device = "default"
```

With `device = "default"`, Hypercolor automatically discovers the monitor source for your default output sink. This works on most PipeWire and PulseAudio setups. If your default output changes, say switching from speakers to headphones, Hypercolor follows it.

**Pinning a specific source:**

If auto-discovery isn't finding the right source, or you have multiple outputs and want a specific one:

```toml
[audio]
enabled = true
device = "alsa_output.pci-0000_00_1f.3.analog-stereo.monitor"
```

Use the exact name from `pactl list sources short`. The match is case-insensitive and Hypercolor also tries partial matches, but an exact name is the most reliable.

{% callout(type="warning") %}
The `device` field must match a PulseAudio/PipeWire source name, not an ALSA device string like `hw:0,0`. Bare ALSA device strings are not supported for system loopback capture.
{% end %}

---

## Step 4: Confirm capture is active

After restarting the daemon, open the TUI:

```bash
hypercolor tui
```

Navigate to the audio panel. When an audio-reactive effect is active and audio is flowing, you should see spectrum bars moving in real time. If the bars are flat with audio playing, either the source is wrong or capture failed to start. Check the daemon log.

To follow daemon logs:

```bash
hypercolor service logs --follow
```

A successful stream start logs the message `Audio capture stream configured` with structured fields for the source type, resolved device name, sample rate, and channel count. The exact rendered format depends on your log subscriber, but it looks roughly like:

```
Audio capture stream configured source=SystemMonitor device=alsa_output.pci-0000_00_1f.3.analog-stereo.monitor sample_rate_hz=48000 channels=2
```

If you instead see:

```
Audio capture unavailable; LightScript audio input will fall back to silence
```

The source name is wrong or PipeWire/PulseAudio refused the connection. Double-check the exact name from `pactl list sources short`.

---

## Step 5: Apply an audio-reactive effect to test

Audio capture only starts when an audio-reactive effect is actually running. The daemon gates live capture on whether the active effect uses audio. If no audio-reactive effect is loaded, the capture stream is not opened.

Apply one to test:

```bash
hypercolor effects activate audio_pulse
```

With the effect running, play audio on your system. The LEDs should pulse with the beat. If they're still static, check the diagnostic sections below.

---

## Diagnostic: noise gate too high

If effects react only to very loud audio but not at normal listening levels, the noise gate may be set too aggressively. The default threshold is `0.02`:

```toml
[audio]
noise_gate = 0.02
```

This is a linear amplitude threshold in the range 0.0–1.0. Lower it to make the pipeline more sensitive, or raise it to suppress background noise:

```toml
[audio]
noise_gate = 0.01   # More sensitive, picks up quieter audio
```

---

## Diagnostic: effects react but weakly

If effects react but with low amplitude, verify your system output volume is not near zero. The analysis pipeline reads real sample amplitude, so very quiet output produces very quiet analysis. Also check beat sensitivity:

```toml
[audio]
beat_sensitivity = 0.6   # Default; lower = more beats detected, higher = only strong hits
```

---

## Bare ALSA systems (no PipeWire or PulseAudio)

If your system uses only bare ALSA with no PulseAudio compatibility layer, system loopback capture is not supported directly. ALSA's `snd-aloop` module can create a software loopback device, but Hypercolor does not configure it automatically.

The recommended path is to install PipeWire, which provides a PulseAudio compatibility layer via `pipewire-pulse`. PipeWire exposes monitor sources that Hypercolor discovers automatically, and it is the standard audio subsystem on most modern Linux distributions.

{% callout(type="info") %}
PipeWire speaks the PulseAudio protocol via `pipewire-pulse`. If `pactl list sources short` works on your system, Hypercolor's audio capture works too, with no extra configuration needed.
{% end %}

---

## Summary of config keys

All keys live under `[audio]` in `~/.config/hypercolor/hypercolor.toml`:

| Key | Default | Purpose |
|---|---|---|
| `enabled` | `true` | Master switch. `false` disables all audio capture. |
| `device` | `"default"` | Source name from `pactl list sources short`, or `"default"` for auto-detect. |
| `fft_size` | `1024` | FFT window size. Larger improves bass resolution at the cost of latency. |
| `smoothing` | `0.8` | Spectrum smoothing factor (0.0 = raw, 1.0 = frozen). |
| `noise_gate` | `0.02` | Linear amplitude threshold below which audio is treated as silence. |
| `beat_sensitivity` | `0.6` | Onset detection multiplier. Lower = more triggers, higher = only strong hits. |

---

For the full audio pipeline architecture and the JavaScript API available to effect authors, see the [audio effects reference](@/effects/audio.md). For a first-time PipeWire setup walkthrough, see [audio setup](@/guide/audio-setup.md).
