import { hslToRgb } from '@hypercolor/sdk'

export const BUILTIN_DESIGN_BASIS = { height: 200, width: 320 } as const

export interface Rgb {
    r: number
    g: number
    b: number
}

export function clamp(value: number, min: number, max: number): number {
    return Math.min(max, Math.max(min, value))
}

export function clamp01(value: number): number {
    return clamp(value, 0, 1)
}

export function lerp(a: number, b: number, t: number): number {
    return a + (b - a) * t
}

export function mixRgb(a: Rgb, b: Rgb, t: number): Rgb {
    const mix = clamp01(t)
    return {
        b: lerp(a.b, b.b, mix),
        g: lerp(a.g, b.g, mix),
        r: lerp(a.r, b.r, mix),
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
    return mixRgb(rgb, { b: 255, g: 255, r: 255 }, clamp01(amount))
}

export function rgbToCss(rgb: Rgb, alpha = 1): string {
    return `rgba(${Math.round(clamp(rgb.r, 0, 255))}, ${Math.round(clamp(rgb.g, 0, 255))}, ${Math.round(clamp(rgb.b, 0, 255))}, ${clamp01(alpha)})`
}

export function hexToRgb(hex: string): Rgb {
    const normalized = hex.trim().replace(/^#/, '')
    if (normalized.length !== 6) {
        return { b: 255, g: 255, r: 255 }
    }

    return {
        b: Number.parseInt(normalized.slice(4, 6), 16),
        g: Number.parseInt(normalized.slice(2, 4), 16),
        r: Number.parseInt(normalized.slice(0, 2), 16),
    }
}

export function hslCss(hue: number, saturation: number, lightness: number, alpha = 1): string {
    const [r, g, b] = hslToRgb(wrapHue(hue), clamp01(saturation / 100), clamp01(lightness / 100))
    return rgbToCss({ b: b * 255, g: g * 255, r: r * 255 }, alpha)
}

export function wrapHue(hue: number): number {
    return ((hue % 360) + 360) % 360
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
    const mix = clamp01(t)
    const from = rgbToOklab(a)
    const to = rgbToOklab(b)
    return oklabToRgb(lerp(from[0], to[0], mix), lerp(from[1], to[1], mix), lerp(from[2], to[2], mix))
}

export function easeInOutSine(value: number): number {
    return 0.5 - 0.5 * Math.cos(clamp01(value) * Math.PI)
}

export function pulse01(time: number): number {
    return 0.5 + 0.5 * Math.sin(time)
}

export function seededNoise(seed: number): number {
    const x = Math.sin(seed * 91.345 + 0.123) * 43758.5453
    return x - Math.floor(x)
}
