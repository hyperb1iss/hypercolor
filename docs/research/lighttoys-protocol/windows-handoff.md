# Windows Session Handoff: Lighttoys FT2 Protocol RE

Cold-start brief for resuming this research on a Windows machine where LtComposer
runs natively and USB capture is uncomplicated. Read this first, then dip into
the sibling docs as needed.

> Companion docs (same folder):
> - `README.md` — full dossier (project context, architecture, integration plan)
> - `vocabulary.md` — complete decoded wire protocol reference
> - `probe.py` (also at `~/lighttoys-probe.py` on vesper) — Python serial probe tool

## 🎯 30-Second Brief

We've reverse-engineered the Pyroterra Lighttoys FT Remote serial protocol from
the firmware binary and confirmed it live. **Full command vocabulary, framing,
echo control, and response shape are documented.** A Windows USBPcap session
captured LtComposer Live Control: it uses `gmute 1` for BLACKOUT and compact
`lprog 0,<cfgstr>` strings for colors/brightness. Dynamic-mode and show-upload
formats are still open.

## ✅ What's Already Locked In

Confirmed live against your actual hardware (Lumi Wand FT2 + new FT Remote
LTC-2.0.0):

- **USB VID:PID** = `0x1669:0x1026`, manufacturer "Pyroterra s.r.o."
- **Transport:** USB-CDC ACM, **115200 baud, 8N1, no flow control**
- **Line discipline:** ASCII, send with `\n`, responses come back with `\r\n`
- **CRITICAL:** Do NOT set DTR/RTS high on port open — wedges the remote
- **Echo control:** `mecho 0` disables remote-side echo
- **Full command vocabulary** — see `vocabulary.md`, every `m*` / `g*` / `l*` /
  `s*` / `h*` / `d*` / `u*` / `ota*` command is documented
- **Master states:** `idle`, `pair`, `show`, `prog`. We're in `prog` while
  USB-attached
- **Your hardware identity:**
  - Master DA: `0x30AE61D0`
  - Slave 1 DA: `0x3B787824`
  - Slave 2 DA: `0xC38DE864`

We have a **proof-of-life sequence** that drove the wand visibly (one batch
worked, then `lstop` broke it):

```
mecho 0
leach 0xFF,255,0,0,0,0,0      # wand → RED
leach 0xFF,0,0,255,0,0,0      # wand → BLUE
leach 0xFF,0,255,0,255,0,255  # wand → A=green B=magenta
lstop 0xFF                    # reset (and apparently leaves wand
                              #  unresponsive to subsequent leach)
```

## ❓ Remaining Questions to Crack on Windows

In priority order. The Live Control color path is solved; remaining captures are
about richer effects and stored pixel content.

1. **Dynamic mode token grammar.** We have solid colors and brightness:
   `lprog 0,mM*B!0*A!0c0xff0000eCOb5` and `lprog 0,b3`. Capture PULSE,
   STROBE, FADE, DOTS, FLASH, speed, and variant changes next.
2. **`hswr` file-write framing (cracks Instagram-style pre-rendered shows).**
   How does LtComposer upload a `.ltp` show into a wand bank? The help output
   gives us the command shapes but not the chunk size, base64 payload structure,
   or the bank-write completion protocol.
3. **`lds*` family unlock (show-upload state gate).** `ldson`, `ldsoff`,
   `ldsupdate`, `ldsclear`, `ldswrite`, `ldsspace` are in the firmware vocab
   but all safe probes return `ERROR 3` from `prog` state. Firmware help says
   this family uploads show-bank data, so treat it as storage until disassembly
   proves a live pixel path.
4. **`dcmd` numeric command IDs.** `dcmd <slvbmp>,<cmd>,[<cmddata>]` lets us
   send raw slave commands. What `<cmd>` values do the slaves recognize?

## 🛠️ Windows Tooling Setup

Install these on the Windows box before plugging the remote in:

1. **LtComposer 4.4** — <https://www.lighttoys.cz/support/#downloads>
2. **Wireshark** with the **USBPcap** option checked during install
   (`https://www.wireshark.org/download.html`) — USB capture on Windows is
   first-class, no Apple Silicon weirdness
3. **Python 3.x** or PowerShell/.NET serial — for post-capture interactive
   probing. The direct replay in this session used
   `System.IO.Ports.SerialPort` with DTR/RTS disabled.
4. **PuTTY** (optional) — for manual serial line poking if you want to test
   discovered commands by hand

After install, plug the FT Remote into USB and confirm:

- Device shows up in Device Manager under "Ports (COM & LPT)" with
  "Pyroterra s.r.o. Lighttoys Controller" — note the COM number
- Wireshark, when you click Capture, shows `USBPcap1` / `USBPcap2` etc as
  available interfaces

## 📸 Capture Plan: Actions To Record

Do each action **deliberately, with 2-3s pause between clicks** so the pcap is
easy to label by timestamp. Keep a `timeline.txt` open in a text editor and jot
down `MM:SS - action` as you go.

### Phase 1: cold-start handshake (optional)

- Start Wireshark capture on USBPcap before plugging the remote in
- Plug in the remote with LtComposer NOT running
- Wait 5 seconds, capture the OS-side enumeration + any device-side banner
- Then start LtComposer
- Wait until the status bar shows the device as connected

### Phase 2: Live Control dynamics

