import { afterEach, describe, expect, test } from 'bun:test'

import type { InjectedDisplayDescriptor } from '../src/faces/context'
import { injectedDisplayDescriptor, resolveDisplayInfo } from '../src/faces/context'
import { __testing } from '../src/faces/face-fn'

function descriptor(overrides: Partial<InjectedDisplayDescriptor> = {}): InjectedDisplayDescriptor {
    return {
        apiVersion: 1,
        circular: false,
        class: 'strip',
        height: 160,
        pixelFormat: 'rgb',
        safeArea: { height: 160, width: 960, x: 0, y: 0 },
        shape: 'wide',
        targetFps: 30,
        width: 960,
        ...overrides,
    }
}

afterEach(() => {
    Reflect.deleteProperty(globalThis, 'hypercolor')
})

describe('injectedDisplayDescriptor', () => {
    test('reads window.hypercolor.display when present', () => {
        Reflect.set(globalThis, 'hypercolor', { display: descriptor() })
        expect(injectedDisplayDescriptor()?.width).toBe(960)
    })

    test('returns undefined without injection', () => {
        expect(injectedDisplayDescriptor()).toBeUndefined()
    })

    test('rejects malformed payloads', () => {
        Reflect.set(globalThis, 'hypercolor', { display: { shape: 'wide' } })
        expect(injectedDisplayDescriptor()).toBeUndefined()
    })
})

describe('resolveDisplayInfo with descriptor', () => {
    test('descriptor wins over viewport and author flags', () => {
        const info = resolveDisplayInfo(480, 480, true, descriptor())

        expect(info.shape).toBe('wide')
        expect(info.class).toBe('strip')
        expect(info.circular).toBe(false)
        expect(info.aspect).toBeCloseTo(6)
        expect(info.safeArea).toEqual({ height: 160, width: 960, x: 0, y: 0 })
    })

    test('unknown shape/class tokens fall back to derivation', () => {
        const info = resolveDisplayInfo(100, 100, false, descriptor({ class: 'hologram', shape: 'pentagon' }))

        expect(info.shape).toBe('wide')
        expect(info.class).toBe('strip')
    })

    test('round descriptor carries the inscribed safe area', () => {
        const info = resolveDisplayInfo(
            100,
            100,
            false,
            descriptor({
                circular: true,
                class: 'pump-lcd',
                height: 480,
                safeArea: { height: 339, width: 339, x: 70, y: 70 },
                shape: 'round',
                width: 480,
            }),
        )

        expect(info.shape).toBe('round')
        expect(info.circular).toBe(true)
        expect(info.safeArea.width).toBe(339)
    })
})

describe('resolveDisplayInfo fallback derivation', () => {
    test('square viewport derives square panel', () => {
        const info = resolveDisplayInfo(480, 480, false, undefined)
        expect(info.shape).toBe('square')
        expect(info.class).toBe('panel')
        expect(info.safeArea).toEqual({ height: 480, width: 480, x: 0, y: 0 })
    })

    test('author circular declaration derives round with inscribed area', () => {
        const info = resolveDisplayInfo(480, 480, true, undefined)
        expect(info.shape).toBe('round')
        expect(info.class).toBe('pump-lcd')
        expect(info.circular).toBe(true)
        expect(info.safeArea).toEqual({ height: 339, width: 339, x: 70, y: 70 })
    })

    test('wide viewport derives strip at the 2:1 threshold', () => {
        expect(resolveDisplayInfo(960, 160, false, undefined).shape).toBe('wide')
        expect(resolveDisplayInfo(960, 481, false, undefined).shape).toBe('square')
        expect(resolveDisplayInfo(160, 960, false, undefined).shape).toBe('tall')
        expect(resolveDisplayInfo(160, 960, false, undefined).class).toBe('strip')
    })
})

describe('variant selection', () => {
    const base = () => () => {}
    const wide = () => () => {}
    const round = () => () => {}

    test('exact shape variant wins, base covers the rest', () => {
        const variants = { round, wide }
        expect(__testing.resolveVariantSetup(variants, 'wide', base)).toBe(wide)
        expect(__testing.resolveVariantSetup(variants, 'round', base)).toBe(round)
        expect(__testing.resolveVariantSetup(variants, 'square', base)).toBe(base)
        expect(__testing.resolveVariantSetup(variants, 'tall', base)).toBe(base)
    })

    test('no variants falls through to the base for every shape', () => {
        for (const shape of ['round', 'square', 'wide', 'tall'] as const) {
            expect(__testing.resolveVariantSetup(undefined, shape, base)).toBe(base)
        }
    })
})
