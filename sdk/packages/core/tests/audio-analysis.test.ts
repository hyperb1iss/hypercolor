import { afterEach, describe, expect, test } from 'bun:test'

import { getAudioData, normalizeAudioLevel } from '../src/audio'

afterEach(() => {
    delete (globalThis as { engine?: unknown }).engine
})

describe('audio analysis contract', () => {
    test('normalizes daemon dB levels to linear amplitude and preserves normalized levels', () => {
        expect(normalizeAudioLevel(-20)).toBeCloseTo(0.1, 5)
        expect(normalizeAudioLevel(0.42)).toBeCloseTo(0.42, 5)
    })

    test('reads daemon-style audio levels from levelLinear while preserving raw dB', () => {
        ;(globalThis as { engine?: unknown }).engine = {
            audio: {
                level: -18,
                levelLinear: 10 ** (-18 / 20),
                levelRaw: -18,
            },
        }

        const audio = getAudioData()
        expect(audio.level).toBeCloseTo(10 ** (-18 / 20), 5)
        expect(audio.levelRaw).toBe(-18)
    })

    test('reads dev-shell normalized audio levels without saturating', () => {
        ;(globalThis as { engine?: unknown }).engine = {
            audio: {
                level: 0.36,
            },
        }

        const audio = getAudioData()
        expect(audio.level).toBeCloseTo(0.36, 5)
        expect(audio.levelRaw).toBeCloseTo(20 * Math.log10(0.36), 5)
    })

    test('falls back to dB conversion when levelLinear is absent', () => {
        ;(globalThis as { engine?: unknown }).engine = {
            audio: {
                level: -12,
            },
        }

        const audio = getAudioData()
        expect(audio.level).toBeCloseTo(10 ** (-12 / 20), 5)
        expect(audio.levelRaw).toBe(-12)
    })
})
