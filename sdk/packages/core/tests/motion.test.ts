import { describe, expect, test } from 'bun:test'

import {
    easeInOutCubic,
    easeOutBack,
    easeOutCubic,
    easeOutElastic,
    linear,
    Smoothed,
    Spring,
    Timeline,
    timeline,
    transitionOnChange,
    tween,
} from '../src/motion'

describe('easings', () => {
    const curves = { easeInOutCubic, easeOutBack, easeOutCubic, easeOutElastic, linear }

    test('all curves hit both endpoints exactly', () => {
        for (const [name, curve] of Object.entries(curves)) {
            expect(curve(0), `${name}(0)`).toBeCloseTo(0, 10)
            expect(curve(1), `${name}(1)`).toBeCloseTo(1, 10)
        }
    })

    test('monotonic curves never go backwards', () => {
        for (const curve of [linear, easeOutCubic, easeInOutCubic]) {
            let previous = curve(0)
            for (let i = 1; i <= 100; i++) {
                const next = curve(i / 100)
                expect(next).toBeGreaterThanOrEqual(previous)
                previous = next
            }
        }
    })

    test('overshoot curves stay bounded', () => {
        for (const curve of [easeOutBack, easeOutElastic]) {
            for (let i = 0; i <= 100; i++) {
                const value = curve(i / 100)
                expect(value).toBeGreaterThan(-0.5)
                expect(value).toBeLessThan(1.5)
            }
        }
    })
})

describe('tween', () => {
    test('maps elapsed time through the easing', () => {
        const track = tween(10, 20, 2, linear)
        expect(track.at(0)).toBe(10)
        expect(track.at(1)).toBeCloseTo(15)
        expect(track.at(2)).toBe(20)
        expect(track.at(99)).toBe(20)
        expect(track.at(-1)).toBe(10)
    })

    test('zero duration snaps to the target', () => {
        const track = tween(3, 7, 0)
        expect(track.at(0)).toBe(7)
        expect(track.done(0)).toBe(true)
    })

    test('done flips exactly at the duration', () => {
        const track = tween(0, 1, 0.5)
        expect(track.done(0.49)).toBe(false)
        expect(track.done(0.5)).toBe(true)
    })
})

describe('Smoothed', () => {
    test('halves the remaining distance every halflife', () => {
        const value = new Smoothed(0, 1)
        value.update(100, 1)
        expect(value.value).toBeCloseTo(50)
        value.update(100, 1)
        expect(value.value).toBeCloseTo(75)
    })

    test('converges identically at 15fps and 60fps', () => {
        const slow = new Smoothed(0, 0.25)
        const fast = new Smoothed(0, 0.25)
        const totalSeconds = 2

        for (let i = 0; i < totalSeconds * 15; i++) slow.update(100, 1 / 15)
        for (let i = 0; i < totalSeconds * 60; i++) fast.update(100, 1 / 60)

        expect(slow.value).toBeCloseTo(fast.value, 6)
    })

    test('zero halflife tracks the target exactly', () => {
        const value = new Smoothed(0, 0)
        expect(value.update(42, 1 / 60)).toBe(42)
    })

    test('snap jumps without smoothing', () => {
        const value = new Smoothed(0, 1)
        value.snap(9)
        expect(value.value).toBe(9)
    })
})

describe('Spring', () => {
    test('settles at the target', () => {
        const gauge = new Spring(0)
        for (let i = 0; i < 120; i++) gauge.update(1, 1 / 30)
        expect(gauge.value).toBeCloseTo(1, 3)
        expect(gauge.settled(1, 0.01)).toBe(true)
    })

    test('frame rate does not change where it lands', () => {
        const slow = new Spring(0)
        const fast = new Spring(0)
        for (let i = 0; i < 45; i++) slow.update(1, 1 / 15)
        for (let i = 0; i < 180; i++) fast.update(1, 1 / 60)
        expect(slow.value).toBeCloseTo(fast.value, 2)
    })

    test('snap zeroes velocity', () => {
        const gauge = new Spring(0)
        gauge.update(10, 0.1)
        gauge.snap(5)
        expect(gauge.value).toBe(5)
        expect(gauge.velocity).toBe(0)
    })
})

describe('Timeline', () => {
    test('segments report eased progress in order', () => {
        const intro = timeline().add('first', 0, 1, linear).add('second', 0.5, 1, linear)

        expect(intro.progress('first', 0.5)).toBeCloseTo(0.5)
        expect(intro.progress('second', 0.5)).toBe(0)
        expect(intro.progress('second', 1.0)).toBeCloseTo(0.5)
        expect(intro.progress('first', 2)).toBe(1)
        expect(intro.done(1.4)).toBe(false)
        expect(intro.done(1.5)).toBe(true)
        expect(intro.duration()).toBeCloseTo(1.5)
    })

    test('then() chains after the latest segment end', () => {
        const intro = new Timeline().add('a', 0, 1).then('b', 1, 0.25)
        expect(intro.progress('b', 1.24)).toBe(0)
        expect(intro.progress('b', 2.25)).toBe(1)
        expect(intro.duration()).toBeCloseTo(2.25)
    })

    test('unknown segments report zero progress', () => {
        expect(timeline().progress('missing', 5)).toBe(0)
    })
})

describe('transitionOnChange', () => {
    test('first update adopts the value immediately', () => {
        const eased = transitionOnChange(1)
        expect(eased.update(40, 100)).toBe(40)
    })

    test('a step change glides over the duration', () => {
        const eased = transitionOnChange(1, linear)
        eased.update(0, 0)
        expect(eased.update(10, 0)).toBe(0)
        expect(eased.update(10, 0.5)).toBeCloseTo(5)
        expect(eased.update(10, 1)).toBe(10)
        expect(eased.update(10, 5)).toBe(10)
    })

    test('retargeting mid-flight starts from the eased position', () => {
        const eased = transitionOnChange(1, linear)
        eased.update(0, 0)
        eased.update(10, 0)
        const midway = eased.update(10, 0.5)
        expect(midway).toBeCloseTo(5)

        // New target arrives halfway through: glide starts at 5, not 0 or 10.
        expect(eased.update(0, 0.5)).toBeCloseTo(5)
        expect(eased.update(0, 1.0)).toBeCloseTo(2.5)
        expect(eased.update(0, 1.5)).toBeCloseTo(0)
    })
})
