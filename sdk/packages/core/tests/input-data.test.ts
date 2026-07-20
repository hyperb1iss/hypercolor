import { afterEach, describe, expect, test } from 'bun:test'

import { getInputData } from '../src/input'

afterEach(() => {
    delete (globalThis as { engine?: unknown }).engine
})

describe('input data contract', () => {
    test('returns an idle snapshot without an engine', () => {
        const input = getInputData()

        expect(input.available).toBeFalse()
        expect(input.dropped).toBe(0)
        expect(input.keyboard.keys).toEqual({})
        expect(input.keyboard.recent).toEqual([])
        expect(input.keyboard.events).toEqual([])
        expect(input.mouse.available).toBeFalse()
        expect(input.mouse.mode).toBe('none')
        expect(input.mouse.down).toBeFalse()
        expect(input.mouse.buttons).toEqual({})
        expect(input.mouse.events).toEqual([])
        expect(input.mouse.x).toBe(0)
        expect(input.mouse.y).toBe(0)
        expect(input.mouse.nx).toBe(0)
        expect(input.mouse.ny).toBe(0)
        expect(input.mouse.wheel).toBe(0)
        expect(input.mouse.velocity).toBe(0)
    })

    test('maps injected engine state into the typed snapshot', () => {
        ;(globalThis as { engine?: unknown }).engine = {
            inputDropped: 3,
            keyboard: {
                events: [
                    { atMs: 1000, key: 'a', kind: 'key', seq: 1, source: 'kbd0', state: 'pressed' },
                    { atMs: 1010, key: 'a', kind: 'key', seq: 2, source: 'kbd0', state: 'released' },
                ],
                keys: { a: true, ghost: false, KeyA: true },
                recent: ['a', 42, 'b'],
            },
            mouse: {
                buttons: { left: true },
                down: true,
                events: [
                    { atMs: 1005, button: 'left', kind: 'button', seq: 3, source: 'mouse0', state: 'pressed' },
                    { atMs: 1006, delta: 1.5, kind: 'wheel', seq: 4, source: 'mouse0' },
                ],
                mode: 'virtual',
                nx: 0.25,
                ny: 0.75,
                velocity: 0.4,
                wheel: 1.5,
                x: 320,
                y: 240,
            },
        }

        const input = getInputData()

        expect(input.available).toBeTrue()
        expect(input.dropped).toBe(3)
        expect(input.keyboard.keys).toEqual({ a: true, KeyA: true })
        expect(input.keyboard.recent).toEqual(['a', 'b'])
        expect(input.keyboard.events).toEqual([
            { atMs: 1000, key: 'a', kind: 'key', seq: 1, source: 'kbd0', state: 'pressed' },
            { atMs: 1010, key: 'a', kind: 'key', seq: 2, source: 'kbd0', state: 'released' },
        ])
        expect(input.mouse.available).toBeTrue()
        expect(input.mouse.mode).toBe('virtual')
        expect(input.mouse.down).toBeTrue()
        expect(input.mouse.buttons).toEqual({ left: true })
        expect(input.mouse.nx).toBe(0.25)
        expect(input.mouse.ny).toBe(0.75)
        expect(input.mouse.x).toBe(320)
        expect(input.mouse.y).toBe(240)
        expect(input.mouse.wheel).toBe(1.5)
        expect(input.mouse.velocity).toBe(0.4)
        expect(input.mouse.events).toEqual([
            { atMs: 1005, button: 'left', kind: 'button', seq: 3, source: 'mouse0', state: 'pressed' },
            { atMs: 1006, delta: 1.5, kind: 'wheel', seq: 4, source: 'mouse0' },
        ])
    })

    test('flags availability from keyboard activity alone', () => {
        ;(globalThis as { engine?: unknown }).engine = {
            keyboard: { events: [], keys: { w: true }, recent: [] },
        }

        const input = getInputData()

        expect(input.available).toBeTrue()
        expect(input.mouse.available).toBeFalse()
        expect(input.mouse.mode).toBe('none')
    })

    test('sanitizes malformed engine data', () => {
        ;(globalThis as { engine?: unknown }).engine = {
            inputDropped: 'lots',
            keyboard: { events: {}, keys: null, recent: 'nope' },
            mouse: { mode: 'weird', nx: Number.NaN, x: 12.7 },
        }

        const input = getInputData()

        expect(input.available).toBeFalse()
        expect(input.dropped).toBe(0)
        expect(input.keyboard.keys).toEqual({})
        expect(input.keyboard.recent).toEqual([])
        expect(input.keyboard.events).toEqual([])
        expect(input.mouse.mode).toBe('none')
        expect(input.mouse.nx).toBe(0)
        expect(input.mouse.x).toBe(12)
    })
})
