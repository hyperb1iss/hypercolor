# hypercolor-driver-adalight

*Adalight and AWA serial driver for Hypercolor: DIY LED strips behind Arduino, ESP8266, ESP32, and RP2040 boards.*

Adalight-compatible firmware streams RGB data from a host over a serial port. This
driver speaks classic Adalight (`Ada` header, used by the Adafruit and
Adalight-FastLED sketches and by WLED's serial input) and HyperSerial's AWA
framing (`Awa` header with a Fletcher-style trailer, plus optional white-channel
data for RGBW strips). Devices are configured by hand: port path, baud rate,
protocol, LED count, color order, and white handling. The driver never
enumerates ports and never claims a device by USB id, because the boards use
generic USB-serial chips that also sit in unrelated hardware.

Output follows the render loop up to what the serial line can carry. The driver
computes the line budget from the baud rate and frame size, advertises it as the
output cadence so faster frames coalesce to the newest one, and shows the
computed maximum on the device's control surface. Spec 91 has the byte-level
details.

## Position in the Workspace

- Depends on: `hypercolor-driver-api`, `hypercolor-driver-support`,
  `hypercolor-types`, `anyhow`, `async-trait`, `serde`, `serde_json`,
  `thiserror`, `tokio`, `tokio-serial`, `tracing`
- Consumed by: `hypercolor-driver-builtin` (via the `adalight` feature, part of
  the default `serial` group)

## Key Public Surface

- `AdalightDriverModule`: `DriverModule` implementation; `new()`
- `DESCRIPTOR: DriverDescriptor`: static descriptor (`id = "adalight"`, serial transport)
- `AdalightBackend`: `DeviceBackend` over `tokio-serial`
- `AdalightConfig`, `AdalightDeviceConfig`, `WhiteMode`, `ColorOrder`: config types
- `resolve_config`, `resolve_device`, `AdalightDevicePlan`: validation and the line budget
- `configured_devices`: config entries as discovery results, opening no port
- `protocol`: `FrameEncoder`, `FrameLayout`, `Framing`, `awa_trailer`, `header`, `Bandwidth`
- `adalight_driver_control_surface`, `adalight_device_control_surface`: control surfaces

## Testing

`tests/protocol_tests.rs` pins golden bytes for every header variant and the AWA
trailer. `tests/backend_tests.rs` (Unix) drives the real backend over
pseudo-terminal pairs the test creates; no test opens a real serial device.
