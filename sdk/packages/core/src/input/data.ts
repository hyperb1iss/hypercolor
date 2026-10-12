/**
 * Input data access — thin wrapper around the Hypercolor runtime.
 *
 * Capture and per-frame aggregation happen in the Rust daemon. Effects just
 * read the injected `engine.keyboard`, `engine.mouse`, `engine.touch`, and
 * `engine.deviceButtons` globals. This module provides typed access and
 * silent fallbacks when running outside the daemon.
 */

import {
    DeviceButtonHold,
    DeviceButtonInputEvent,
    DeviceButtonInputState,
    InputAvailability,
    InputData,
    KeyboardInputState,
    KeyEventState,
    KeyInputEvent,
    MouseInputEvent,
    MouseInputState,
    MouseMode,
    MouseScrollPhase,
    MouseScrollState,
    MouseScrollUnit,
    TouchCanvasPoint,
    TouchContact,
    TouchInputEvent,
    TouchInputState,
    TouchPhase,
} from './types'

/**
 * Get the interactive input snapshot from the Hypercolor runtime.
 * Returns an idle, unavailable snapshot when running outside the daemon.
 */
export function getInputData(): InputData {
    const hasEngine = typeof engine !== 'undefined' && engine !== null
    const raw = hasEngine ? (engine as any) : undefined
    const keyboard = readKeyboard(raw?.keyboard)
    const mouse = readMouse(raw?.mouse)
    const availability = readAvailability(raw?.inputAvailability)

    return {
        ...availability,
        deviceButtons: readDeviceButtons(raw?.deviceButtons),
        dropped: finiteNumber(raw?.inputDropped, 0),
        keyboard,
        mouse,
        touch: readTouch(raw?.touch),
    }
}

function readTouch(raw: any): TouchInputState {
    if (typeof raw !== 'object' || raw === null) {
        return { down: false, events: [], touches: [] }
    }

    const touches: TouchContact[] = []
    if (Array.isArray(raw.touches)) {
        for (const entry of raw.touches as unknown[]) {
            if (typeof entry === 'object' && entry !== null) touches.push(readTouchContact(entry))
        }
    }

    const events: TouchInputEvent[] = []
    if (Array.isArray(raw.events)) {
        for (const entry of raw.events as any[]) {
            if (typeof entry !== 'object' || entry === null || entry.kind !== 'touch') continue
            const event: TouchInputEvent = {
                ...readTouchContact(entry),
                atMs: finiteNumber(entry.atMs, 0),
                kind: 'touch',
                phase: readTouchPhase(entry.phase),
                repeatCount: positiveInteger(entry.repeatCount, 1),
                seq: finiteNumber(entry.seq, 0),
                source: typeof entry.source === 'string' ? entry.source : '',
            }
            if (typeof entry.physicalCode === 'string') event.physicalCode = entry.physicalCode
            events.push(event)
        }
    }

    return { down: touches.length > 0, events, touches }
}

function readTouchContact(raw: any): TouchContact {
    return {
        canvas: readCanvasPoint(raw.canvas),
        contact: Math.max(0, Math.trunc(finiteNumber(raw.contact, 0))),
        device: typeof raw.device === 'string' ? raw.device : '',
        pressure: clamp01(finiteNumber(raw.pressure, 0)),
        x: clamp01(finiteNumber(raw.x, 0)),
        y: clamp01(finiteNumber(raw.y, 0)),
    }
}

function readCanvasPoint(raw: any): TouchCanvasPoint | null {
    if (typeof raw !== 'object' || raw === null) return null
    const { nx, ny, x, y } = raw
    if (![nx, ny, x, y].every((value) => typeof value === 'number' && Number.isFinite(value))) return null
    return { nx, ny, x, y }
}

function readTouchPhase(raw: unknown): TouchPhase {
    return raw === 'ended' || raw === 'cancelled' ? raw : 'began'
}

function readDeviceButtons(raw: any): DeviceButtonInputState {
    if (typeof raw !== 'object' || raw === null) {
        return { buttons: {}, events: [], held: [] }
    }

    const held: DeviceButtonHold[] = []
    if (Array.isArray(raw.held)) {
        for (const entry of raw.held as any[]) {
            if (typeof entry !== 'object' || entry === null || typeof entry.button !== 'string') continue
            held.push({ button: entry.button, device: typeof entry.device === 'string' ? entry.device : '' })
        }
    }

    const events: DeviceButtonInputEvent[] = []
    if (Array.isArray(raw.events)) {
        for (const entry of raw.events as any[]) {
            if (typeof entry !== 'object' || entry === null || entry.kind !== 'device_button') continue
            const event: DeviceButtonInputEvent = {
                atMs: finiteNumber(entry.atMs, 0),
                button: typeof entry.button === 'string' ? entry.button : '',
                device: typeof entry.device === 'string' ? entry.device : '',
                kind: 'device_button',
                repeatCount: positiveInteger(entry.repeatCount, 1),
                seq: finiteNumber(entry.seq, 0),
                source: typeof entry.source === 'string' ? entry.source : '',
                state: readButtonState(entry.state),
            }
            if (typeof entry.physicalCode === 'string') event.physicalCode = entry.physicalCode
            events.push(event)
        }
    }

    return { buttons: heldMap(raw.buttons), events, held }
}

