import { describe, expect, test } from 'bun:test'
import vectorFile from '../../../shared/color-vectors.json'
import type { Rgb } from '../src/color'
import {
    hexToRgb,
    hexToRgba,
    hslToRgb,
    hslToRgbUnit,
    hsvToRgb,
    linearToOklab,
    linearToSrgb,
    oklabToLinear,
    rgbToHex,
    rgbToHsl,
    rgbToHsv,
    scaleRgb,
    srgbToLinear,
    unitToByte,
    wrapHue,
} from '../src/color'

const BLACK: Rgb = { b: 0, g: 0, r: 0 }
const MAGENTA: Rgb = { b: 255, g: 0, r: 255 }

describe('hexToRgb grammar', () => {
    test('parses 6-digit forms with and without the hash', () => {
        expect(hexToRgb('#ff8800', BLACK)).toEqual({ b: 0, g: 136, r: 255 })
        expect(hexToRgb('ff8800', BLACK)).toEqual({ b: 0, g: 136, r: 255 })
    })

    test('expands CSS shorthand', () => {
        expect(hexToRgb('#F80', BLACK)).toEqual({ b: 0, g: 136, r: 255 })
        expect(hexToRgb('000', MAGENTA)).toEqual({ b: 0, g: 0, r: 0 })
    })

    test('is case insensitive', () => {
        expect(hexToRgb('#AbCdEf', BLACK)).toEqual({ b: 239, g: 205, r: 171 })
    })

    test('rejects alpha-bearing forms rather than dropping alpha', () => {
        expect(hexToRgb('#abcd', MAGENTA)).toEqual(MAGENTA)
        expect(hexToRgb('#ff8800ff', MAGENTA)).toEqual(MAGENTA)
    })

    test('rejects malformed input and returns the caller fallback', () => {
        for (const bad of ['', '#', '#ab', '#abcde', '#ff88001', '##fff', '#ggg', '#ff880g', '# f80']) {
            expect(hexToRgb(bad, MAGENTA)).toEqual(MAGENTA)
        }
    })

    test('strips only one leading hash', () => {
        expect(hexToRgb('##ff8800', BLACK)).toEqual(BLACK)
    })
})

describe('hexToRgba grammar', () => {
    test('accepts all four digit counts', () => {
        expect(hexToRgba('#ff8800', { a: 0, b: 0, g: 0, r: 0 })).toEqual({ a: 255, b: 0, g: 136, r: 255 })
        expect(hexToRgba('#ff880080', { a: 0, b: 0, g: 0, r: 0 })).toEqual({ a: 128, b: 0, g: 136, r: 255 })
        expect(hexToRgba('#f80c', { a: 0, b: 0, g: 0, r: 0 })).toEqual({ a: 204, b: 0, g: 136, r: 255 })
        expect(hexToRgba('#abcd', { a: 0, b: 0, g: 0, r: 0 })).toEqual({ a: 221, b: 204, g: 187, r: 170 })
    })

    test('forms without alpha digits imply full opacity', () => {
        expect(hexToRgba('#fff', { a: 0, b: 0, g: 0, r: 0 }).a).toBe(255)
    })

    test('rejects malformed input', () => {
        const fallback = { a: 1, b: 2, g: 3, r: 4 }
        expect(hexToRgba('#ab', fallback)).toEqual(fallback)
        expect(hexToRgba('zzzz', fallback)).toEqual(fallback)
    })
})

describe('rgbToHex', () => {
    test('formats lowercase #rrggbb', () => {
        expect(rgbToHex({ b: 0, g: 136, r: 255 })).toBe('#ff8800')
        expect(rgbToHex({ b: 0, g: 0, r: 0 })).toBe('#000000')
        expect(rgbToHex({ b: 239, g: 205, r: 171 })).toBe('#abcdef')
    })

    test('rounds and clamps out-of-range channels', () => {
        expect(rgbToHex({ b: -12, g: 300, r: 127.6 })).toBe('#80ff00')
    })

    test('round-trips through hexToRgb', () => {
        for (const hex of ['#000000', '#ffffff', '#ff8800', '#123456']) {
            expect(rgbToHex(hexToRgb(hex, BLACK))).toBe(hex)
        }
    })
})

