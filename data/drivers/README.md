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

[[devices]]
pid = 0x026C
name = "Huntsman V2"
type = "keyboard"        # keyboard | mouse | mousepad | headset | microphone
                         # speakers | aio | fan_controller | argb_controller
                         # gpu | motherboard | ram | monitor | lcd
                         # lightbar | case | desk | strip | other
status = "supported"     # supported      — works in Hypercolor today
                         # in_progress    — actively being implemented
                         # blocked        — needs firmware/hardware work
                         # planned        — will implement, protocol known
                         # researched     — protocol documented, not started
                         # known          — device exists, not yet researched
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
| `supported`   | Works in Hypercolor today              | Maintain and test            |
| `in_progress` | Driver under active development        | Finish implementation        |
| `blocked`     | Needs work outside our control         | Track blocker, revisit later |
| `planned`     | Protocol known, implementation queued  | Build when prioritized       |
| `researched`  | Protocol documented, not yet started   | Evaluate for implementation  |
| `known`       | Device exists, needs protocol research | Research when prioritized    |

## Updating

When adding a new driver to Hypercolor, update the corresponding vendor TOML:

1. Change `status` from `planned`/`researched` to `in_progress` or `supported`
2. Set `driver` to the Hypercolor driver family name
3. Set `transport` to the transport type used
4. Add any `notes` about quirks discovered during implementation

When researching a new vendor, create a new TOML file from the template above.
