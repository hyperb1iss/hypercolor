/**
 * Input helper utilities — timestamp-driven, frame-rate-independent
 * building blocks computed from the per-frame event stream.
 */

import { KeyInputEvent } from './types'

export interface PressEnvelopeOptions {
    /** Attack ramp duration in milliseconds (press → 1). */
    attackMs?: number
    /** Decay ramp duration in milliseconds (release → 0). */
    decayMs?: number
}

interface EnvelopeEntry {
    pressedAtMs: number
    releasedAtMs: number | null
}

/**
 * Per-key attack/decay envelope tracker.
 *
 * Feed it `keyboard.events` every frame; envelopes evolve on event
 * timestamps (`atMs`), so values are independent of frame rate. A re-press
 * replaces the live envelope for that key.
 */
export class PressEnvelope {
    private readonly attackMs: number
    private readonly decayMs: number
    private readonly entries = new Map<string, EnvelopeEntry>()
    private nowMs = 0

    constructor(options: PressEnvelopeOptions = {}) {
        this.attackMs = Math.max(0, options.attackMs ?? 20)
        this.decayMs = Math.max(1, options.decayMs ?? 350)
    }

    /**
     * Consume this frame's key events. Pass `nowMs` (same monotonic clock as
     * event `atMs`) to keep decays advancing on event-free frames.
     */
    feed(events: readonly KeyInputEvent[], nowMs?: number): void {
        for (const event of events) {
            if (event.atMs > this.nowMs) this.nowMs = event.atMs

            if (event.state === 'pressed') {
                this.entries.set(event.key, { pressedAtMs: event.atMs, releasedAtMs: null })
            } else if (event.state === 'repeated') {
                const entry = this.entries.get(event.key)
                if (!entry || entry.releasedAtMs !== null) {
                    this.entries.set(event.key, { pressedAtMs: event.atMs, releasedAtMs: null })
                }
            } else {
                const entry = this.entries.get(event.key)
                if (entry && entry.releasedAtMs === null) entry.releasedAtMs = event.atMs
            }
        }

        if (nowMs !== undefined && nowMs > this.nowMs) this.nowMs = nowMs
        this.prune()
    }

    /** Envelope value for `key` in [0, 1]. */
    value(key: string): number {
        const entry = this.entries.get(key)
        return entry ? this.evaluate(entry) : 0
    }

    /** Sum of all live envelope values. */
    total(): number {
        let sum = 0
        for (const entry of this.entries.values()) {
            sum += this.evaluate(entry)
        }
        return sum
    }

    private evaluate(entry: EnvelopeEntry): number {
        const attackEnd = Math.min(this.nowMs, entry.releasedAtMs ?? this.nowMs)
        const attack =
            this.attackMs <= 0 ? 1 : Math.max(0, Math.min(1, (attackEnd - entry.pressedAtMs) / this.attackMs))
        if (entry.releasedAtMs === null) return attack

        const decay = Math.max(0, 1 - (this.nowMs - entry.releasedAtMs) / this.decayMs)
        return attack * decay
    }

    private prune(): void {
        for (const [key, entry] of this.entries) {
            if (entry.releasedAtMs !== null && this.nowMs - entry.releasedAtMs >= this.decayMs) {
                this.entries.delete(key)
            }
        }
    }
}

/** Create a per-key attack/decay envelope tracker. */
export function pressEnvelope(options?: PressEnvelopeOptions): PressEnvelope {
    return new PressEnvelope(options)
}

export interface TypingRateOptions {
    /** Sliding window length in milliseconds. */
    windowMs?: number
}

/**
 * Sliding-window typing rate tracker.
 *
 * Feed it `keyboard.events` every frame; the rate is computed from event
 * timestamps over a sliding window (~2s by default), independent of frame
 * rate.
 */
export class TypingRate {
    private readonly windowMs: number
    private timestamps: number[] = []
    private nowMs = 0

    constructor(options: TypingRateOptions = {}) {
        this.windowMs = Math.max(1, options.windowMs ?? 2000)
    }

