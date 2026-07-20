/**
 * Interactive input data types.
 *
 * The daemon captures keyboard/mouse input, aggregates it per frame, and
 * injects state plus ordered, capture-timestamped events into
 * `engine.keyboard` / `engine.mouse` when the effect declares `input: true`.
 */

/** Coordinate availability mode for the pointer. */
export type MouseMode = 'none' | 'absolute' | 'virtual'

/** Lifecycle state carried by a key event. */
export type KeyEventState = 'pressed' | 'released' | 'repeated'

/**
 * A single keyboard event, ordered by `seq` and stamped with the capture
 * timestamp (`atMs`, monotonic milliseconds).
 */
export interface KeyInputEvent {
    kind: 'key'
    /** Identifier of the device that produced the event. */
    source: string
    /** Key name (alias forms like "A" and "KeyA" both appear in state maps). */
    key: string
    state: KeyEventState
    /** Capture timestamp in monotonic milliseconds. */
    atMs: number
    /** Strictly increasing sequence number. */
    seq: number
}

/**
 * A single mouse button or wheel event, ordered by `seq` and stamped with
 * the capture timestamp (`atMs`, monotonic milliseconds).
 */
export interface MouseInputEvent {
    kind: 'button' | 'wheel'
    /** Identifier of the device that produced the event. */
    source: string
    /** Button name (present for `kind: 'button'`). */
    button?: string
    /** Button lifecycle (present for `kind: 'button'`). */
    state?: 'pressed' | 'released'
    /** Wheel delta in notches (present for `kind: 'wheel'`). */
    delta?: number
    /** Capture timestamp in monotonic milliseconds. */
    atMs: number
    /** Strictly increasing sequence number. */
    seq: number
}

/** Keyboard snapshot for the current frame. */
export interface KeyboardInputState {
    /** Currently held keys (includes alias forms like "A" and "KeyA"). */
    keys: Record<string, boolean>
    /** Keys newly pressed since the last frame. */
    recent: string[]
    /** Ordered key events captured since the last frame. */
    events: KeyInputEvent[]
}

/** Mouse snapshot for the current frame. */
export interface MouseInputState {
    /** Pointer x in platform pixels (0 when unavailable). */
    x: number
    /** Pointer y in platform pixels (0 when unavailable). */
    y: number
    /** Normalized pointer x in [0, 1]. */
    nx: number
    /** Normalized pointer y in [0, 1]. */
    ny: number
    /** True while any button is held. */
    down: boolean
    /** Currently held buttons keyed by button name. */
    buttons: Record<string, boolean>
    /** Coordinate availability mode. */
    mode: MouseMode
    /** True when pointer coordinates are meaningful (`mode !== 'none'`). */
    available: boolean
    /** Accumulated wheel notches this frame (hi-res deltas divided by 120). */
    wheel: number
    /** Normalized pointer motion magnitude per second. */
    velocity: number
    /** Ordered button/wheel events captured since the last frame. */
    events: MouseInputEvent[]
}

/** Typed per-frame input snapshot returned by `getInputData()`. */
export interface InputData {
    /** True when the mouse is available or any keyboard activity is present. */
    available: boolean
    keyboard: KeyboardInputState
    mouse: MouseInputState
    /** Count of input events dropped this frame due to overflow. */
    dropped: number
}

/**
 * Keyboard contract injected by the daemon at `engine.keyboard`, including
 * the helper functions pre-installed by the LightScript runtime.
 */
export interface EngineKeyboard extends KeyboardInputState {
    /** True while `key` (or its lowercase alias) is held. */
    isKeyDown(key: string): boolean
    /** True when `key` was newly pressed since the last frame. */
    wasKeyPressed(key: string): boolean
    /** Returns and clears the newly pressed keys. */
    consumePressedKeys(): string[]
}

/**
 * Mouse contract injected by the daemon at `engine.mouse`, including the
 * helper function pre-installed by the LightScript runtime.
 */
export interface EngineMouse extends MouseInputState {
    /** True while `button` is held; with no argument, true while any button is held. */
    isDown(button?: string | number): boolean
}