function readButtonState(raw: unknown): KeyEventState {
    return raw === 'released' || raw === 'repeated' ? raw : 'pressed'
}

function readAvailability(raw: any): InputAvailability {
    if (typeof raw !== 'object' || raw === null) {
        return {
            declared: false,
            degraded: false,
            fresh: false,
            healthy: false,
            routed: false,
        }
    }

    return {
        declared: raw.declared === true,
        degraded: raw.degraded === true,
        fresh: raw.fresh === true,
        healthy: raw.healthy === true,
        routed: raw.routed === true,
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
        scroll: readMouseScroll(raw.scroll),
        velocity: finiteNumber(raw.velocity, 0),
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
        scroll: createIdleScroll(),
        velocity: 0,
        x: 0,
        y: 0,
    }
}

function readKeyEvents(raw: unknown): KeyInputEvent[] {
    if (!Array.isArray(raw)) return []

    const events: KeyInputEvent[] = []
    for (const entry of raw as any[]) {
        if (typeof entry !== 'object' || entry === null || entry.kind !== 'key') continue
        const event: KeyInputEvent = {
            atMs: finiteNumber(entry.atMs, 0),
            key: typeof entry.key === 'string' ? entry.key : '',
            kind: 'key',
            repeatCount: positiveInteger(entry.repeatCount, 1),
            seq: finiteNumber(entry.seq, 0),
            source: typeof entry.source === 'string' ? entry.source : '',
            state: entry.state === 'released' || entry.state === 'repeated' ? entry.state : 'pressed',
        }
        if (typeof entry.physicalCode === 'string') event.physicalCode = entry.physicalCode
        events.push(event)
    }
    return events
}

function readMouseEvents(raw: unknown): MouseInputEvent[] {
    if (!Array.isArray(raw)) return []

    const events: MouseInputEvent[] = []
    for (const entry of raw as any[]) {
        if (typeof entry !== 'object' || entry === null) continue
        if (entry.kind === 'button') {
            const event: MouseInputEvent = {
                atMs: finiteNumber(entry.atMs, 0),
                button: typeof entry.button === 'string' ? entry.button : '',
                kind: 'button',
                repeatCount: positiveInteger(entry.repeatCount, 1),
                seq: finiteNumber(entry.seq, 0),
                source: typeof entry.source === 'string' ? entry.source : '',
                state: entry.state === 'released' || entry.state === 'repeated' ? entry.state : 'pressed',
            }
            if (typeof entry.physicalCode === 'string') event.physicalCode = entry.physicalCode
            events.push(event)
        } else if (entry.kind === 'scroll') {
            const event: MouseInputEvent = {
                atMs: finiteNumber(entry.atMs, 0),
                deltaX: finiteNumber(entry.deltaX, 0),
                deltaY: finiteNumber(entry.deltaY, 0),
                kind: 'scroll',
                momentumPhase: readMouseScrollPhase(entry.momentumPhase),
                phase: readMouseScrollPhase(entry.phase),
                repeatCount: positiveInteger(entry.repeatCount, 1),
                seq: finiteNumber(entry.seq, 0),
                source: typeof entry.source === 'string' ? entry.source : '',
                unit: readMouseScrollUnit(entry.unit),
            }
            if (typeof entry.physicalCode === 'string') event.physicalCode = entry.physicalCode
            events.push(event)
        }
    }
    return events
}

function readMouseScroll(raw: any): MouseScrollState {
    if (typeof raw !== 'object' || raw === null) return createIdleScroll()
    return {
        line120X: finiteNumber(raw.line120X, 0),
        line120Y: finiteNumber(raw.line120Y, 0),
        pixelX: finiteNumber(raw.pixelX, 0),
        pixelY: finiteNumber(raw.pixelY, 0),
    }
}

function createIdleScroll(): MouseScrollState {
    return { line120X: 0, line120Y: 0, pixelX: 0, pixelY: 0 }
}

function readMouseScrollUnit(raw: unknown): MouseScrollUnit {
    return raw === 'pixels' ? raw : 'line120'
}

function readMouseScrollPhase(raw: unknown): MouseScrollPhase {
    switch (raw) {
        case 'may_begin':
        case 'began':
        case 'changed':
        case 'stationary':
        case 'ended':
        case 'cancelled':
            return raw
        default:
            return 'none'
    }
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

function positiveInteger(raw: unknown, fallback: number): number {
    const value = Math.trunc(finiteNumber(raw, fallback))
    return value > 0 ? value : fallback
}

function clamp01(value: number): number {
    return Math.max(0, Math.min(1, value))
}
