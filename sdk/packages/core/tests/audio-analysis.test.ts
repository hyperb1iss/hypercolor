import { afterEach, describe, expect, test } from 'bun:test'

import { getAudioData } from '../src/audio'

afterEach(() => {
    delete (globalThis as { engine?: unknown }).engine
})

describe('audio analysis contract', () => {
    test('reads the exact daemon audio level fields', () => {
        ;(globalThis as { engine?: unknown }).engine = {
            audio: {
                levelDb: -18,
                levelLinear: 10 ** (-18 / 20),
            },
        }

        const audio = getAudioData()
        expect(audio.levelLinear).toBeCloseTo(10 ** (-18 / 20), 5)
        expect(audio.levelDb).toBe(-18)
    })

    test('does not reinterpret the retired level alias', () => {
        ;(globalThis as { engine?: unknown }).engine = {
            audio: {
                level: 0.36,
            },
        }

        const audio = getAudioData()
        expect(audio.levelLinear).toBe(0)
        expect(audio.levelDb).toBe(-100)
    })
})
