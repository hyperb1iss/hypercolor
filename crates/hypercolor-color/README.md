# hypercolor-color

*The color kernel — pixel types, conversions, blending, and device encoding.*

This crate is the single canonical home for color math across the workspace. It
owns the pixel value types, color-space conversions, hex parsing, pixel
blending, brightness scaling, and device channel encoding. It sits at the very
bottom of the crate graph and depends on no other Hypercolor crate, so every
other crate can reach the same color semantics without a cycle.

Normative conventions (Spec 76 §1.1) are enforced here rather than at each call
site: hue is degrees wrapped with `rem_euclid(360.0)` at every entry point;
saturation, value, lightness, and alpha are `0.0..=1.0`; float to `u8`
conversion is always `(x * 255.0).round().clamp(0.0, 255.0)`; and linearization
always appears in the function name, because nothing converts color space
implicitly.

## Workspace position

**Depends on:** `thiserror`; optionally `serde` and `utoipa` behind the matching
features. No Hypercolor crates.

**Depended on by:** `hypercolor-types` (which re-exports the pixel data
carriers), `hypercolor-core`, `hypercolor-hal`, `hypercolor-daemon`,
`hypercolor-cli`, `hypercolor-tui`, `hypercolor-ui`, `hypercolor-driver-hue`,
and `hypercolor-driver-nanoleaf`.

## Key types

**Pixel value types** (all `Copy + PartialEq + Debug`)

- `Rgb`, `Rgba` — 8-bit sRGB carriers. Both parse with `from_hex`; `Rgb` also
  formats with `to_hex`. `Rgb::from_hex` rejects the 4- and 8-digit
  alpha-bearing forms rather than silently dropping alpha, so parse `Rgba` when
  alpha matters.
- `LinearRgba` — linear-light RGBA, the form blending and compositing operate
  on. `from_hex_srgb` parses an sRGB hex string straight into linear space.
- `Hsv`, `Hsl`, `Oklab`, `Oklch` — perceptual and cylindrical spaces used by
  palettes, cross-fades, and control surfaces.

**Conversion and transfer**

- `srgb_to_linear`, `linear_to_srgb` — the sRGB transfer function, plus the
  `lut` module holding its table-driven 8-bit forms (`srgb_u8_to_linear`,
  `linear_to_srgb_u8`).
- `linear_to_output_u8` — the one canonical linear-to-8-bit projection.
- `wrap_hue` — folds a hue angle into `[0.0, 360.0)`, including the edge case
  where `rem_euclid` rounds back up to exactly `360.0`.
- `LUMA_R`, `LUMA_G`, `LUMA_B` — BT.709 luma coefficients.

**Blending and device output**

- `PixelBlendMode` — the blend modes the compositor and scene layers share.
- `DevicePixelLayout`, `EncodedChannels` — per-device channel ordering and the
  encoded byte payload drivers write to the wire.
- `ColorParseError` — the single error type for hex parsing.

## Feature flags

| Feature | What it gates |
|---|---|
| `serde` | `Serialize`/`Deserialize` on the value types |
| `schema` | `utoipa` OpenAPI schema derives |
| `default` | Empty. Both features are opt-in. |

---

Part of [Hypercolor](https://github.com/hyperb1iss/hypercolor) — open-source
RGB lighting orchestration for Linux, Windows, and macOS. Licensed under
Apache-2.0.
