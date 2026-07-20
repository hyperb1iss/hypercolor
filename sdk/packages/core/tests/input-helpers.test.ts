import { describe, expect, test } from 'bun:test'

import type { KeyInputEvent } from '../src/input'
import { keyToGridPosition, pressEnvelope, typingRate, wasdVector } from '../src/input'

let sequence = 0

function keyEvent(key: string, state: KeyInputEvent['state'], atMs: number): KeyInputEvent {
    sequence += 1
    return { atMs, key, kind: 'key', seq: sequence, source: 'kbd0', state }
}

describe('pressEnvelope', () => {
    test('ramps attack, decays after release, and prunes dead envelopes', () => {
        const envelope = pressEnvelope({ attackMs: 100, decayMs: 200 })

        envelope.feed([keyEvent('a', 'pressed', 1000)])
        expect(envelope.value('a')).toBe(0)

        envelope.feed([], 1050)
        expect(envelope.value('a')).toBeCloseTo(0.5, 5)

        envelope.feed([], 1100)
        expect(envelope.value('a')).toBe(1)

        envelope.feed([keyEvent('a', 'released', 1100)])
        expect(envelope.value('a')).toBe(1)

        envelope.feed([], 1200)
        expect(envelope.value('a')).toBeCloseTo(0.5, 5)

        envelope.feed([], 1300)
        expect(envelope.value('a')).toBe(0)
        expect(envelope.total()).toBe(0)
    })

    test('caps attack at the level reached when released early', () => {
        const envelope = pressEnvelope({ attackMs: 100, decayMs: 200 })

        envelope.feed([keyEvent('b', 'pressed', 2000), keyEvent('b', 'released', 2050)])
        expect(envelope.value('b')).toBeCloseTo(0.5, 5)

        envelope.feed([], 2150)
        expect(envelope.value('b')).toBeCloseTo(0.25, 5)
    })

    test('totals across live keys and re-press replaces the live envelope', () => {
        const envelope = pressEnvelope({ attackMs: 100, decayMs: 200 })

        envelope.feed([keyEvent('a', 'pressed', 0), keyEvent('b', 'pressed', 0)], 100)
        expect(envelope.total()).toBe(2)

        envelope.feed([keyEvent('a', 'pressed', 100)])
        expect(envelope.value('a')).toBe(0)
        expect(envelope.value('b')).toBe(1)
        expect(envelope.total()).toBe(1)
    })
})

describe('typingRate', () => {
    test('reports keys per second over the sliding window', () => {
        const tracker = typingRate({ windowMs: 2000 })

        tracker.feed([
            keyEvent('a', 'pressed', 0),
            keyEvent('b', 'pressed', 100),
            keyEvent('b', 'released', 150),
            keyEvent('c', 'pressed', 200),
            keyEvent('d', 'pressed', 300),
            keyEvent('e', 'pressed', 400),
        ])
        expect(tracker.rate()).toBeCloseTo(2.5, 5)

        tracker.feed([], 2200)
        expect(tracker.rate()).toBeCloseTo(1, 5)

        tracker.feed([], 5000)
        expect(tracker.rate()).toBe(0)
    })
})

describe('wasdVector', () => {
    test('returns zero with no movement keys held', () => {
        expect(wasdVector({})).toEqual({ x: 0, y: 0 })
    })

    test('maps WASD, arrows, and alias forms to canvas-convention axes', () => {
        expect(wasdVector({ w: true })).toEqual({ x: 0, y: -1 })
        expect(wasdVector({ KeyD: true })).toEqual({ x: 1, y: 0 })
        expect(wasdVector({ ArrowDown: true })).toEqual({ x: 0, y: 1 })
        expect(wasdVector({ ArrowUp: true, a: true })).toEqual({ x: -1, y: -1 })
    })

    test('opposing keys cancel out', () => {
        expect(wasdVector({ a: true, d: true })).toEqual({ x: 0, y: 0 })
        expect(wasdVector({ ArrowUp: true, s: true })).toEqual({ x: 0, y: 0 })
    })
})

describe('keyToGridPosition', () => {
    test('projects known keys into the normalized grid', () => {
        const a = keyToGridPosition('a')
        expect(a).not.toBeNull()
        expect(a?.x).toBeCloseTo(2.25 / 13.5, 5)
        expect(a?.y).toBeCloseTo(0.5, 5)

        const q = keyToGridPosition('KeyQ')
        expect(q?.x).toBeCloseTo(2 / 13.5, 5)
        expect(q?.y).toBeCloseTo(0.25, 5)

        const one = keyToGridPosition('Digit1')
        expect(one?.x).toBeCloseTo(1.5 / 13.5, 5)
        expect(one?.y).toBe(0)

        expect(keyToGridPosition('Space')).toEqual({ x: 0.5, y: 1 })
    })

    test('positions stay in [0, 1] and advance left to right along a row', () => {
        const home = ['a', 's', 'd', 'f', 'g', 'h', 'j', 'k', 'l']
        let previousX = -1
        for (const key of home) {
            const position = keyToGridPosition(key)
            expect(position).not.toBeNull()
            expect(position?.x).toBeGreaterThan(previousX)
            expect(position?.x).toBeGreaterThanOrEqual(0)
            expect(position?.x).toBeLessThanOrEqual(1)
            expect(position?.y).toBeCloseTo(0.5, 5)
            previousX = position?.x ?? previousX
        }
    })

    test('returns null for keys outside the projection', () => {
        expect(keyToGridPosition('Escape')).toBeNull()
        expect(keyToGridPosition('F13')).toBeNull()
        expect(keyToGridPosition('MediaPlayPause')).toBeNull()
    })
})
