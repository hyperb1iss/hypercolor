# Hypercolor Driver Database

Single source of truth for every RGB device we know about and its support status.

## Structure

```
data/drivers/
  README.md              # This file
  vendors/
    razer.toml           # One file per vendor
    corsair.toml
    asus.toml
    ...
```

## Schema

Each vendor TOML file follows this structure:

```toml
[vendor]
name = "Razer"
vid = [0x1532]           # Primary USB vendor ID(s)
website = "https://razer.com"
notes = ""               # Optional vendor-level note

[[devices]]
pid = 0x026C
name = "Huntsman V2"
type = "keyboard"        # keyboard | mouse | mousepad | headset | microphone
                         # speakers | aio | fan_controller | argb_controller
                         # gpu | motherboard | ram | monitor | lcd
                         # lightbar | case | desk | strip | other
status = "supported"     # supported     : a Hypercolor driver exists today
                         # in_progress   : actively being implemented
                         # blocked       : needs firmware/hardware work
                         # planned       : will implement, protocol known
                         # researched    : protocol documented, not started
                         # known         : device exists, not yet researched
verification = "hardware"  # hardware | community | unverified (default)
                         # see Verification below
driver = "razer"         # Hypercolor driver family (if supported/in_progress)
transport = "usb_hid"    # usb_hid | usb_hid_raw | usb_control | usb_bulk
                         # usb_serial | usb_vendor | usb_midi | i2c_smbus
                         # network_http | network_udp | network_mdns
leds = 22                # LED count (optional, 0 = unknown)
notes = ""               # Anything notable (firmware requirements, quirks, etc.)
```

## Status Definitions

| Status        | Meaning                                | Action Needed                |
| ------------- | -------------------------------------- | ---------------------------- |
| `supported`   | A Hypercolor driver exists today       | Maintain, test, verify       |
| `in_progress` | Driver under active development        | Finish implementation        |
| `blocked`     | Needs work outside our control         | Track blocker, revisit later |
| `planned`     | Protocol known, implementation queued  | Build when prioritized       |
| `researched`  | Protocol documented, not yet started   | Evaluate for implementation  |
| `known`       | Device exists, needs protocol research | Research when prioritized    |

## Verification

`verification` records how a device with a driver was confirmed (Spec 89).
It is optional and defaults to `unverified`, so a row that says nothing
claims nothing.

| Value        | Meaning                                                 | Evidence                                                                 |
| ------------ | ------------------------------------------------------- | ------------------------------------------------------------------------ |
| `hardware`   | A maintainer ran it on real hardware with this driver   | Commit, spec, or PR naming the device, firmware, OS, and what was tested |
| `community`  | A user reported it working                              | A closed issue or discussion on `hyperb1iss/hypercolor`, cited in `notes` |
| `unverified` | Encoder, spec, or captures only                         | None                                                                     |

The generator rejects unknown values and rejects `hardware` or `community` on
a row without a `driver`. A row that covers several products under one PID
needs evidence for the product it is named after. A device appearing in a
protocol catalog, a capture from someone else's machine, or an issue closed
without a user's confirmation is not evidence.

To promote a row, change `verification`, cite the evidence in the PR body,
and run `just compat`. For `community`, also add "Reported working in #N"
to the row's `notes`. Spec 89 §6 has the full promotion path, including when
an experimental driver label flips to stable.

## Updating

When adding a new driver to Hypercolor, update the corresponding vendor TOML:

1. Change `status` from `planned`/`researched` to `in_progress` or `supported`
2. Set `driver` to the Hypercolor driver family name
3. Set `transport` to the transport type used
4. Add any `notes` about quirks discovered during implementation
5. Leave `verification` out (it defaults to `unverified`) unless you ran the
   device yourself or a user confirmed it
6. Run `just compat` to regenerate every downstream artifact, and commit the
   regenerated files alongside your TOML change

When researching a new vendor, create a new TOML file from the template above.

## Generated Outputs

Editing any vendor TOML makes four artifacts stale. `just compat` regenerates
all of them from `data/drivers/vendors/*.toml`:

| Artifact | What it feeds |
| --- | --- |
| `data/compat/compatibility.json` | Machine-readable matrix |
| `docs/content/hardware/compatibility.md` | The full matrix on the docs site |
| `data/compat/README-hardware.md` | Reusable hardware snippet |
| The `<!-- BEGIN COMPAT -->` block in the root `README.md` | Front-door summary table |

`just compat-check` verifies all four are current without writing anything, and
it is a CI gate: the `compat` job runs `bun scripts/gen-compat.ts --check` on
any pull request touching `data/drivers/vendors/**`, `data/compat/**`,
`docs/content/hardware/compatibility.md`, or `scripts/gen-compat.ts`. Skipping
the regeneration lands a red PR.

Never hand-edit the generated files, including the block between the compat
markers in the root `README.md`.