describe('hue wrapping', () => {
    test('wrapHue lands in [0, 360)', () => {
        expect(wrapHue(0)).toBe(0)
        expect(wrapHue(360)).toBe(0)
        expect(wrapHue(720)).toBe(0)
        expect(wrapHue(-60)).toBe(300)
        expect(wrapHue(-360)).toBe(0)
        expect(wrapHue(420)).toBe(60)
        expect(wrapHue(539.5)).toBe(179.5)
    })

    test('never escapes the range, however close to a full turn', () => {
        for (const hue of [-1e-12, -Number.MIN_VALUE, -1e-9, 359.9999999, 1e12 * 360 - 1e-6]) {
            const wrapped = wrapHue(hue)
            expect(wrapped).toBeGreaterThanOrEqual(0)
            expect(wrapped).toBeLessThan(360)
        }
    })

    test('converters wrap at entry', () => {
        expect(hsvToRgb(360, 1, 1)).toEqual(hsvToRgb(0, 1, 1))
        expect(hsvToRgb(420, 1, 1)).toEqual(hsvToRgb(60, 1, 1))
        expect(hsvToRgb(-60, 1, 1)).toEqual(hsvToRgb(300, 1, 1))
        expect(hslToRgb(720, 1, 0.5)).toEqual(hslToRgb(0, 1, 0.5))
    })
})

describe('hsvToRgb and hslToRgb', () => {
    test('hits the sector corners', () => {
        expect(hsvToRgb(0, 1, 1)).toEqual({ b: 0, g: 0, r: 255 })
        expect(hsvToRgb(120, 1, 1)).toEqual({ b: 0, g: 255, r: 0 })
        expect(hsvToRgb(240, 1, 1)).toEqual({ b: 255, g: 0, r: 0 })
    })

    test('clamps saturation and value', () => {
        expect(hsvToRgb(0, 5, 5)).toEqual({ b: 0, g: 0, r: 255 })
        expect(hsvToRgb(0, -1, 1)).toEqual({ b: 255, g: 255, r: 255 })
        expect(hslToRgb(200, -3, 4)).toEqual({ b: 255, g: 255, r: 255 })
    })

    test('hslToRgbUnit is hslToRgb before quantization', () => {
        const unit = hslToRgbUnit(240, 0.5, 0.75)
        expect(hslToRgb(240, 0.5, 0.75)).toEqual({
            b: Math.round(unit[2] * 255),
            g: Math.round(unit[1] * 255),
            r: Math.round(unit[0] * 255),
        })
    })
})

describe('rgbToHsv and rgbToHsl', () => {
    test('inverts the primaries', () => {
        expect(rgbToHsv({ b: 0, g: 0, r: 255 })).toEqual({ h: 0, s: 1, v: 1 })
        expect(rgbToHsv({ b: 0, g: 255, r: 0 })).toEqual({ h: 120, s: 1, v: 1 })
        expect(rgbToHsl({ b: 255, g: 0, r: 0 })).toEqual({ h: 240, l: 0.5, s: 1 })
    })

    test('greys report zero hue and zero saturation', () => {
        expect(rgbToHsv({ b: 128, g: 128, r: 128 })).toEqual({ h: 0, s: 0, v: 128 / 255 })
        expect(rgbToHsl({ b: 255, g: 255, r: 255 })).toEqual({ h: 0, l: 1, s: 0 })
    })

    test('round-trips through hsvToRgb', () => {
        for (const rgb of [{ b: 0, g: 136, r: 255 }, { b: 200, g: 12, r: 33 }, BLACK]) {
            const hsv = rgbToHsv(rgb)
            expect(hsvToRgb(hsv.h, hsv.s, hsv.v)).toEqual(rgb)
        }
    })
})

describe('float to byte rounding', () => {
    test('rounds at the half then clamps to [0, 255]', () => {
        expect(unitToByte(0)).toBe(0)
        expect(unitToByte(1)).toBe(255)
        expect(unitToByte(0.5)).toBe(128)
        expect(unitToByte(127.5 / 255)).toBe(128)
        expect(unitToByte(-4)).toBe(0)
        expect(unitToByte(9)).toBe(255)
    })

    test('scaleRgb rounds then clamps', () => {
        expect(scaleRgb({ b: 255, g: 255, r: 255 }, 0.5)).toEqual({ b: 128, g: 128, r: 128 })
        expect(scaleRgb({ b: 1, g: 1, r: 1 }, 0.5)).toEqual({ b: 1, g: 1, r: 1 })
        expect(scaleRgb({ b: 200, g: 200, r: 200 }, 2)).toEqual({ b: 255, g: 255, r: 255 })
        expect(scaleRgb({ b: 100, g: 100, r: 100 }, -1)).toEqual({ b: 0, g: 0, r: 0 })
    })
})

describe('oklab', () => {
    test('round-trips linear values', () => {
        const linear = { a: 0.5, b: 0.75, g: 0.25, r: 0.5 }
        const back = oklabToLinear(linearToOklab(linear))
        expect(back.r).toBeCloseTo(linear.r, 5)
        expect(back.g).toBeCloseTo(linear.g, 5)
        expect(back.b).toBeCloseTo(linear.b, 5)
        expect(back.a).toBe(linear.a)
    })

    test('white is achromatic', () => {
        const lab = linearToOklab({ a: 1, b: 1, g: 1, r: 1 })
        expect(lab.l).toBeCloseTo(1, 3)
        expect(lab.a).toBeCloseTo(0, 3)
        expect(lab.b).toBeCloseTo(0, 3)
    })
})

