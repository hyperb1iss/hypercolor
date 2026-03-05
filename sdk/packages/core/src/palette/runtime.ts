/**
 * Palette runtime — provides palette-as-function for canvas effects.
 *
 * In canvas context, `palette(0.5)` returns a CSS color string.
 * Uses Oklab interpolation for perceptually uniform gradients.
 */

import palettesData from '../../../../shared/palettes.json'

// ── Types ────────────────────────────────────────────────────────────────

export interface PaletteEntry {
    readonly id: string
    readonly name: string
    readonly mood: readonly string[]
    readonly stops: readonly string[]
    readonly iq: { a: number[]; b: number[]; c: number[]; d: number[] }
    readonly accent: string
    readonly background: string
}

/** A palette function: takes t ∈ [0,1], optional alpha, returns CSS color. */
export type PaletteFn = (t: number, alpha?: number) => string

// ── Palette Registry ─────────────────────────────────────────────────────

const registry = new Map<string, PaletteEntry>()
for (const p of palettesData as PaletteEntry[]) {
    registry.set(p.name, p)
}

/** Get all palette names. */
export function paletteNames(): string[] {
    return Array.from(registry.keys())
}

/** Get a palette entry by name. */
export function getPalette(name: string): PaletteEntry | undefined {
    return registry.get(name)
}

// ── Color Math ───────────────────────────────────────────────────────────

/** Parse hex color to [r, g, b] in 0-1 range. */
function hexToRgb(hex: string): [number, number, number] {
    const h = hex.replace('#', '')
    return [
        Number.parseInt(h.slice(0, 2), 16) / 255,
        Number.parseInt(h.slice(2, 4), 16) / 255,
        Number.parseInt(h.slice(4, 6), 16) / 255,
    ]
}

/** Linear sRGB → Oklab (approximate). */
function srgbToOklab(r: number, g: number, b: number): [number, number, number] {
    // Linearize
    const lr = r <= 0.04045 ? r / 12.92 : ((r + 0.055) / 1.055) ** 2.4
    const lg = g <= 0.04045 ? g / 12.92 : ((g + 0.055) / 1.055) ** 2.4
    const lb = b <= 0.04045 ? b / 12.92 : ((b + 0.055) / 1.055) ** 2.4

    const l_ = Math.cbrt(0.4122214708 * lr + 0.5363325363 * lg + 0.0514459929 * lb)
    const m_ = Math.cbrt(0.2119034982 * lr + 0.6806995451 * lg + 0.1073969566 * lb)
    const s_ = Math.cbrt(0.0883024619 * lr + 0.2817188376 * lg + 0.6299787005 * lb)

    return [
        0.2104542553 * l_ + 0.7936177850 * m_ - 0.0040720468 * s_,
        1.9779984951 * l_ - 2.4285922050 * m_ + 0.4505937099 * s_,
        0.0259040371 * l_ + 0.7827717662 * m_ - 0.8086757660 * s_,
    ]
}

/** Oklab → linear sRGB → sRGB. */
function oklabToSrgb(L: number, a: number, b: number): [number, number, number] {
    const l_ = L + 0.3963377774 * a + 0.2158037573 * b
    const m_ = L - 0.1055613458 * a - 0.0638541728 * b
    const s_ = L - 0.0894841775 * a - 1.2914855480 * b

    const l = l_ * l_ * l_
    const m = m_ * m_ * m_
    const s = s_ * s_ * s_

    let r = +4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s
    let g = -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s
    let bl = -0.0041960863 * l - 0.7034186147 * m + 1.7076147010 * s

    // Gamma compress
    r = r <= 0.0031308 ? 12.92 * r : 1.055 * (r ** (1 / 2.4)) - 0.055
    g = g <= 0.0031308 ? 12.92 * g : 1.055 * (g ** (1 / 2.4)) - 0.055
    bl = bl <= 0.0031308 ? 12.92 * bl : 1.055 * (bl ** (1 / 2.4)) - 0.055

    return [
        Math.max(0, Math.min(1, r)),
        Math.max(0, Math.min(1, g)),
        Math.max(0, Math.min(1, bl)),
    ]
}

// ── LUT Cache ────────────────────────────────────────────────────────────

const LUT_SIZE = 256
const lutCache = new Map<string, [number, number, number][]>()

/** Build a 256-entry Oklab-interpolated LUT for a palette. */
function buildLut(palette: PaletteEntry): [number, number, number][] {
    const stops = palette.stops.map(hexToRgb)
    const oklabStops = stops.map(([r, g, b]) => srgbToOklab(r, g, b))
    const lut: [number, number, number][] = []

    for (let i = 0; i < LUT_SIZE; i++) {
        const t = i / (LUT_SIZE - 1)
        const segment = t * (oklabStops.length - 1)
        const idx = Math.min(Math.floor(segment), oklabStops.length - 2)
        const frac = segment - idx

        const a = oklabStops[idx]
        const b = oklabStops[idx + 1]
        const L = a[0] + (b[0] - a[0]) * frac
        const aa = a[1] + (b[1] - a[1]) * frac
        const bb = a[2] + (b[2] - a[2]) * frac

        lut.push(oklabToSrgb(L, aa, bb))
    }

    return lut
}

function getLut(palette: PaletteEntry): [number, number, number][] {
    let lut = lutCache.get(palette.name)
    if (!lut) {
        lut = buildLut(palette)
        lutCache.set(palette.name, lut)
    }
    return lut
}

// ── Public API ───────────────────────────────────────────────────────────

/**
 * Sample a palette at position t ∈ [0, 1].
 * Returns [r, g, b] in 0-1 range.
 */
export function samplePalette(paletteName: string, t: number): [number, number, number] {
    const entry = registry.get(paletteName)
    if (!entry) return [1, 0, 1] // magenta = missing palette

    const lut = getLut(entry)
    const clamped = Math.max(0, Math.min(1, t))
    const idx = Math.round(clamped * (LUT_SIZE - 1))
    return lut[idx]
}

/** Sample a palette and return a CSS color string. */
export function samplePaletteCSS(paletteName: string, t: number, alpha?: number): string {
    const [r, g, b] = samplePalette(paletteName, t)
    const ri = Math.round(r * 255)
    const gi = Math.round(g * 255)
    const bi = Math.round(b * 255)
    if (alpha !== undefined && alpha < 1) {
        return `rgba(${ri},${gi},${bi},${alpha.toFixed(3)})`
    }
    return `rgb(${ri},${gi},${bi})`
}

/**
 * Create a palette function for a given palette name.
 * The returned function takes t ∈ [0,1] and optional alpha, returns CSS string.
 */
export function createPaletteFn(paletteName: string): PaletteFn {
    return (t: number, alpha?: number) => samplePaletteCSS(paletteName, t, alpha)
}
