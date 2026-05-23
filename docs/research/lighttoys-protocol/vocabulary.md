# Lighttoys FT Remote Wire Protocol

Live captured against a real FT Remote (USB VID `0x1669`, PID `0x1026`,
manufacturer "Pyroterra s.r.o.", product "Lighttoys Controller", master DA
`0x30AE61D0`) running firmware `LTC-2.0.0` / `FW v2.0` (build `20260323220644`).

> Status: protocol shape, framing, command vocabulary, and LtComposer Live
> Control command path decoded. Dynamic-mode and show-upload details remain open.

## Transport

- **USB-CDC ACM** on `/dev/ttyACM*` (Linux) / `/dev/cu.usbmodem*` (Mac).
- **Baud**: `115200`, 8N1.
- **Flow control**: none. **DTR/RTS must be left at False on connect** —
  setting them True on open will wedge the remote until USB replug.
- **Echo**: enabled by default; disable with `mecho 0`.

## Framing

Line-oriented ASCII. Commands are sent terminated with `\n` (LF). Responses
are terminated with `\r\n` (CRLF). With echo on, the device echoes the input
line back before the response.

Response classes (only when content is sent — bare `\n` and `\r\n` produce no
output):

| Response | Meaning |
| --- | --- |
| `CMD>OK\r\n` | Success |
| `CMD>Warn\r\n` | Command issued but slave did not ACK (broadcast OK, response missing) |
| `CMD>Wait\r\n` | Command in progress (async) |
| `CMD>Done\r\n` | Async command completed |
| `CMD>Error\r\nERROR <code>\r\n` | Error with code |
| `ERROR <code>\r\n` | Bare error (older / no `CMD>Error` line) |

Known error codes:

| Code | Meaning |
| --- | --- |
| `ERROR 3` | Unknown command, OR command not valid in current state |
| `ERROR 5` | Missing or invalid arguments |

Other firmware-side output (data/info responses):

| Prefix | Meaning |
| --- | --- |
| `INF>` | Information response (e.g. `minfo`) |
| `GRP>` | Group / paired-device entries |
| `UDN>` | User Device Name |
| `DCR>` | Direct command response |
| `SCP>` | Show check / playback response |
| `UFW>` | Update firmware response |
| `DBG>` | Debug output |
| `ERR>` | Error log |
| `MSG:` | Debug messages (DINF / DERR / DWAR) |

## Master States

The master (FT Remote) has 4 internal states stored as a contiguous lookup
table in firmware (confirmed by static analysis):

| ST | Meaning |
| --- | --- |
| `idle` | At rest |
| `pair` | Pairing mode (entered during `gadd`) |
| `show` | Show playback mode |
| `prog` | Programming mode — host-attached, USB-driven |

When the remote is connected via USB and you've talked to it, `ST` reports
`prog`. **The physical remote's "live" / "show" UI modes do NOT change the
`ST` field** — those are an internal UI thread state that drives RF
independently of the USB-side state.

## Command Vocabulary

### Master (`m*`) — controls the remote itself

| Command | Args | Notes |
| --- | --- | --- |
| `help` | — | Returns full help text. Always works. |
| `minfo` | — | Identification: `INF>HW:LTC-x.y.z;DT:<build>;FW:vX.Y;M:<mode>;NS:<nslaves>;SB:0x<bmp>;ST:<state>;VC:<voltage>;CV:<chgV>;RF:<MHz>` |
| `mping` | `<ptime>` | Set period [ms] of automatic radio ping to keep slaves alive. |
| `moff` | — | Switch off master. |
| `mtxp` | `<txpwr>` | Set master radio output power. |
| `mecho` | `<enb>` | `1` = enable serial echo, `0` = disable. |
| `mset` | `<param>` (GET) or `<param>,<value>` (SET) | Master parameter set/get. Confirmed param: `rfreq` (returns `rfreq=2405`). Other params return `CMD>Error\r\nERROR 3`. |
| `rst2boot` | `<passw>` | Reset to bootloader (password-protected). |

### Group / paired devices (`g*`) — operations on the slave group

`<slvbmp>` is a hex bitmap selecting which slaves to address. `0xFF` is
broadcast. `<slvla>` is the local address (numeric, 0..N-1).

| Command | Args | Notes |
| --- | --- | --- |
| `gadd` | `<clr>` | Pair more devices. `clr=0` add, `clr=1` start new group. |
| `glist` | — | List paired devices. Returns `GRP>R:m;DA:0xXXXX;NSLV:N;SMSK:0xXX;M:X` for master plus `GRP>R:s;DA:0xXXXX` per slave. |
| `gping` | `<slvbmp>` | Send PING. Returns `CMD>Warn` if no slave ACKs. |
| `ginfo` | `<slvbmp>` | Get slave info. |
| `gname` | `<slvbmp>` | Get slave name (responds via `UDN>` lines, possibly async). |
| `gtime` | `<slvbmp>` | Synchronize time to slaves. |
| `grem` | `<slvbmp>` | Ungroup (remove from paired set). |
| `goff` | `<slvbmp>[,<args>]` | Switch off slaves. |
| `gtxp` | `<slvbmp>,<txpwr>` | Set slave radio TX power. |
| `gstat` | `<slvbmp>` | Get slave state. |
| `gid` | `<slvla>` | Get device ID by local address. |
| `gfullid` | `<slvla>` | Get device full ID by local address. |
| `gmute` | `<1/0>` | Group mute / blackout path. LtComposer Live Control sends `gmute 1` when BLACKOUT is clicked. Earlier "slave indicator LED" meaning is now doubtful. |

