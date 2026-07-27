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
