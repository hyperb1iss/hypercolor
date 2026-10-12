import { afterEach, describe, expect, test } from 'bun:test'

import { getInputData } from '../src/input'

afterEach(() => {
    delete (globalThis as { engine?: unknown }).engine
})

describe('input data contract', () => {
    test('returns an idle snapshot without an engine', () => {
        const input = getInputData()

        expect(input.declared).toBeFalse()
        expect(input.routed).toBeFalse()
        expect(input.healthy).toBeFalse()
        expect(input.fresh).toBeFalse()
        expect(input.degraded).toBeFalse()
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
        expect(input.mouse.scroll).toEqual({ line120X: 0, line120Y: 0, pixelX: 0, pixelY: 0 })
        expect(input.mouse.velocity).toBe(0)
        expect(input.touch).toEqual({ down: false, events: [], touches: [] })
        expect(input.deviceButtons).toEqual({ buttons: {}, events: [], held: [] })
    })

    test('maps injected engine state into the typed snapshot', () => {
        ;(globalThis as { engine?: unknown }).engine = {
            inputAvailability: {
                declared: true,
                degraded: false,
                fresh: true,
                healthy: true,
                routed: true,
            },
            inputDropped: 3,
            keyboard: {
                events: [
                    {
                        atMs: 1000,
                        key: 'a',
                        kind: 'key',
                        physicalCode: 'evdev:key:30',
                        repeatCount: 4,
                        seq: 1,
                        source: 'kbd0',
                        state: 'pressed',
                    },
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
                    { atMs: 1006, button: 'left', kind: 'button', seq: 4, source: 'mouse0', state: 'repeated' },
                    {
                        atMs: 1007,
                        deltaX: 0.5,
                        deltaY: -0.25,
                        kind: 'scroll',
                        momentumPhase: 'began',
                        phase: 'changed',
                        physicalCode: 'macos:scroll',
                        seq: 5,
                        source: 'mouse0',
                        unit: 'pixels',
                    },
                ],
                mode: 'virtual',
                nx: 0.25,
                ny: 0.75,
                scroll: { line120X: 0.5, line120Y: -2, pixelX: 1.5, pixelY: -0.25 },
                velocity: 0.4,
                x: 320,
                y: 240,
            },
        }

        const input = getInputData()

        expect(input.declared).toBeTrue()
        expect(input.routed).toBeTrue()
        expect(input.healthy).toBeTrue()
        expect(input.fresh).toBeTrue()
        expect(input.degraded).toBeFalse()
        expect(input.dropped).toBe(3)
        expect(input.keyboard.keys).toEqual({ a: true, KeyA: true })
        expect(input.keyboard.recent).toEqual(['a', 'b'])
        expect(input.keyboard.events).toEqual([
            {
                atMs: 1000,
                key: 'a',
                kind: 'key',
                physicalCode: 'evdev:key:30',
                repeatCount: 4,
                seq: 1,
                source: 'kbd0',
                state: 'pressed',
            },
            {
                atMs: 1010,
                key: 'a',
                kind: 'key',
                repeatCount: 1,
                seq: 2,
                source: 'kbd0',
                state: 'released',
            },
        ])
        expect(input.mouse.available).toBeTrue()
        expect(input.mouse.mode).toBe('virtual')
        expect(input.mouse.down).toBeTrue()
        expect(input.mouse.buttons).toEqual({ left: true })
        expect(input.mouse.nx).toBe(0.25)
        expect(input.mouse.ny).toBe(0.75)
        expect(input.mouse.x).toBe(320)
        expect(input.mouse.y).toBe(240)
        expect(input.mouse.scroll).toEqual({ line120X: 0.5, line120Y: -2, pixelX: 1.5, pixelY: -0.25 })
        expect(input.mouse.velocity).toBe(0.4)
        expect(input.mouse.events).toEqual([
            {
                atMs: 1005,
                button: 'left',
                kind: 'button',
                repeatCount: 1,
                seq: 3,
                source: 'mouse0',
                state: 'pressed',
            },
            {
                atMs: 1006,
                button: 'left',
                kind: 'button',
                repeatCount: 1,
                seq: 4,
                source: 'mouse0',
                state: 'repeated',
            },
            {
                atMs: 1007,
                deltaX: 0.5,
                deltaY: -0.25,
                kind: 'scroll',
                momentumPhase: 'began',
                phase: 'changed',
                physicalCode: 'macos:scroll',
                repeatCount: 1,
                seq: 5,
                source: 'mouse0',
                unit: 'pixels',
            },
        ])
    })

    test('keeps an idle source healthy and routed', () => {
        ;(globalThis as { engine?: unknown }).engine = {
            inputAvailability: {
                declared: true,
                degraded: false,
                fresh: true,
                healthy: true,
                routed: true,
            },
            keyboard: { events: [], keys: {}, recent: [] },
        }

        const input = getInputData()

        expect(input.healthy).toBeTrue()
        expect(input.fresh).toBeTrue()
    })

    test('does not infer source health from recent activity', () => {
        ;(globalThis as { engine?: unknown }).engine = {
            keyboard: { events: [], keys: { w: true }, recent: [] },
        }

        const input = getInputData()

        expect(input.healthy).toBeFalse()
        expect(input.routed).toBeFalse()
        expect(input.mouse.available).toBeFalse()
        expect(input.mouse.mode).toBe('none')
    })

    test('reports recently active failed sources as unhealthy and stale', () => {
        ;(globalThis as { engine?: unknown }).engine = {
            inputAvailability: {
                declared: true,
                degraded: true,
                fresh: false,
                healthy: false,
                routed: true,
            },
            keyboard: {
                events: [{ atMs: 1000, key: 'w', kind: 'key', seq: 1, source: 'kbd0', state: 'pressed' }],
                keys: { w: true },
                recent: ['w'],
            },
            mouse: { mode: 'virtual', nx: 0.5, ny: 0.5 },
        }

        const input = getInputData()

        expect(input.declared).toBeTrue()
        expect(input.routed).toBeTrue()
        expect(input.healthy).toBeFalse()
        expect(input.fresh).toBeFalse()
        expect(input.degraded).toBeTrue()
        expect(input.keyboard.events).toHaveLength(1)
        expect(input.mouse.available).toBeTrue()
    })

    test('maps device touches and buttons with canvas positions', () => {
        ;(globalThis as { engine?: unknown }).engine = {
            deviceButtons: {
                buttons: { ghost: false, mode: true },
                events: [
                    {
                        atMs: 2002,
                        button: 'mode',
                        device: 'pad-1',
                        kind: 'device_button',
                        seq: 12,
                        source: 'device:pad-1',
                        state: 'pressed',
                    },
                    { atMs: 2003, button: 'left', kind: 'button', seq: 13, source: 'mouse0', state: 'pressed' },
                ],
                held: [{ button: 'mode', device: 'pad-1' }, { device: 'pad-2' }],
            },
            touch: {
                down: true,
                events: [
                    {
                        atMs: 2000,
                        canvas: { nx: 0.25, ny: 0.5, x: 80, y: 100 },
                        contact: 3,
                        device: 'pad-1',
                        kind: 'touch',
                        phase: 'began',
                        pressure: 0.6,
                        seq: 10,
                        source: 'device:pad-1',
                        x: 0.1,
                        y: 0.2,
                    },
                    {
                        atMs: 2001,
                        canvas: null,
                        contact: 0,
                        device: 'loose-pad',
                        kind: 'touch',
                        phase: 'cancelled',
                        pressure: 0,
                        seq: 11,
                        source: 'device:loose-pad',
                        x: 0.9,
                        y: 0.9,
                    },
                ],
                touches: [
                    {
                        canvas: { nx: 0.3, ny: 0.55, x: 96, y: 110 },
                        contact: 3,
                        device: 'pad-1',
                        pressure: 0.7,
                        x: 0.15,
                        y: 0.25,
                    },
                    { canvas: null, contact: 1, device: 'loose-pad', pressure: 0.4, x: 0.5, y: 0.5 },
                ],
            },
        }

        const input = getInputData()

        expect(input.touch.down).toBeTrue()
        expect(input.touch.touches).toEqual([
            {
                canvas: { nx: 0.3, ny: 0.55, x: 96, y: 110 },
                contact: 3,
                device: 'pad-1',
                pressure: 0.7,
                x: 0.15,
                y: 0.25,
            },
            { canvas: null, contact: 1, device: 'loose-pad', pressure: 0.4, x: 0.5, y: 0.5 },
        ])
        expect(input.touch.events).toEqual([
            {
                atMs: 2000,
                canvas: { nx: 0.25, ny: 0.5, x: 80, y: 100 },
                contact: 3,
                device: 'pad-1',
                kind: 'touch',
                phase: 'began',
                pressure: 0.6,
                repeatCount: 1,
                seq: 10,
                source: 'device:pad-1',
                x: 0.1,
                y: 0.2,
            },
            {
                atMs: 2001,
                canvas: null,
                contact: 0,
                device: 'loose-pad',
                kind: 'touch',
                phase: 'cancelled',
                pressure: 0,
                repeatCount: 1,
                seq: 11,
                source: 'device:loose-pad',
                x: 0.9,
                y: 0.9,
            },
        ])
        expect(input.deviceButtons.buttons).toEqual({ mode: true })
        expect(input.deviceButtons.held).toEqual([{ button: 'mode', device: 'pad-1' }])
        expect(input.deviceButtons.events).toEqual([
            {
                atMs: 2002,
                button: 'mode',
                device: 'pad-1',
                kind: 'device_button',
                repeatCount: 1,
                seq: 12,
                source: 'device:pad-1',
                state: 'pressed',
            },
        ])
    })

    test('sanitizes malformed device input', () => {
        ;(globalThis as { engine?: unknown }).engine = {
            deviceButtons: { buttons: 'mode', events: {}, held: 'mode' },
            touch: {
                events: [{ canvas: { nx: Number.NaN, ny: 0, x: 0, y: 0 }, kind: 'touch', phase: 'wobble', x: 4 }],
                touches: [null, { canvas: { nx: 0.5 }, contact: -2.5, pressure: 3, x: -1, y: 'top' }],
            },
        }

        const input = getInputData()

        expect(input.touch.touches).toEqual([{ canvas: null, contact: 0, device: '', pressure: 1, x: 0, y: 0 }])
        expect(input.touch.down).toBeTrue()
        expect(input.touch.events).toEqual([
            {
                atMs: 0,
                canvas: null,
                contact: 0,
                device: '',
                kind: 'touch',
                phase: 'began',
                pressure: 0,
                repeatCount: 1,
                seq: 0,
                source: '',
                x: 1,
                y: 0,
            },
        ])
        expect(input.deviceButtons).toEqual({ buttons: {}, events: [], held: [] })
    })

    test('sanitizes malformed engine data', () => {
        ;(globalThis as { engine?: unknown }).engine = {
            inputAvailability: {
                declared: 'yes',
                degraded: 1,
                fresh: {},
                healthy: 'yes',
                routed: [],
            },
            inputDropped: 'lots',
            keyboard: { events: {}, keys: null, recent: 'nope' },
            mouse: { mode: 'weird', nx: Number.NaN, x: 12.7 },
        }

        const input = getInputData()

        expect(input.declared).toBeFalse()
        expect(input.routed).toBeFalse()
        expect(input.healthy).toBeFalse()
        expect(input.fresh).toBeFalse()
        expect(input.degraded).toBeFalse()
        expect(input.dropped).toBe(0)
        expect(input.keyboard.keys).toEqual({})
        expect(input.keyboard.recent).toEqual([])
        expect(input.keyboard.events).toEqual([])
        expect(input.mouse.mode).toBe('none')
        expect(input.mouse.nx).toBe(0)
        expect(input.mouse.x).toBe(12)
        expect(input.mouse.scroll).toEqual({ line120X: 0, line120Y: 0, pixelX: 0, pixelY: 0 })
    })
})
