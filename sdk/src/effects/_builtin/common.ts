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
