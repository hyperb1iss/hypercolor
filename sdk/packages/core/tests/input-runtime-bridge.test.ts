import { afterAll, beforeEach, describe, expect, test } from 'bun:test'
import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'

import { getInputData } from '../src/input'

type RuntimeGlobal = typeof globalThis & {
    __hypercolorApplyFramePayload?: (payload: unknown) => void
    engine?: Record<string, unknown>
    window?: RuntimeGlobal
}

const runtime = globalThis as RuntimeGlobal
const adapterUrl = pathToFileURL(
    resolve(import.meta.dir, '../../../../crates/hypercolor-core/src/effect/lightscript/frame_payload_adapter.js'),
).href

runtime.window = runtime
await import(adapterUrl)

beforeEach(() => {
    runtime.engine = {}
})

afterAll(() => {
    delete runtime.__hypercolorApplyFramePayload
    delete runtime.engine
    delete runtime.window
})

describe('LightScript input availability bridge', () => {
    test('preserves physical codes and repeat multiplicity through the production adapter', () => {
        runtime.__hypercolorApplyFramePayload?.({
            canvas: { height: 200, width: 320 },
            interaction: {
                dropped: 0,
                events: [
                    {
                        atMs: 1000,
                        key: 'a',
                        kind: 'key',
                        physicalCode: 'evdev:key:30',
                        repeatCount: 3,
                        seq: 1,
                        source: 'kbd0',
                        state: 'repeated',
                    },
                    {
                        atMs: 1001,
                        button: 'left',
                        kind: 'button',
                        physicalCode: 'hid:button:1',
                        repeatCount: 2,
                        seq: 2,
                        source: 'mouse0',
                        state: 'pressed',
                    },
                    {
                        atMs: 1002,
                        deltaX: 0.5,
                        deltaY: -0.25,
                        kind: 'scroll',
                        momentumPhase: 'began',
                        phase: 'changed',
                        physicalCode: 'macos:scroll',
                        repeatCount: 1,
                        seq: 3,
                        source: 'mouse0',
                        unit: 'pixels',
                    },
                    { atMs: 1003, delta: -240, kind: 'wheel', repeatCount: 1, seq: 4, source: 'mouse0' },
                ],
                keyboard: { keys: ['a'], recent: ['a'] },
                mouse: {
                    buttons: ['left'],
                    mode: 'virtual',
                    scroll: { line120X: 0.5, line120Y: -2, pixelX: 1.5, pixelY: -0.25 },
                    wheel: -240,
                },
            },
            timing: { deltaSecs: 1 / 60, frameNumber: 8, timeSecs: 1 },
        })

        const input = getInputData()

        expect(input.keyboard.events).toEqual([
            {
                atMs: 1000,
                key: 'a',
                kind: 'key',
                physicalCode: 'evdev:key:30',
                repeatCount: 3,
                seq: 1,
                source: 'kbd0',
                state: 'repeated',
            },
        ])
        expect(input.mouse.events).toEqual([
            {
                atMs: 1001,
                button: 'left',
                kind: 'button',
                physicalCode: 'hid:button:1',
                repeatCount: 2,
                seq: 2,
                source: 'mouse0',
                state: 'pressed',
            },
            {
                atMs: 1002,
                deltaX: 0.5,
                deltaY: -0.25,
                kind: 'scroll',
                momentumPhase: 'began',
                phase: 'changed',
                physicalCode: 'macos:scroll',
                repeatCount: 1,
                seq: 3,
                source: 'mouse0',
                unit: 'pixels',
            },
            { atMs: 1003, delta: -240, kind: 'wheel', repeatCount: 1, seq: 4, source: 'mouse0' },
        ])
        expect(input.mouse.scroll).toEqual({ line120X: 0.5, line120Y: -2, pixelX: 1.5, pixelY: -0.25 })
        expect(input.mouse.wheel).toBe(-240)
    })

    test('keeps an idle healthy routed source available', () => {
        runtime.__hypercolorApplyFramePayload?.({
            canvas: { height: 200, width: 320 },
            inputAvailability: {
                declared: true,
                degraded: false,
                fresh: true,
                healthy: true,
                routed: true,
            },
            timing: { deltaSecs: 1 / 60, frameNumber: 8, timeSecs: 1 },
        })

        const input = getInputData()

        expect(input.available).toBeTrue()
        expect(input.declared).toBeTrue()
        expect(input.routed).toBeTrue()
        expect(input.healthy).toBeTrue()
        expect(input.fresh).toBeTrue()
        expect(input.degraded).toBeFalse()
        expect(input.keyboard.events).toEqual([])
    })

    test('does not let recent activity mask a failed stale source', () => {
        runtime.__hypercolorApplyFramePayload?.({
            canvas: { height: 200, width: 320 },
            inputAvailability: {
                declared: true,
                degraded: false,
                fresh: false,
                healthy: false,
                routed: true,
            },
            interaction: {
                dropped: 0,
                events: [
                    {
                        atMs: 1000,
                        key: 'w',
                        kind: 'key',
                        seq: 1,
                        source: 'kbd0',
                        state: 'pressed',
                    },
                ],
                keyboard: { keys: ['w'], recent: ['w'] },
                mouse: { mode: 'virtual', nx: 0.5, ny: 0.5 },
            },
            timing: { deltaSecs: 1 / 60, frameNumber: 9, timeSecs: 2 },
        })

        const input = getInputData()

        expect(input.available).toBeFalse()
        expect(input.healthy).toBeFalse()
        expect(input.fresh).toBeFalse()
        expect(input.keyboard.events).toHaveLength(1)
        expect(input.keyboard.keys).toEqual({ w: true })
    })

    test('normalizes malformed bridge values before publishing runtime state', () => {
        runtime.__hypercolorApplyFramePayload?.({
            canvas: {},
            inputAvailability: {
                declared: 'yes',
                degraded: 1,
                fresh: {},
                healthy: 'yes',
                routed: [],
            },
            timing: {},
        })

        expect(runtime.engine?.inputAvailability).toEqual({
            declared: false,
            degraded: false,
            fresh: false,
            healthy: false,
            routed: false,
        })
    })
})