    /**
     * Consume this frame's key events. Pass `nowMs` (same monotonic clock as
     * event `atMs`) to keep the window sliding on event-free frames.
     */
    feed(events: readonly KeyInputEvent[], nowMs?: number): void {
        for (const event of events) {
            if (event.atMs > this.nowMs) this.nowMs = event.atMs
            if (event.state !== 'pressed') continue
            this.timestamps.push(event.atMs)
        }

        if (nowMs !== undefined && nowMs > this.nowMs) this.nowMs = nowMs
        const cutoff = this.nowMs - this.windowMs
        this.timestamps = this.timestamps.filter((atMs) => atMs > cutoff)
    }

    /** Key presses per second over the sliding window. */
    rate(): number {
        return this.timestamps.length / (this.windowMs / 1000)
    }
}

/** Create a sliding-window typing rate tracker. */
export function typingRate(options?: TypingRateOptions): TypingRate {
    return new TypingRate(options)
}

const LEFT_KEYS = ['a', 'A', 'KeyA', 'ArrowLeft', 'Left'] as const
const RIGHT_KEYS = ['d', 'D', 'KeyD', 'ArrowRight', 'Right'] as const
const UP_KEYS = ['w', 'W', 'KeyW', 'ArrowUp', 'Up'] as const
const DOWN_KEYS = ['s', 'S', 'KeyS', 'ArrowDown', 'Down'] as const

function anyHeld(keys: Record<string, boolean>, names: readonly string[]): boolean {
    return names.some((name) => keys[name] === true)
}

/**
 * Movement vector from WASD and arrow keys.
 *
 * Returns `{ x, y }` with each axis in [-1, 1] using canvas convention:
 * positive x is right, positive y is down (W / ArrowUp yields y = -1).
 */
export function wasdVector(keys: Record<string, boolean>): { x: number; y: number } {
    const x = (anyHeld(keys, RIGHT_KEYS) ? 1 : 0) - (anyHeld(keys, LEFT_KEYS) ? 1 : 0)
    const y = (anyHeld(keys, DOWN_KEYS) ? 1 : 0) - (anyHeld(keys, UP_KEYS) ? 1 : 0)
    return { x, y }
}

const QWERTY_ROWS = ['`1234567890-=', 'qwertyuiop[]', "asdfghjkl;'", 'zxcvbnm,./'] as const
const QWERTY_ROW_OFFSETS = [0, 1.5, 1.75, 2.25] as const
const QWERTY_WIDTH = 13.5
const QWERTY_ROW_COUNT = 5

const CODE_STYLE_ALIASES: Record<string, string> = {
    Backquote: '`',
    BracketLeft: '[',
    BracketRight: ']',
    Comma: ',',
    Equal: '=',
    Minus: '-',
    Period: '.',
    Quote: "'",
    Semicolon: ';',
    Slash: '/',
    Space: ' ',
}

function normalizeKeyName(key: string): string | null {
    if (key.length === 1) return key.toLowerCase()
    if (/^Key[A-Z]$/.test(key)) return key.slice(3).toLowerCase()
    if (/^Digit[0-9]$/.test(key)) return key.slice(5)
    return CODE_STYLE_ALIASES[key] ?? null
}

/**
 * Approximate QWERTY grid position for a key.
 *
 * Returns `{ x, y }` normalized to [0, 1] for letters, digits, common
 * punctuation, and space; `null` for unknown keys. This is an approximate
 * layout projection (standard QWERTY with row stagger), not
 * physical-device truth — real key-to-LED spatial mapping needs the
 * device's own topology.
 */
export function keyToGridPosition(key: string): { x: number; y: number } | null {
    const normalized = normalizeKeyName(key)
    if (normalized === null) return null
    if (normalized === ' ') {
        return { x: 0.5, y: 1 }
    }

    for (let row = 0; row < QWERTY_ROWS.length; row++) {
        const col = QWERTY_ROWS[row].indexOf(normalized)
        if (col === -1) continue
        return {
            x: (QWERTY_ROW_OFFSETS[row] + col + 0.5) / QWERTY_WIDTH,
            y: row / (QWERTY_ROW_COUNT - 1),
        }
    }

    return null
}
