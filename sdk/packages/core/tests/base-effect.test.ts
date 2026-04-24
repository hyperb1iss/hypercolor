import { describe, expect, test } from 'bun:test'

import { __testing } from '../src/effects/base-effect'

describe('BaseEffect FPS cap cadence', () => {
    test('accepts near-boundary frames at 30fps', () => {
        const next = __testing.nextFpsCapFrameTime(33.32, 0, 30)

        expect(next).toBeCloseTo(1000 / 30, 6)
    })

    test('keeps cadence aligned after delayed frames', () => {
        const interval = 1000 / 30
        const first = __testing.nextFpsCapFrameTime(33.32, 0, 30)
        const second = __testing.nextFpsCapFrameTime(83.4, first ?? 0, 30)

        expect(second).toBeCloseTo(interval * 2, 6)
    })

    test('skips genuinely early frames', () => {
        expect(__testing.nextFpsCapFrameTime(16.67, 0, 30)).toBeNull()
    })
})
