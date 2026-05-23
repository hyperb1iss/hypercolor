# Lighttoys Pixel Streaming Investigation

Status: no evidence yet for host-driven per-pixel live streaming through the
FT Remote. Evidence currently points to two separate control paths:

1. Live Control: `lprog 0,<cfgstr>` for A/B segment colors, brightness, and
   dynamic modes.
2. Show upload/playback: `lds*` / `hswr` style banked storage commands for
   pre-rendered image shows.

## What We Confirmed Live

With LtComposer closed, direct replay from Hypercolor's Windows box works:

```text
mecho 0                                      -> CMD>OK
lprog 0,mM*B!0*A!0c0xff0000eCOb5             -> CMD>OK  # red
lprog 0,mM*B!0*A!0c0x00ff00eCOb5             -> CMD>OK  # green
lprog 0,mM*B!0*A!0c0x0000ffeCOb5             -> CMD>OK  # blue
lprog 0,mA*A!0c0x0000ffeCO*B!0c0xff00ffeCOb5 -> CMD>OK
lprog 0,b0                                   -> CMD>OK
lprog 0,b5                                   -> CMD>OK
```

This means Hypercolor can drive the Lumi Wand FT2 today as a low-rate A/B
zoned lighting device through the FT Remote.

## LDS Findings

The `ctrl2` firmware help strings define LDS as show-upload storage, not live
streaming:

```text
ldsupdate <slvbmp>,<bank> update 'bank' record in show list
ldswrite  <slvbmp>,<bank>,<cmdofs>,<base64data> upload data in B64,max.18cmd(216B/288ch)
ldsclear  <slvbmp>,<bank> clear 'bank'
ldsspace  <slvbmp>,<bank> return free space in the 'bank' (size in commands)
ldsoff    <slvbmp> exit from mode for uploading shows
ldson     <slvbmp> enter to mode for uploading shows
```

Safe live probes all returned `ERROR 3`, including valid-looking bitmap/bank
forms:

```text
ldson 1          -> ERROR 3
ldsspace 1,0     -> ERROR 3
ldsspace 1,1     -> ERROR 3
ldson 2          -> ERROR 3
ldsspace 2,0     -> ERROR 3
ldson 0xFF       -> ERROR 3
ldsspace 0xFF,1  -> ERROR 3
```

`ERROR 3` here means either state-gated or not valid for the paired device in
the current `prog` state. We deliberately did not run valid `ldsclear`,
`ldswrite`, or `ldsupdate` forms because those can mutate show banks.

## Firmware Evidence

The FT2 slave firmware has substantial image/show playback code:

```text
IMG:DERR> Unsupported type of image
IMG:DERR> RGB pixel cannot be corrected.
SHOW:DDEV> >>> show start <<<
SHOW:DDEV> ------ FT2SHOW UID=x%08lX
SHOW:DERR> iVPOI: image space outside? @ %d
SHOW:DERR> iVPOI: set_space error %d
```

That supports the product behavior: pixel-level content exists as uploaded
shows/images, then the wand plays them locally. It does not prove host-driven
live pixel streaming exists.

## Current Read

Most likely:

- Real-time USB/RF control is `lprog` A/B zoned control.
- Pixel-level content is pre-rendered into show banks with `ldswrite`/`hswr`,
  then played with `splay`/`sstart`.
- The hidden-looking `LDS>1..6` debug strings are the show-upload command
  response path, not a frame-streaming protocol.

Still possible:

- A factory/dev state unlocks LDS upload mode.
- A direct slave command (`dcmd`) can access an LDS status path, but initial
  safe probes with candidate command `3` were silent:

```text
dcmd 1,3   -> no serial response
dresp 1,0  -> no serial response
dcmd 2,3   -> no serial response
dresp 2,0  -> no serial response
```

## Next Experiments

1. Capture a real LtComposer show upload and decode `ldson`/`ldswrite`/`hswr`
   state transitions, chunk size, and bank selection.
2. Capture dynamic Live Control modes to finish the `lprog` token grammar.
3. Use Ghidra on `ctrl2-app-v10r136_260323.lhex` to trace why `ldson` returns
   `ERROR 3` in our current state.
4. Use Ghidra on `ft2_slave-app-v10r136_260323.lbc` to identify whether any
   RF command path accepts raw pixel frames, or only stored show columns.