### LED / live control (`l*`) — direct color/effect on slaves

| Command | Args | Notes |
| --- | --- | --- |
| `bright` | `<level>` | Set brightness (1-6). Returns `CMD>Error\r\nERROR 3` in `prog` state. LtComposer Live Control uses `lprog 0,b<N>` instead. |
| `leach` | `<slvbmp>,<pwm0>,<pwm1>,<pwm2>,<pwm3>,<pwm4>,<pwm5>` | **6-channel raw PWM**: pwm0-2 = A segment R/G/B, pwm3-5 = B segment R/G/B. Values 0-255. |
| `lprog` | `<slvbmp>,<chan>,<color>,<effect>,<blevel>,<slevel>` OR `<slvbmp>,<cfgstr>` | High-level program. LtComposer Live Control uses the `cfgstr` form with compact ASCII tokens, not base64. Confirmed examples: `lprog 0,mM*B!0*A!0c0xff0000eCOb5`, `lprog 0,b3`. |
| `lrand` | `<slvbmp>,<cfgstr>` | Run random mode with config string. |
| `lstop` | `<slvbmp>` | Reset all channels. **Caution: appears to leave the wand in a non-receptive state for subsequent `leach`/`lprog` commands until something else activates it.** |

### Show (`s*`) — pre-uploaded show playback

| Command | Args | Notes |
| --- | --- | --- |
| `sstart` | `<slvbmp>,<bank>,<dtime>,<dfrom>` | Start show in `bank` from `dfrom` with delay `dtime`. |
| `splay` | `<slvbmp>,<bank/UID>[,h<holdoff>][,o<offset>][,d<duration>][,b<blevel>]` | Play show with rich options. |
| `sstop` | `<slvbmp>` | Halt running show. |
| `sdelay` | `<delay>` | Set delay in [ms] before starting show. |
| `slist` | `<slvla>` | Read list of shows from device. Asynchronous: returns `CMD>OK` immediately, data arrives later via `SCP>` lines. |

### Host file I/O (`h*`) — used for show upload from PC

`hsrd` and `hswr` read/write files on slave flash. Used by LtComposer's show-
upload workflow. Multiple syntactic forms:

```
hsrd <sbmp>,<UID>,<sz>       # read by UID
hsrd <sbmp>,n,<sz>           # next chunk
hsrd <sbmp>,o<offs>,<sz>     # read at offset
hsrd <sbmp>,s                # start
hsrd <sbmp>,h                # halt
hswr <sbmp>,<UID>,i/e,b<bank>,<size>,<name>   # write header
hswr <sbmp>,n,<data>         # next chunk (data probably base64)
hswr <sbmp>,o<offs>,<data>   # write at offset
hswr <sbmp>,s                # start
hswr <sbmp>,h                # halt
```

Firmware contains base64 encode/decode references (`DBG>decode base64 failed`),
confirming chunk payloads are base64.

### Direct device commands (`d*`)

| Command | Args | Notes |
| --- | --- | --- |
| `dcmd` | `<slvbmp>,<cmd>,[<cmddata>]` | Send raw protocol command to slave. `<cmd>` is a numeric command ID. Probably what `/lighttoys/direct` OSC routes to. |
| `dresp` | `<slvbmp>,<cmdidx>` | Return answer to a direct command. |
| `loprc` | `<slvbmp>` | Get return codes from last operation. |

### Firmware update (`u*`, `ota*`)

| Command | Args | Notes |
| --- | --- | --- |
| `ufwon` | `<passw>` | Switch master to UFW mode (password gated). |
| `otarun` | `<da> <fileuid>` | Switch master to OTA mode for slave by device-address. |
| `otaget` | `<da>[,<cmd>]` | Send OTA data to slave. |

### Hidden / firmware-only

These tokens appear in the firmware binary's string table but are NOT
registered as commands in the current state. They return `ERROR 3` if called:

| Token | Suspected purpose |
| --- | --- |
| `ldson`, `ldsoff`, `ldsupdate`, `ldsclear`, `ldswrite`, `ldsspace` | Show-upload bank commands, not currently live pixel streaming. Firmware help says `ldson` enters upload-show mode, `ldswrite` uploads base64 bank data, and `ldsspace` returns free bank space. All safe probes returned `ERROR 3` in current `prog` state. |
| `pair`, `prog`, `show`, `idle` | State name strings — used in `ST:` field, not as commands themselves. |
| `bcreset` | "Bouper reset" — recovery / bootloader reset path. |
| `batt`, `version`, `uptime`, `inf` | Looked like introspection but return `ERROR 3`. May be hidden behind a state or password. |

