import { clamp, hslToRgb, mix, hexToRgb as parseHexColor, saturate } from 'hypercolor'

export { clamp, mix as lerp, saturate as clamp01, wrapHue } from 'hypercolor'

export const BUILTIN_DESIGN_BASIS = { height: 200, width: 320 } as const

export interface Rgb {
    r: number
    g: number
    b: number
}

const WHITE: Rgb = { b: 255, g: 255, r: 255 }

export function mixRgb(a: Rgb, b: Rgb, t: number): Rgb {
    const amount = saturate(t)
    return {
        b: mix(a.b, b.b, amount),
        g: mix(a.g, b.g, amount),
        r: mix(a.r, b.r, amount),
    }
}

export function scaleRgb(rgb: Rgb, scale: number): Rgb {
    return {
        b: clamp(rgb.b * scale, 0, 255),
        g: clamp(rgb.g * scale, 0, 255),
        r: clamp(rgb.r * scale, 0, 255),
    }
}

export function withLift(rgb: Rgb, amount: number): Rgb {
    return mixRgb(rgb, WHITE, saturate(amount))
}

export function rgbToCss(rgb: Rgb, alpha = 1): string {
    return `rgba(${Math.round(clamp(rgb.r, 0, 255))}, ${Math.round(clamp(rgb.g, 0, 255))}, ${Math.round(clamp(rgb.b, 0, 255))}, ${saturate(alpha)})`
}

export function hexToRgb(hex: string): Rgb {
    return parseHexColor(hex.trim(), WHITE)
}

export function hslCss(hue: number, saturation: number, lightness: number, alpha = 1): string {
    return rgbToCss(hslToRgb(hue, saturation / 100, lightness / 100), alpha)
}

// ── Oklab blending ───────────────────────────────────────────────────────
// Ported from the SDK palette runtime (packages/core/src/palette/runtime.ts)
// but operating on 0-255 Rgb values.

function srgbChannelToLinear(value: number): number {
    const n = clamp(value, 0, 255) / 255
    return n <= 0.04045 ? n / 12.92 : ((n + 0.055) / 1.055) ** 2.4
}

function linearChannelToSrgbByte(value: number): number {
    const s = value <= 0.0031308 ? value * 12.92 : 1.055 * value ** (1 / 2.4) - 0.055
    return clamp(Math.round(s * 255), 0, 255)
}

function rgbToOklab(rgb: Rgb): [number, number, number] {
    const lr = srgbChannelToLinear(rgb.r)
    const lg = srgbChannelToLinear(rgb.g)
    const lb = srgbChannelToLinear(rgb.b)

    const l = Math.cbrt(0.4122214708 * lr + 0.5363325363 * lg + 0.0514459929 * lb)
    const m = Math.cbrt(0.2119034982 * lr + 0.6806995451 * lg + 0.1073969566 * lb)
    const s = Math.cbrt(0.0883024619 * lr + 0.2817188376 * lg + 0.6299787005 * lb)

    return [
        0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s,
        1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s,
        0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s,
    ]
}

function oklabToRgb(lightness: number, a: number, b: number): Rgb {
    const l_ = lightness + 0.3963377774 * a + 0.2158037573 * b
    const m_ = lightness - 0.1055613458 * a - 0.0638541728 * b
    const s_ = lightness - 0.0894841775 * a - 1.291485548 * b

    const l = l_ * l_ * l_
    const m = m_ * m_ * m_
    const s = s_ * s_ * s_

    return {
        b: linearChannelToSrgbByte(-0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s),
        g: linearChannelToSrgbByte(-1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s),
        r: linearChannelToSrgbByte(4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s),
    }
}

/**
 * Blend two colors in Oklab space. Perceptually uniform — midpoints between
 * saturated hues stay vivid instead of collapsing into the muddy grays that
 * straight sRGB blending produces. Prefer this over [`mixRgb`] for gradients.
 */
export function mixOklab(a: Rgb, b: Rgb, t: number): Rgb {
    const amount = saturate(t)
    const from = rgbToOklab(a)
    const to = rgbToOklab(b)
    return oklabToRgb(mix(from[0], to[0], amount), mix(from[1], to[1], amount), mix(from[2], to[2], amount))
}

export function easeInOutSine(value: number): number {
    return 0.5 - 0.5 * Math.cos(saturate(value) * Math.PI)
}

export function pulse01(time: number): number {
    return 0.5 + 0.5 * Math.sin(time)
}

export function seededNoise(seed: number): number {
    const x = Math.sin(seed * 91.345 + 0.123) * 43758.5453
    return x - Math.floor(x)
}