// ── Shared-vector agreement ──────────────────────────────────────────────
// The Rust kernel and this module run the same table. Every op the module
// implements is fenced here; wave 1.5 grows the table itself.

interface Vector {
    op: string
    input: unknown
    expected: unknown
    tol?: number
}

const SENTINEL: Rgb = { b: 7, g: 7, r: 7 }
const SENTINEL_RGBA = { a: 7, b: 7, g: 7, r: 7 }

type Triple = [number, number, number]
type Quad = [number, number, number, number]

function toLinear(input: Quad) {
    const [r, g, b, a] = input
    return { a, b, g, r }
}

function runVector(vector: Vector): unknown {
    switch (vector.op) {
        case 'hex_to_rgb': {
            const parsed = hexToRgb(vector.input as string, SENTINEL)
            return parsed === SENTINEL ? null : [parsed.r, parsed.g, parsed.b]
        }
        case 'hex_to_rgba': {
            const parsed = hexToRgba(vector.input as string, SENTINEL_RGBA)
            return parsed === SENTINEL_RGBA ? null : [parsed.r, parsed.g, parsed.b, parsed.a]
        }
        case 'rgb_to_hex': {
            const [r, g, b] = vector.input as Triple
            return rgbToHex({ b, g, r })
        }
        case 'hsv_to_rgb': {
            const [h, s, v] = vector.input as Triple
            const rgb = hsvToRgb(h, s, v)
            return [rgb.r, rgb.g, rgb.b]
        }
        case 'hsl_to_rgb': {
            const [h, s, l] = vector.input as Triple
            const rgb = hslToRgb(h, s, l)
            return [rgb.r, rgb.g, rgb.b]
        }
        case 'rgb_to_hsv': {
            const [r, g, b] = vector.input as Triple
            const hsv = rgbToHsv({ b, g, r })
            return [hsv.h, hsv.s, hsv.v]
        }
        case 'rgb_to_hsl': {
            const [r, g, b] = vector.input as Triple
            const hsl = rgbToHsl({ b, g, r })
            return [hsl.h, hsl.s, hsl.l]
        }
        case 'srgb_to_linear':
            return srgbToLinear(vector.input as number)
        case 'linear_to_srgb':
            return linearToSrgb(vector.input as number)
        case 'oklab_from_linear': {
            const lab = linearToOklab(toLinear(vector.input as Quad))
            return [lab.l, lab.a, lab.b, lab.alpha]
        }
        case 'oklab_roundtrip': {
            const back = oklabToLinear(linearToOklab(toLinear(vector.input as Quad)))
            return [back.r, back.g, back.b, back.a]
        }
        case 'scale_rgb': {
            const [[r, g, b], factor] = vector.input as [Triple, number]
            const scaled = scaleRgb({ b, g, r }, factor)
            return [scaled.r, scaled.g, scaled.b]
        }
        default:
            throw new Error(`unhandled vector op: ${vector.op}`)
    }
}

/** Hue components compare by shortest angular distance, per the file's notes. */
function hueDelta(a: number, b: number): number {
    const raw = Math.abs(a - b) % 360
    return raw > 180 ? 360 - raw : raw
}

const HUE_FIRST_OPS = new Set(['rgb_to_hsv', 'rgb_to_hsl'])

function expectVector(vector: Vector, actual: unknown, tolerance: number): void {
    if (vector.expected === null || typeof vector.expected === 'string') {
        expect(actual).toEqual(vector.expected)
        return
    }
    if (typeof vector.expected === 'number') {
        expect(Math.abs((actual as number) - vector.expected)).toBeLessThanOrEqual(tolerance)
        return
    }
    const expected = vector.expected as number[]
    const got = actual as number[]
    expect(got).toHaveLength(expected.length)
    expected.forEach((value, index) => {
        const delta =
            index === 0 && HUE_FIRST_OPS.has(vector.op)
                ? hueDelta(got[index] as number, value)
                : Math.abs((got[index] as number) - value)
        expect(delta).toBeLessThanOrEqual(tolerance)
    })
}

describe('shared color vectors', () => {
    const vectors = vectorFile.vectors as Vector[]
    const defaultTolerance = vectorFile.default_tolerance

    test('the whole table is covered by an op handler', () => {
        expect(vectors.length).toBeGreaterThan(60)
        for (const vector of vectors) {
            expect(() => runVector(vector)).not.toThrow()
        }
    })

    for (const vector of vectors) {
        test(`${vector.op}(${JSON.stringify(vector.input)})`, () => {
            expectVector(vector, runVector(vector), vector.tol ?? defaultTolerance)
        })
    }
})
