# Lighttoys Live Control Activation

Decoded from Windows USBPcap capture
`captures/2026-05-23-162521-user-driven-livecontrol.pcapng`.

Capture receipt:

| Field | Value |
| --- | --- |
| Tool | `tshark -i \\.\USBPcap1 -a duration:75` |
| File size | 308,941,056 bytes |
| Packets | 770,127 |
| Duration | 75.233655 seconds |
| SHA256 | `b6cfde3466815cb7ec983060afe63bd2aad1c472db009b3507f8d0310079d833` |
| Lighttoys USB address | `39` |
| Host OUT endpoint | `0x02` |
| Device IN endpoint | `0x82` |

## Result

LtComposer Live Control does not use `leach` for normal UI changes. It sends
`lprog 0,<cfgstr>` using a compact ASCII config string. The remote echoes
`lprog 0` and replies `CMD>OK`.

Direct replay with LtComposer closed also works from our own serial session
when DTR/RTS are left false:

```text
lprog 0,mM*B!0*A!0c0xff0000eCOb5  -> CMD>OK  # wand turned red
lprog 0,mM*B!0*A!0c0x00ff00eCOb5  -> CMD>OK  # wand turned green
lprog 0,mM*B!0*A!0c0x0000ffeCOb5  -> CMD>OK  # wand turned blue
```

The first observed Live Control action sent:

```text
gmute 1
```

That appears to be the blackout/mute path from the Live Control UI, not merely
a diagnostic LED toggle as previously guessed.

## Captured Transcript

```text
6.118430  OUT gmute 1
6.118634  IN  gmute 1
6.118748  IN  CMD>OK

8.898180  OUT lprog 0,mM*B!0*A!0c0xff0000eCOb5
8.898546  IN  lprog 0
8.898761  IN  CMD>OK

11.476110 OUT lprog 0,mM*B!0*A!0c0x00ff00eCOb5
11.476425 IN  lprog 0
11.476705 IN  CMD>OK

15.179619 OUT lprog 0,mM*B!0*A!0c0x0000ffeCOb5
15.179901 IN  lprog 0
15.180117 IN  CMD>OK

20.099447 OUT lprog 0,mM*B!0*A!0c0x0000ffeCOb5
20.099753 IN  lprog 0
20.100039 IN  CMD>OK

22.395791 OUT lprog 0,mA*A!0c0x0000ffeCO*B!0c0xff00ffeCOb5
22.396153 IN  lprog 0
22.396442 IN  CMD>OK

24.258236 OUT lprog 0,mA*A!0c0x0000ffeCO*B!0c0xff00ffeCOb5
24.258581 IN  lprog 0
24.258805 IN  CMD>OK

26.666418 OUT lprog 0,b0
26.666639 IN  lprog 0
26.666915 IN  CMD>OK

28.357377 OUT minfo
28.358811 IN  INF>HW:LTC-2.0.0;DT:20260323220644;FW:v2.0;M:B;NS:2;SB:0x0;ST:prog;VC:4.230;CV:4330;RF:2405
28.358811 IN  CMD>OK

28.388057 OUT glist
28.389254 IN  GRP>R:m;DA:0x30AE61D0;NSLV:2;SMSK:0x0;M:B
28.389254 IN  GRP>R:s;DA:0x3B787824
28.389254 IN  GRP>R:s;DA:0xC38DE864
28.389254 IN  CMD>OK

28.418508 OUT glist 0x3b787824
28.423088 IN  GRP>R:s;DA:0x3B787824;RSSm2s:30;RSSs2m:30;VCC:244185;BAT:72;Temp:166
28.423088 IN  CMD>OK

28.449170 OUT glist 0xc38de864
28.462725 IN  CMD>Warn

28.651339 OUT lprog 0,b3
28.651691 IN  lprog 0
28.651801 IN  CMD>OK

30.078617 OUT lprog 0,b5
30.078863 IN  lprog 0
30.079082 IN  CMD>OK

31.085249 OUT lprog 0,mA*A!0c0x0000ffeCO*B!0c0xff00ffeCOb5
31.085593 IN  lprog 0
31.085834 IN  CMD>OK

32.323727 OUT lprog 0,mA*A!0c0x0000ffeCO*B!0c0xff00ffeCOb5
32.324080 IN  lprog 0
32.324298 IN  CMD>OK

33.430397 OUT lprog 0,mA*A!0c0x0000ffeCO*B!0c0xff00ffeCOb5
33.430699 IN  lprog 0
33.430976 IN  CMD>OK
```

## `lprog` Config Grammar Observed

These strings are not base64. They are compact ASCII tokens.

| Token | Meaning |
| --- | --- |
| `b<N>` | Brightness, zero-based. UI 10/60/100 percent mapped to `b0`/`b3`/`b5`. |
| `c0xRRGGBB` | Solid RGB color. |
| `*A!0` | A segment uses effect/program slot `0` (solid in this capture). |
| `*B!0` | B segment uses effect/program slot `0`. |
| `mM...` | Mirrored or combined A+B mode. Captured for single color applied to both segments. |
| `mA...*B...` | Split segment mode. Captured as A blue and B magenta. |
| `eCO` | Terminator or effect/config option. Present after color values. |

Confirmed examples:

```text
lprog 0,mM*B!0*A!0c0xff0000eCOb5       # A+B red, brightness 5
lprog 0,mM*B!0*A!0c0x00ff00eCOb5       # A+B green, brightness 5
lprog 0,mM*B!0*A!0c0x0000ffeCOb5       # A+B blue, brightness 5
lprog 0,mA*A!0c0x0000ffeCO*B!0c0xff00ffeCOb5
lprog 0,b0                             # 10 percent brightness
lprog 0,b3                             # 60 percent brightness
lprog 0,b5                             # 100 percent brightness
```

`lprog` uses slave bitmap `0` in LtComposer Live Control, even though earlier
manual probes used `0xFF`. On this remote, `0` means the current Live Control
selection/group, not "no slaves".

## Remaining Unknowns

1. Whether `gmute 1` is required before `lprog`, or only happened because the
   first captured click was BLACKOUT.
2. Dynamic mode token grammar beyond solid color slot `!0`.
3. Whether `lprog 0,<cfgstr>` recovers from the `lstop 0xFF` non-receptive
   state without a USB replug.
4. Whether any Live Control action unlocks the hidden `lds*` streaming family.

## Extraction Command

```powershell
& 'C:\Program Files\Wireshark\tshark.exe' `
  -r docs\research\lighttoys-protocol\captures\2026-05-23-162521-user-driven-livecontrol.pcapng `
  -Y 'usb.device_address == 39 && (usbcom.data.out_payload || usbcom.data.in_payload)' `
  -T fields `
  -e frame.number -e frame.time_relative -e usb.src -e usb.dst `
  -e usb.endpoint_address -e usbcom.data.out_payload -e usbcom.data.in_payload
```
