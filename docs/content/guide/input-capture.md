+++
title = "Input capture"
description = "Let effects react to your keyboard and mouse: how consent works, what Hypercolor can and cannot see on Linux and Windows, and why a Windows service sees nothing."
weight = 125
+++

Some effects react to what you are doing: a ripple that spreads from each keypress, a glow that tracks your cursor, a shockwave on every click. For any of that to work, Hypercolor has to be able to observe your keyboard and mouse, and that is exactly the kind of capability you should have to turn on deliberately.

This page covers:

- The consent switch, and what each half of it grants
- What Hypercolor can and cannot see on each platform
- Why a Windows daemon installed as a service sees nothing at all, and what to do instead

---

## Consent comes first

Input capture is off until you turn it on. Nothing observes your keyboard or mouse before that: no device is opened on Linux, and no registration is taken on Windows.

```toml
[input]
enabled = true
keyboard = true
mouse = true
```

`keyboard` and `mouse` are separate on purpose. Declining the mouse is not a filter applied after the fact: on Windows the daemon never registers for mouse input at all, so no pointer position can reach it even by accident, and on Linux pointer nodes are never opened. If you want key-reactive effects without handing over your cursor position, turn `mouse` off and everything else keeps working.

The UI has a one-click version of the same switch. When an interactive effect is active and capture is off, a banner appears above the preview offering to enable it.

**Capture is also demand-gated.** Even with consent granted, Hypercolor only listens while an effect that declares input reactivity is running. Switch to a non-interactive effect and the devices close again.

### Choose where interaction comes from

Host capture and browser preview input are separate sources. Configure their routing in Settings under **Input Access**, or set it directly:

```toml
[input]
daemon_route = "host"
preview_route = "browser"
```

Each route accepts `host`, `browser`, or `merge`. `daemon_route` controls the input used for authoritative device output. `preview_route` controls each interactive browser preview. Browser input is addressed to the preview that opened it, so two tabs or preview panels do not accidentally drive each other. `merge` combines host and addressed browser state only when that behavior is intentional.

The web UI subscribes to the keyed `interactive_preview` topic before it sends pointer or key events. Every `input_inject` message names the preview it drives, so a client owns the matching subscribe and unsubscribe lifecycle for that preview id.

---

## What gets captured

Only what an effect can actually use, and never the text you type.

Hypercolor records **which physical key** was pressed, not which character it produced. A key is identified by its position on the keyboard: the key where `A` sits on a US QWERTY layout is always `a`, whether your layout prints `A`, `Q`, or something else on it. That is what lets a WASD-driven effect work identically on a French AZERTY keyboard, and it is also why the capture is a poor keylogger: it does not know your layout, your modifiers' effect on characters, or your composed input.

Captured: key positions with press and release timing, mouse buttons, wheel travel, pointer motion, and which device each came from. Not captured: typed text, clipboard, window titles, or what application you are using.

Input never leaves your machine unless you explicitly enable a remote surface, and the WebSocket protocol applies its own privacy gate on top.

---

## Linux

Hypercolor reads input events directly from `/dev/input/event*`. That needs read access to those device nodes, which the `70-hypercolor-input.rules` udev rules grant. Release installs from 0.3.0 onward ship the input rules in the payload: the installer prompts to apply them alongside the USB rules, and the `.deb` and AUR packages install them automatically. On a source checkout:

```bash
just udev-install
```

An install made from a pre-0.3.0 prebuilt payload does not include the input rules; copy `udev/70-hypercolor-input.rules` from a repo checkout to `/etc/udev/rules.d/`, or upgrade.

Then replug the device, or log out and back in for group membership to take effect. Installing the rules heals a running daemon: it rescans and picks up newly readable devices without a restart.

If the rules are missing, the UI banner says so and shows the command. Diagnostics distinguish three states that look identical from the outside: capture is off, capture is on but every device node is unreadable, and capture is on and working.

---

## Windows

Hypercolor uses the Raw Input API, which observes keyboard and mouse activity while the daemon sits in the background with no window focus. There is no permission prompt and no setting to grant, but there is one hard constraint, and it is worth understanding because it fails silently.

### Run the daemon in your own session

**Raw Input cannot cross a session boundary.** A Windows service runs on its own window station with its own desktop, and it never receives a single keystroke or mouse movement from your desktop. This does not produce an error: every call succeeds, and input simply never arrives.

The same applies to a scheduled task configured to "run whether the user is logged on or not", which gets a non-interactive window station inside an otherwise ordinary session.

So if you want input-reactive effects on Windows, run the foreground Hypercolor daemon in your own desktop session rather than installing it as a service. The daemon detects this case explicitly and reports it: the UI banner will tell you it has no interactive desktop rather than leaving you to wonder why a ripple effect does nothing.

### Two things stay invisible, by design

- **Elevated windows.** An unelevated daemon cannot observe input destined for an elevated application. Typing into an administrator terminal produces nothing. This is Windows' user interface privilege isolation doing its job, and Hypercolor does not ask for an exemption; a lighting daemon has no business holding one.
- **Secure desktop.** UAC prompts, the lock screen, and Ctrl+Alt+Del are invisible, and the cursor position is unreadable there. Hypercolor holds its last known cursor position across those moments so effects do not lurch when you unlock.

Neither case produces a signal that input was skipped. If an effect seems to miss keystrokes only in an admin window, this is why.

### Remote Desktop works

An RDP session is a legitimate interactive session with its own desktop, and Hypercolor treats it as one. Virtual RDP keyboards and mice are captured normally; diagnostics label them so they do not look like phantom hardware.

---

## macOS

macOS uses native Core Graphics session event taps. Keyboard and pointer
capture are independent, event-driven sources. The keyboard source reports
physical key locations, modifiers, media keys, repeats, and releases. The
pointer source reports global position, motion, buttons, exact wheel units,
trackpad phases, and momentum.

Keyboard listening requires **Input Monitoring** permission. Hypercolor first
checks the current grant without prompting. Only an explicit authorization
action may open the system prompt. Pointer-only effects do not request Input
Monitoring, and Hypercolor does not request Accessibility or Apple Events
access for host input.

A permission loss, secure-input gap, session lock, disabled tap, or source
restart releases every held key and button before capture resumes. This keeps
interactive effects from retaining phantom input across a protected desktop
transition.

---

## Checking whether it is working

The daemon's status reports how many input devices are open and streaming, and, when input is not flowing, a reason:

```bash
hypercolor status
```

`devices_opened` counts devices that are genuinely streaming, not devices that merely exist. If it is zero while capture is on, the reason field says whether that is missing permissions, no interactive desktop, or a backend that could not start.

---

## Turning it off

```toml
[input]
enabled = false
```

Capture stops immediately, every device closes, and all held state clears, so no key or button can be left stuck on in an effect's view of the world.
