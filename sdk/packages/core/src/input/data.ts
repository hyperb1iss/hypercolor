/**
 * Input data access — thin wrapper around the Hypercolor runtime.
 *
 * Capture and per-frame aggregation happen in the Rust daemon. Effects just
 * read the injected `engine.keyboard` / `engine.mouse` globals. This module
 * provides typed access and silent fallbacks when running outside the daemon.
 */

import { InputData, KeyboardInputState, KeyInputEvent, MouseInputEvent, MouseInputState, MouseMode } from './types'

/**
 * Get the interactive input snapshot from the Hypercolor runtime.
 * Returns an idle snapshot (`available: false`) when running outside the
 * daemon or when the effect has not declared `input: true`.
 */
export function getInputData(): InputData {
    const hasEngine = typeof engine !== 'undefined' && engine !== null
    const raw = hasEngine ? (engine as any) : undefined
    const keyboard = readKeyboard(raw?.keyboard)
    const mouse = readMouse(raw?.mouse)
    const keyboardActive =
        Object.keys(keyboard.keys).length > 0 || keyboard.recent.length > 0 || keyboard.events.length > 0

    return {
        available: mouse.available || keyboardActive,
        dropped: finiteNumber(raw?.inputDropped, 0),
        keyboard,
        mouse,
    }
}

function readKeyboard(raw: any): KeyboardInputState {
    if (typeof raw !== 'object' || raw === null) {
        return { events: [], keys: {}, recent: [] }
    }

    return {
        events: readKeyEvents(raw.events),
        keys: heldMap(raw.keys),
        recent: Array.isArray(raw.recent) ? raw.recent.filter((entry: unknown) => typeof entry === 'string') : [],
    }
}

function readMouse(raw: any): MouseInputState {
    if (typeof raw !== 'object' || raw === null) {
        return createIdleMouse()
    }

    const mode = readMouseMode(raw.mode)
    return {
        available: mode !== 'none',
        buttons: heldMap(raw.buttons),
        down: raw.down === true,
        events: readMouseEvents(raw.events),
        mode,
        nx: clamp01(finiteNumber(raw.nx, 0)),
        ny: clamp01(finiteNumber(raw.ny, 0)),
        velocity: finiteNumber(raw.velocity, 0),
        wheel: finiteNumber(raw.wheel, 0),
        x: Math.trunc(finiteNumber(raw.x, 0)),
        y: Math.trunc(finiteNumber(raw.y, 0)),
    }
}

function createIdleMouse(): MouseInputState {
    return {
        available: false,
        buttons: {},
        down: false,
        events: [],
        mode: 'none',
        nx: 0,
        ny: 0,
        velocity: 0,
        wheel: 0,
        x: 0,
        y: 0,
    }
}

function readKeyEvents(raw: unknown): KeyInputEvent[] {
    if (!Array.isArray(raw)) return []

    const events: KeyInputEvent[] = []
    for (const entry of raw as any[]) {
        if (typeof entry !== 'object' || entry === null || entry.kind !== 'key') continue
        events.push({
            atMs: finiteNumber(entry.atMs, 0),
            key: typeof entry.key === 'string' ? entry.key : '',
            kind: 'key',
            seq: finiteNumber(entry.seq, 0),
            source: typeof entry.source === 'string' ? entry.source : '',
            state: entry.state === 'released' || entry.state === 'repeated' ? entry.state : 'pressed',
        })
    }
    return events
}

function readMouseEvents(raw: unknown): MouseInputEvent[] {
    if (!Array.isArray(raw)) return []

    const events: MouseInputEvent[] = []
    for (const entry of raw as any[]) {
        if (typeof entry !== 'object' || entry === null) continue
        if (entry.kind === 'button') {
            events.push({
                atMs: finiteNumber(entry.atMs, 0),
                button: typeof entry.button === 'string' ? entry.button : '',
                kind: 'button',
                seq: finiteNumber(entry.seq, 0),
                source: typeof entry.source === 'string' ? entry.source : '',
                state: entry.state === 'released' ? 'released' : 'pressed',
            })
        } else if (entry.kind === 'wheel') {
            events.push({
                atMs: finiteNumber(entry.atMs, 0),
                delta: finiteNumber(entry.delta, 0),
                kind: 'wheel',
                seq: finiteNumber(entry.seq, 0),
                source: typeof entry.source === 'string' ? entry.source : '',
            })
        }
    }
    return events
}

function readMouseMode(raw: unknown): MouseMode {
    return raw === 'absolute' || raw === 'virtual' ? raw : 'none'
}

function heldMap(raw: any): Record<string, boolean> {
    if (typeof raw !== 'object' || raw === null) return {}

    const held: Record<string, boolean> = {}
    for (const key of Object.keys(raw)) {
        if (raw[key] === true) held[key] = true
    }
    return held
}

function finiteNumber(raw: unknown, fallback: number): number {
    return typeof raw === 'number' && Number.isFinite(raw) ? raw : fallback
}

function clamp01(value: number): number {
    return Math.max(0, Math.min(1, value))
}
