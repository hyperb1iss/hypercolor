/**
 * The SDK color kernel — the TypeScript mirror of the `hypercolor-color`
 * Rust crate (Spec 76 §1.6).
 *
 * Both implementations are fenced by `sdk/shared/color-vectors.json`: a
 * shared table of `(op, input, expected)` triples that each language runs
 * against its own kernel, so the two can never drift apart silently.
 *
 * Conventions, identical to the Rust crate (Spec 76 §1.1):
 *
 * - Hue is degrees, wrapped into `[0, 360)` at every entry point.
 * - Saturation, value, lightness and alpha are `0.0`–`1.0`. Percent and
 *   0–255 adapters live at call sites, never here.
 * - Float to byte is always round, then clamp.
 * - Hex parsing never invents a fallback color. The caller passes the one
 *   it wants, so a malformed string can never silently mean white.
 */

import { saturate } from '../math/lerp'

export { clamp, mix, saturate } from '../math/lerp'

/** Encoded sRGB, one byte per channel. */
export interface Rgb {
    r: number
    g: number
    b: number
}

/** Encoded sRGB with straight alpha, one byte per channel. */
export interface Rgba {
    r: number
    g: number
    b: number
    a: number
}

/** Linear-light RGB with straight alpha, `0.0`–`1.0` per channel. */
export interface LinearRgba {
    r: number
    g: number
    b: number
    a: number
}

/** Hue in degrees, saturation and value in `0.0`–`1.0`. */
export interface Hsv {
    h: number
    s: number
    v: number
}

/** Hue in degrees, saturation and lightness in `0.0`–`1.0`. */
export interface Hsl {
    h: number
    s: number
    l: number
}

/** Perceptual Oklab, alpha carried through unchanged. */
export interface Oklab {
    l: number
    a: number
    b: number
    alpha: number
}

/** BT.709 luma coefficients. */
export const LUMA_R = 0.2126
export const LUMA_G = 0.7152
export const LUMA_B = 0.0722

/**
 * Wrap a hue into `[0, 360)`.
 *
 * The `>= 360` guard mirrors the one in the Rust kernel, where a hue a
 * hair below zero makes `rem_euclid` round up to exactly 360. JavaScript's
 * double modulus cannot land there, so here the guard is the invariant
 * written down rather than a live correction.
 */
export function wrapHue(hue: number): number {
    const wrapped = ((hue % 360) + 360) % 360
    return wrapped >= 360 ? 0 : wrapped
}

/** The one canonical unit-float to byte conversion: round, then clamp. */
export function unitToByte(value: number): number {
    return Math.min(255, Math.max(0, Math.round(value * 255)))
}

/**
 * Shared sector decomposition for the chroma-based HSV and HSL algorithms:
 * a hue in `[0, 360)` with chroma `c` produces the unit RGB triple before
 * the match-lightness offset is added.
 */
function sectorRgb(h: number, c: number): [number, number, number] {
    const hp = h / 60
    const x = c * (1 - Math.abs((hp % 2) - 1))
    switch (Math.floor(hp)) {
        case 0:
            return [c, x, 0]
        case 1:
            return [x, c, 0]
        case 2:
            return [0, c, x]
        case 3:
            return [0, x, c]
        case 4:
            return [x, 0, c]
        default:
            return [c, 0, x]
    }
}

/** Hue from the max/min decomposition, in degrees, wrapped into `[0, 360)`. */
function hueFromDeltas(r: number, g: number, b: number, max: number, delta: number): number {
    if (delta === 0) return 0
    let h: number
    if (max === r) {
        const sixths = (g - b) / delta
        h = 60 * (((sixths % 6) + 6) % 6)
    } else if (max === g) {
        h = 60 * ((b - r) / delta + 2)
    } else {
        h = 60 * ((r - g) / delta + 4)
    }
    return wrapHue(h)
}

// ── Hex ──────────────────────────────────────────────────────────────────

/** Strip the single optional leading `#`, mirroring Rust's `strip_prefix`. */
function hexDigits(source: string): string {
    return source.startsWith('#') ? source.slice(1) : source
}

function nibble(code: number): number {
    if (code >= 0x30 && code <= 0x39) return code - 0x30
    if (code >= 0x61 && code <= 0x66) return code - 0x61 + 10
    if (code >= 0x41 && code <= 0x46) return code - 0x41 + 10
    return Number.NaN
}