- Open Tools → Live Control (or press `j`)
- Do each of these one at a time, 2-3s apart:
  - Click `BLACKOUT`
  - Click `STANDBY`
  - Click color RED in favorite colors
  - Click color GREEN
  - Click color BLUE
  - Click color PURPLE
  - Toggle active segment: `A+B` → `A` → `B`
  - Brightness clicks: 10 → 20 → 60 → 100 → back to 60
  - Dynamic mode click: PULSE 1
  - Dynamic mode click: PULSE 5
  - Dynamic mode click: STROBE 3
  - Dynamic mode click: FADE 7
  - Dynamic mode click: DOTS 4
  - Dynamic mode click: FLASH 8
  - Speed slider: 1 → 4 → 8 → back to 4
  - Click `BLACKOUT` again to clean up

### Phase 3: Run Show (if you have any shows uploaded)

- Close Live Control
- Use the toolbar Run Show button with bank 1, 2, 3, 4 in sequence
- Click Stop

### Phase 4: Show upload (this cracks the `hswr` Instagram path)

- Build a tiny test project (one color element on a single Wand track)
- Click Upload
- Upload to bank 1
- Capture the entire upload session
- Erase bank 1 (also captures the bank-erase protocol)

### Phase 5: Tear-down

- Quit LtComposer cleanly (do NOT just unplug — let LtComposer close so we
  catch any session-end bytes)
- Stop Wireshark capture
- Save as `capture-YYYY-MM-DD-actions.pcapng`

## 🔬 Decoding Workflow (after capture)

The CDC ACM bulk OUT endpoint contains the host -> remote bytes. Bulk IN is
remote -> host. The command stream is line-oriented ASCII.

Wireshark UI:
1. Open the pcap
2. Filter to your device: `usb.device_address == <N>` (find N in the URB
   Submit packets at the top of the capture)
3. Prefer `tshark`'s decoded `usbcom` fields for a clean transcript
4. The host -> remote bytes will look like `lprog 0,<cfgstr>\n` or
   `hswr ...\n`, depending on the UI action.

Or via `tshark` for scripted extraction:

```powershell
& 'C:\Program Files\Wireshark\tshark.exe' `
  -r capture.pcapng `
  -Y 'usb.device_address == <N> && (usbcom.data.out_payload || usbcom.data.in_payload)' `
  -T fields `
  -e frame.number -e frame.time_relative -e usb.src -e usb.dst `
  -e usb.endpoint_address -e usbcom.data.out_payload -e usbcom.data.in_payload
```

What to look for in each phase:

- **Phase 1 init:** any command sent before the user clicks anything
- **Phase 2 Live Control:** map each dynamic click to its emitted `lprog`
  config tokens
- **Phase 4 upload:** look for `hswr` lines, chunk size, base64 payload
  structure, completion handshake

## 📋 Files To Create After Capture

Add these to this folder (`docs/research/lighttoys-protocol/`):

- `captures/capture-YYYY-MM-DD-actions.pcapng` — raw capture (consider
  gitignoring large pcaps; commit only smaller annotated extracts)
- `captures/capture-YYYY-MM-DD-timeline.md` — markdown timeline mapping
  seconds → UI action (so future you can re-label the pcap easily)
- `activation-sequence.md` — decoded Live Control bytes with explanation
- `hswr-format.md` — decoded show-upload protocol (frame format, chunk size,
  base64 layout, ack handshake)
- Update `vocabulary.md` with any new discovered commands

## ⚠️ Don't Repeat These Mistakes

- **Never set DTR/RTS True when opening the port** — wedges the remote until
  you replug USB. The probe script handles this correctly.
- **`lstop 0xFF` may put the wand into a non-receptive state** for subsequent
  raw color commands. The capture will show whether LtComposer re-arms
  after lstop or avoids using it.
- **OSC command names are NOT the wire vocabulary.** LtComposer translates.
  Use `vocabulary.md` for wire commands, not the OSC manual.
- **Don't kill LtComposer with task manager** between captures — let it close
  cleanly, otherwise the remote may be left in an inconsistent state.

## 🔄 Recovery If The Remote Wedges

If commands stop working entirely:

1. Quit LtComposer
2. Unplug USB for 10 seconds
3. Long-press the remote's main button to power-cycle
4. Replug USB
5. Verify with a quick `python -c "import serial; s=serial.Serial('COM<n>',
   115200); s.write(b'minfo\n'); print(s.read(200))"` — should return a
   normal `INF>HW:LTC...` response

## 🎯 First Action On Windows

After tooling is installed:

```
1. Wireshark → Capture → USBPcap1 (start)
2. Plug FT Remote
3. Start LtComposer
4. Open Live Control (Tools → Live Control or 'j')
5. Click ONE dynamic mode or speed control
6. Stop Wireshark, save the pcap
```

That single click is the smallest experiment that extends the solved Live
Control path. Everything else in Phase 2 fills in more `lprog` grammar.

## 📖 Resuming An Agent Session

If you're feeding this to a fresh Claude/Codex session, the prompt template:

```
I'm continuing reverse-engineering of the Pyroterra Lighttoys FT Remote
serial protocol on Windows. Full background is in
docs/research/lighttoys-protocol/ in this repo. Read README.md,
vocabulary.md, activation-sequence.md, and streaming-investigation.md.
We have a pcap from a LtComposer dynamic-mode or show-upload session at
<path>. Decode the host-to-remote bytes, identify the command sequence,
and update vocabulary.md plus the focused research note with the findings.
```

The dossier is structured so a cold-read of those three files gives full
context. Vocabulary doc is the authoritative protocol reference and gets
updated as we learn more.

---

_Created 2026-05-23 by Nova for handoff to Windows session. Last verified
vocabulary against live hardware: same day._
