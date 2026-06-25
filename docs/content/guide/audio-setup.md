+++
title = "Audio setup"
description = "Make your lights react to music: configure a PipeWire/PulseAudio monitor source, set the device field, and verify with the TUI spectrum strip."
weight = 120
+++

# Audio setup ⚡

Hypercolor can make every LED in your rig react to music in real time — spectrum bars, beat flashes, harmonic color shifts. Getting there takes one configuration decision that trips almost everyone: you need a **monitor source**, not a microphone.

This page covers:

- Why monitor vs. capture matters and how to find the right source name
- The `[audio]` config keys that control it
- How to verify the pipeline is alive using `hypercolor audio devices` and the TUI

---

## Monitor vs. capture: the critical distinction

On PipeWire and PulseAudio (the two audio systems you are almost certainly running on Linux), every output sink automatically exposes a corresponding **monitor** source. A monitor source is a loopback tap on what the system is *playing*, not what a microphone is *hearing*. That is the source Hypercolor wants.

If you point Hypercolor at your microphone, you get room noise and your own voice. Audio-reactive effects will fire on ambient sound and stay dark whenever music is playing on headphones. Using the monitor source captures whatever is routed to your speakers or headphones, regardless of the application producing it.

By default Hypercolor auto-detects the monitor source (see below). If auto-detection works, you may not need to touch the config at all.

---

## Auto-detection

When `[audio].device` is set to `"default"` (the out-of-the-box value), the daemon queries the PulseAudio compatibility layer (which PipeWire exposes) for the default sink name, then constructs the monitor source name as `{sink_name}.monitor` and verifies it exists before opening the stream.

On most desktop installs this just works. You can confirm it by applying any audio-reactive effect — the built-in `audio-pulse` effect is designed specifically for this — and watching the TUI spectrum strip (described at the end of this page).

---

## Finding the right source name

If auto-detection picks the wrong device (for example, you have multiple sound cards and the default sink is not the one your music plays through), you need to supply an explicit source name.

List all sources your system exposes:

```bash
pactl list short sources
```

The output looks roughly like this:

```
0  alsa_output.pci-0000_00_1f.3.analog-stereo.monitor  PipeWire  s16le 2ch 48000Hz  IDLE
1  alsa_input.pci-0000_00_1f.3.analog-stereo           PipeWire  s16le 2ch 48000Hz  SUSPENDED
2  alsa_output.usb-Focusrite_Scarlett_2i2.monitor       PipeWire  s16le 2ch 48000Hz  IDLE
```

Sources whose names end in `.monitor` are the loopback taps you want. Sources without `.monitor` are physical inputs (microphones, line-in). Use the full name of the monitor source that corresponds to your active output.

You can also ask the daemon what it already knows about:

```bash
hypercolor audio devices
```

This returns the sources the daemon has enumerated along with which one is currently active (marked with a star).

---

## Configuration

Audio settings live in the `[audio]` section of `~/.config/hypercolor/hypercolor.toml`. A minimal explicit configuration looks like this:

```toml
[audio]
enabled = true
device  = "alsa_output.pci-0000_00_1f.3.analog-stereo.monitor"
```

Replace the device value with the exact source name you found above. If you want to restore auto-detection, set it back to `"default"`.

### All audio config keys

| Key | Default | What it does |
|---|---|---|
| `enabled` | `true` | Enable or disable the audio capture pipeline entirely. |
| `device` | `"default"` | Source name, or `"default"` for auto-detected monitor. |
| `fft_size` | `1024` | Primary FFT window size. Larger = more bass resolution, more CPU. Valid: 256, 512, 1024, 2048, 4096. |
| `smoothing` | `0.8` | Temporal smoothing on the falling edge (0.0 = instant, 1.0 = frozen). |
| `noise_gate` | `0.02` | RMS level below which the pipeline emits silence. Raises to avoid flicker in quiet rooms. |
| `beat_sensitivity` | `0.6` | Onset threshold multiplier. Lower = more sensitive to subtle transients. |

{% callout(type="info") %}
The `device` key maps to `AudioSourceType::Named(...)` internally when set to anything other than `"default"`. The daemon resolves the name against the PulseAudio source list at startup and on every live reconfiguration.
{% end %}

---

## Applying an audio-reactive effect

Once the config is saved, restart the daemon (or let it hot-reload the config — `watch_config = true` by default) and apply the built-in audio-pulse effect:

```bash
hypercolor effects activate audio-pulse
```

Music playing anywhere on your system should now drive your lights. To browse more audio-reactive effects, open the effects browser in the UI or TUI. If you want to author your own, the [@/effects/audio.md](@/effects/audio.md) reference documents the full `AudioData` surface effects receive each frame.

---

## Verifying with the TUI spectrum strip

The quickest way to confirm the audio pipeline is healthy is to open the TUI:

```bash
hypercolor tui
```

The bottom chrome of the TUI dashboard shows a real-time audio strip: a mini spectrum bar chart on the top row and a stats line below it with a level percentage, beat-confidence dots, and estimated BPM. If the bars are moving while music plays, the pipeline is alive.

![TUI dashboard showing the spectrum strip at the bottom](/img/tui/tui-dashboard.png)

If the strip shows "No audio", the daemon is not receiving samples. Work through the checklist below.

---

## Troubleshooting

**Spectrum strip shows "No audio" / lights do not react**

1. Run `hypercolor audio devices` and confirm the active device is a `.monitor` source.
2. Run `pactl list short sources` and check that the monitor source you configured actually exists. If you recently changed your audio hardware or switched PipeWire profiles, the source name may have changed.
3. Check that `enabled = true` in `[audio]`. After editing the file, restart the daemon (or rely on `watch_config = true`, the default, to hot-reload) so the change takes effect.
4. Make sure nothing is preventing the daemon from connecting to the PulseAudio compatibility socket. On some minimal installs, `pipewire-pulse` is not running — start it with `systemctl --user start pipewire-pulse`.

**Lights react to room noise or voice instead of music**

Your `device` is pointing at a microphone input rather than a monitor source. Set it explicitly to the `.monitor` source name from `pactl list short sources`.

**Auto-detection picks the wrong card**

Explicitly set `device` to the monitor source for your preferred output. Run `pactl list short sources` to confirm the name.

**Lights react but feel sluggish or over-smoothed**

Lower `smoothing` (try `0.5`) or increase `beat_sensitivity` (try `0.8`) in `[audio]`.

**Lights are flickering on silence / in a quiet room**

Raise `noise_gate` (try `0.05` or `0.08`). This tells the pipeline to treat very low RMS levels as silence rather than feeding noisy near-zero data to effects.

---

For deeper troubleshooting (daemon logs, audio pipeline diagnostics, PipeWire routing), see [@/troubleshooting/audio-not-reacting.md](@/troubleshooting/audio-not-reacting.md).
