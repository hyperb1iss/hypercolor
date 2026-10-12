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
                ],
                keyboard: { keys: ['a'], recent: ['a'] },
                mouse: {
                    buttons: ['left'],
                    mode: 'virtual',
                    scroll: { line120X: 0.5, line120Y: -2, pixelX: 1.5, pixelY: -0.25 },
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
        ])
        expect(input.mouse.scroll).toEqual({ line120X: 0.5, line120Y: -2, pixelX: 1.5, pixelY: -0.25 })
    })

    test('keeps an idle source healthy and routed', () => {
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

        expect(input.healthy).toBeFalse()
        expect(input.fresh).toBeFalse()
        expect(input.keyboard.events).toHaveLength(1)
        expect(input.keyboard.keys).toEqual({ w: true })
    })

    test('carries device touches and buttons through the production adapter', () => {
        runtime.__hypercolorApplyFramePayload?.({
            canvas: { height: 200, width: 320 },
            interaction: {
                device: {
                    buttons: [{ button: 'mode', device: 'pad-1' }],
                    touches: [
                        {
                            canvas: { nx: 0.5, ny: 0.25, x: 160, y: 50.25 },
                            contact: 2,
                            device: 'pad-1',
                            pressure: 0.75,
                            x: 0.4,
                            y: 0.6,
                        },
                        { canvas: null, contact: 0, device: 'loose-pad', pressure: 0.5, x: 0.1, y: 0.9 },
                    ],
                },
                events: [
                    {
                        atMs: 3000,
                        canvas: { nx: 0.5, ny: 0.25, x: 160, y: 50.25 },
                        contact: 2,
                        device: 'pad-1',
                        kind: 'touch',
                        phase: 'began',
                        pressure: 0.75,
                        repeatCount: 1,
                        seq: 20,
                        source: 'device:pad-1',
                        x: 0.4,
                        y: 0.6,
                    },
                    {
                        atMs: 3001,
                        button: 'mode',
                        device: 'pad-1',
                        kind: 'device_button',
                        repeatCount: 1,
                        seq: 21,
                        source: 'device:pad-1',
                        state: 'pressed',
                    },
                    {
                        atMs: 3002,
                        key: 'a',
                        kind: 'key',
                        repeatCount: 1,
                        seq: 22,
                        source: 'kbd0',
                        state: 'pressed',
                    },
                ],
                keyboard: { keys: [], recent: [] },
                mouse: { mode: 'none' },
            },
            timing: { deltaSecs: 1 / 60, frameNumber: 10, timeSecs: 3 },
        })

        const input = getInputData()

        expect(input.touch.down).toBeTrue()
        expect(input.touch.touches).toEqual([
            {
                canvas: { nx: 0.5, ny: 0.25, x: 160, y: 50.25 },
                contact: 2,
                device: 'pad-1',
                pressure: 0.75,
                x: 0.4,
                y: 0.6,
            },
            { canvas: null, contact: 0, device: 'loose-pad', pressure: 0.5, x: 0.1, y: 0.9 },
        ])
        expect(input.touch.events).toEqual([
            {
                atMs: 3000,
                canvas: { nx: 0.5, ny: 0.25, x: 160, y: 50.25 },
                contact: 2,
                device: 'pad-1',
                kind: 'touch',
                phase: 'began',
                pressure: 0.75,
                repeatCount: 1,
                seq: 20,
                source: 'device:pad-1',
                x: 0.4,
                y: 0.6,
            },
        ])
        expect(input.deviceButtons.buttons).toEqual({ mode: true })
        expect(input.deviceButtons.held).toEqual([{ button: 'mode', device: 'pad-1' }])
        expect(input.deviceButtons.events).toEqual([
            {
                atMs: 3001,
                button: 'mode',
                device: 'pad-1',
                kind: 'device_button',
                repeatCount: 1,
                seq: 21,
                source: 'device:pad-1',
                state: 'pressed',
            },
        ])
        expect(input.keyboard.events).toHaveLength(1)
        expect(input.mouse.events).toEqual([])

        // An interaction payload without device holds releases them.
        runtime.__hypercolorApplyFramePayload?.({
            canvas: { height: 200, width: 320 },
            interaction: { keyboard: { keys: [], recent: [] }, mouse: { mode: 'none' } },
            timing: { deltaSecs: 1 / 60, frameNumber: 11, timeSecs: 3.1 },
        })

        const released = getInputData()
        expect(released.touch).toEqual({ down: false, events: [], touches: [] })
        expect(released.deviceButtons).toEqual({ buttons: {}, events: [], held: [] })
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