function pair(digits: string, high: number, low: number): number {
    const hi = nibble(digits.charCodeAt(high))
    const lo = nibble(digits.charCodeAt(low))
    if (Number.isNaN(hi) || Number.isNaN(lo)) return Number.NaN
    return (hi << 4) | lo
}

/**
 * Parse the digit body into RGBA channels, or `null` when the grammar
 * rejects it. Shorthand forms without alpha digits imply full opacity.
 */
function parseChannels(digits: string): Rgba | null {
    let channels: Rgba
    switch (digits.length) {
        case 3:
            channels = { a: 255, b: pair(digits, 2, 2), g: pair(digits, 1, 1), r: pair(digits, 0, 0) }
            break
        case 4:
            channels = { a: pair(digits, 3, 3), b: pair(digits, 2, 2), g: pair(digits, 1, 1), r: pair(digits, 0, 0) }
            break
        case 6:
            channels = { a: 255, b: pair(digits, 4, 5), g: pair(digits, 2, 3), r: pair(digits, 0, 1) }
            break
        case 8:
            channels = { a: pair(digits, 6, 7), b: pair(digits, 4, 5), g: pair(digits, 2, 3), r: pair(digits, 0, 1) }
            break
        default:
            return null
    }
    if (Number.isNaN(channels.r) || Number.isNaN(channels.g) || Number.isNaN(channels.b) || Number.isNaN(channels.a)) {
        return null
    }
    return channels
}

/**
 * Parse a 3- or 6-digit hex color with an optional single leading `#`,
 * expanding CSS shorthand (`f80` becomes `ff8800`).
 *
 * Alpha-bearing forms (4 and 8 digits) are rejected rather than silently
 * dropping the alpha, matching `Rgb::from_hex` in the Rust kernel. Parse
 * with {@link hexToRgba} and decide about alpha explicitly instead.
 *
 * Anything the grammar rejects returns `fallback`, which the caller must
 * pass: there is no house fallback color, because a wrong one is
 * indistinguishable from a parse that worked.
 */
export function hexToRgb(source: string, fallback: Rgb): Rgb {
    const digits = hexDigits(source)
    if (digits.length !== 3 && digits.length !== 6) return fallback
    const parsed = parseChannels(digits)
    return parsed === null ? fallback : { b: parsed.b, g: parsed.g, r: parsed.r }
}

/**
 * Parse a 3-, 4-, 6-, or 8-digit hex color with an optional single leading
 * `#`, expanding CSS shorthand. Forms without alpha digits imply full
 * opacity. Anything the grammar rejects returns `fallback`.
 */
export function hexToRgba(source: string, fallback: Rgba): Rgba {
    return parseChannels(hexDigits(source)) ?? fallback
}

/** Format as a lowercase CSS hex color, `#rrggbb`. */
export function rgbToHex(rgb: Rgb): string {
    const channel = (value: number) =>
        Math.min(255, Math.max(0, Math.round(value)))
            .toString(16)
            .padStart(2, '0')
    return `#${channel(rgb.r)}${channel(rgb.g)}${channel(rgb.b)}`
}

// ── HSV and HSL ──────────────────────────────────────────────────────────

/**
 * HSV to encoded RGB bytes. Hue wraps into `[0, 360)`; saturation and
 * value clamp to `[0, 1]`.
 */
export function hsvToRgb(h: number, s: number, v: number): Rgb {
    const hue = wrapHue(h)
    const value = saturate(v)
    const chroma = value * saturate(s)
    const [r, g, b] = sectorRgb(hue, chroma)
    const m = value - chroma
    return { b: unitToByte(b + m), g: unitToByte(g + m), r: unitToByte(r + m) }
}

/**
 * HSL to encoded RGB bytes. Hue wraps into `[0, 360)`; saturation and
 * lightness clamp to `[0, 1]`.
 */
export function hslToRgb(h: number, s: number, l: number): Rgb {
    const [r, g, b] = hslToRgbUnit(h, s, l)
    return { b: unitToByte(b), g: unitToByte(g), r: unitToByte(r) }
}

/**
 * HSL to unit-float RGB — the same math as {@link hslToRgb}, stopping
 * before the byte quantization.
 *
 * This is the shape the audio helpers have always returned. Prefer
 * {@link hslToRgb} in new code: bytes are the RGB vocabulary everywhere
 * else in this module, so unit floats need a conversion at every boundary.
 */