## `glist` Output Format

```
GRP>R:m;DA:0x30AE61D0;NSLV:2;SMSK:0x0;M:B
GRP>R:s;DA:0x3B787824
GRP>R:s;DA:0xC38DE864
CMD>OK
```

Fields:

| Field | Meaning |
| --- | --- |
| `R:m` | Role: master |
| `R:s` | Role: slave |
| `DA:0xXXXX` | Device Address (32-bit unique ID) |
| `NSLV` | Number of paired slaves |
| `SMSK` | Slave bitmap (active selection) |
| `M:B` | Mode (B = broadcast?) |

## `minfo` Output Format

```
INF>HW:LTC-2.0.0;DT:20260323220644;FW:v2.0;M:B;NS:2;SB:0x0;ST:prog;VC:4.229;CV:4330;RF:2405
```

Fields:

| Field | Meaning |
| --- | --- |
| `HW` | Hardware version (LTC = Lighttoys Controller) |
| `DT` | Build date/time (YYYYMMDDHHMMSS) |
| `FW` | Firmware version |
| `M` | Mode (`B` = broadcast) |
| `NS` | Number of slaves paired |
| `SB` | Slave bitmap (active) |
| `ST` | State (`idle` / `pair` / `show` / `prog`) |
| `VC` | Voltage (battery, in volts) |
| `CV` | Charging voltage (millivolts) |
| `RF` | Radio frequency (MHz) |

## Confirmed Working Sequence (live color, captured 2026-05-23)

The very first batch after a fresh port open DID drive the wand visibly:

```
mecho 0                             → CMD>OK (echo disabled)
gtime 0xFF                          → CMD>Warn (no slave ACK, fine)
gping 0xFF                          → CMD>Warn
gmute 0                             → CMD>OK
leach 0xFF,255,0,0,0,0,0            → CMD>OK   [wand → RED]
leach 0xFF,0,0,255,0,0,0            → CMD>OK   [wand → BLUE]
leach 0xFF,0,255,0,255,0,255        → CMD>OK   [wand → A=green, B=magenta]
lstop 0xFF                          → CMD>OK   [wand reset, became unresponsive to subsequent serial leach/lprog]
```

### LtComposer Live Control sequence

Windows USBPcap capture
`captures/2026-05-23-162521-user-driven-livecontrol.pcapng` captured the real
Live Control command path. See [`activation-sequence.md`](activation-sequence.md)
for full transcript and receipt.

```text
gmute 1                                  -> CMD>OK
lprog 0,mM*B!0*A!0c0xff0000eCOb5        -> CMD>OK   [A+B red]
lprog 0,mM*B!0*A!0c0x00ff00eCOb5        -> CMD>OK   [A+B green]
lprog 0,mM*B!0*A!0c0x0000ffeCOb5        -> CMD>OK   [A+B blue]
lprog 0,mA*A!0c0x0000ffeCO*B!0c0xff00ffeCOb5
lprog 0,b0                              -> CMD>OK   [10% brightness]
lprog 0,b3                              -> CMD>OK   [60% brightness]
lprog 0,b5                              -> CMD>OK   [100% brightness]
```

Important: LtComposer uses slave bitmap `0` for the active Live Control target.
The compact `cfgstr` is plain ASCII token syntax. Confirmed tokens include
`b<N>` brightness, `c0xRRGGBB` color, `*A!0` / `*B!0` segment solid slots,
`mM` combined A+B mode, and `mA` split mode.

## Open Questions

1. Whether `gmute 1` is required before `lprog`, or only appeared because the
   first captured action was BLACKOUT.
2. Dynamic mode token grammar beyond solid `!0`.
3. Whether `lprog 0,<cfgstr>` recovers from the `lstop 0xFF` non-receptive
   state without a USB replug.
4. What state unlocks the `lds*` show-upload bank family.
5. What numeric command IDs work for `dcmd`.
6. Why `gping 0xFF` consistently returns `CMD>Warn` even though physical
   buttons clearly reach the slave (suggesting slaves listen but don't ACK).
7. The bidirectional message format for `gname` / `ginfo` data lines
   (`UDN>LA:%d;NM:%s`, `INF>DA:0x%08lX;%s`).

## Next Steps

1. Capture dynamic mode clicks to decode `!<effect>` and speed/variant tokens.
2. Capture show upload to decode `hswr` / `ldswrite` chunking and bank-write
   handshake.
3. Disassemble `ctrl2.bin` in Ghidra around the `lprog` / `leach` handlers
   to confirm bitmap `0`, cfg token grammar, and state gates.

## Probe Hygiene Lessons Learned

- **Never set DTR/RTS high on a fresh port open** — wedges the remote.
- One probe at a time during initial vocabulary discovery; clean drain
  before each send.
- The `help` command works in any state and is the safest probe.
- Empty input (bare `\n` or `\r\n`) produces no output and is safe to send.
- `lstop` may be unsafe to send if you want continued live control.