export function hslToRgbUnit(h: number, s: number, l: number): [number, number, number] {
    const hue = wrapHue(h)
    const lightness = saturate(l)
    const chroma = (1 - Math.abs(2 * lightness - 1)) * saturate(s)
    const [r, g, b] = sectorRgb(hue, chroma)
    const m = lightness - chroma / 2
    return [r + m, g + m, b + m]
}

/** Encoded RGB bytes to HSV. */
export function rgbToHsv(rgb: Rgb): Hsv {
    const r = rgb.r / 255
    const g = rgb.g / 255
    const b = rgb.b / 255
    const max = Math.max(r, g, b)
    const min = Math.min(r, g, b)
    const delta = max - min
    return {
        h: hueFromDeltas(r, g, b, max, delta),
        s: max === 0 ? 0 : delta / max,
        v: max,
    }
}

/** Encoded RGB bytes to HSL. */
export function rgbToHsl(rgb: Rgb): Hsl {
    const r = rgb.r / 255
    const g = rgb.g / 255
    const b = rgb.b / 255
    const max = Math.max(r, g, b)
    const min = Math.min(r, g, b)
    const delta = max - min
    const l = (max + min) / 2
    return {
        h: hueFromDeltas(r, g, b, max, delta),
        l,
        s: delta === 0 ? 0 : delta / (1 - Math.abs(2 * l - 1)),
    }
}

// ── Brightness and transfer ──────────────────────────────────────────────

/** Scale every channel by `factor`, rounding then clamping to `[0, 255]`. */
export function scaleRgb(rgb: Rgb, factor: number): Rgb {
    const channel = (value: number) => Math.min(255, Math.max(0, Math.round(value * factor)))
    return { b: channel(rgb.b), g: channel(rgb.g), r: channel(rgb.r) }
}

/** Decode one sRGB gamma-encoded channel to linear light (IEC 61966-2-1). */
export function srgbToLinear(c: number): number {
    return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4
}

/** Encode one linear-light channel as sRGB. */
export function linearToSrgb(c: number): number {
    return c <= 0.0031308 ? c * 12.92 : 1.055 * c ** (1 / 2.4) - 0.055
}

/** Decode encoded RGB bytes into linear light, alpha `1.0`. */
export function rgbToLinear(rgb: Rgb): LinearRgba {
    return { a: 1, b: srgbToLinear(rgb.b / 255), g: srgbToLinear(rgb.g / 255), r: srgbToLinear(rgb.r / 255) }
}

/** Gamma-encode linear light back to RGBA bytes. */
export function linearToRgba(linear: LinearRgba): Rgba {
    return {
        a: unitToByte(linear.a),
        b: unitToByte(linearToSrgb(linear.b)),
        g: unitToByte(linearToSrgb(linear.g)),
        r: unitToByte(linearToSrgb(linear.r)),
    }
}

/** BT.709 relative luminance on linear values. */
export function linearLuma(linear: LinearRgba): number {
    return LUMA_R * linear.r + LUMA_G * linear.g + LUMA_B * linear.b
}

// ── Oklab ────────────────────────────────────────────────────────────────

/** Linear sRGB to Oklab (Björn Ottosson, 2020). Alpha passes through. */
export function linearToOklab(linear: LinearRgba): Oklab {
    const { r, g, b } = linear
    const l = Math.cbrt(0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b)
    const m = Math.cbrt(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b)
    const s = Math.cbrt(0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b)
    return {
        a: 1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s,
        alpha: linear.a,
        b: 0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s,
        l: 0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s,
    }
}

/**
 * Oklab back to linear sRGB. Out-of-gamut inputs produce channels outside
 * `[0, 1]` — clamp at the byte boundary, not here. Alpha passes through.
 */
export function oklabToLinear(lab: Oklab): LinearRgba {
    const l = lab.l + 0.3963377774 * lab.a + 0.2158037573 * lab.b
    const m = lab.l - 0.1055613458 * lab.a - 0.0638541728 * lab.b
    const s = lab.l - 0.0894841775 * lab.a - 1.291485548 * lab.b
    const l3 = l * l * l
    const m3 = m * m * m
    const s3 = s * s * s
    return {
        a: lab.alpha,
        b: -0.0041960863 * l3 - 0.7034186147 * m3 + 1.707614701 * s3,
        g: -1.2684380046 * l3 + 2.6097574011 * m3 - 0.3413193965 * s3,
        r: 4.0767416621 * l3 - 3.3077115913 * m3 + 0.2309699292 * s3,
    }
}
