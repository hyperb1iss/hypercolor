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
    /** Backend-neutral physical key code when the source provides one. */
    physicalCode?: string
    /** Number of equivalent ordered events represented by this entry. */
    repeatCount: number
}

/** Coordinate unit carried by an exact scroll event. */
export type MouseScrollUnit = 'line120' | 'pixels'

/** Lifecycle phase carried by an exact scroll event. */
export type MouseScrollPhase = 'none' | 'may_begin' | 'began' | 'changed' | 'stationary' | 'ended' | 'cancelled'

interface MouseInputEventBase {
    /** Identifier of the device that produced the event. */
    source: string
    /** Capture timestamp in monotonic milliseconds. */
    atMs: number
    /** Strictly increasing sequence number. */
    seq: number
    /** Backend-neutral physical control code when the source provides one. */
    physicalCode?: string
    /** Number of equivalent ordered events represented by this entry. */
    repeatCount: number
}

/** One ordered mouse-button lifecycle event. */
export interface MouseButtonInputEvent extends MouseInputEventBase {
    kind: 'button'
    button: string
    state: KeyEventState
}

/** One ordered exact two-axis scroll event. */
export interface MouseScrollInputEvent extends MouseInputEventBase {
    kind: 'scroll'
    deltaX: number
    deltaY: number
    unit: MouseScrollUnit
    phase: MouseScrollPhase
    momentumPhase: MouseScrollPhase
}

/**
 * One ordered legacy vertical wheel event.
 *
 * @deprecated Consume the adjacent `scroll` event instead. This member remains
 * available through the next API major.
 */
export interface MouseWheelInputEvent extends MouseInputEventBase {
    kind: 'wheel'
    delta: number
}

/** Mouse event ordered by `seq` and stamped with monotonic capture time. */
export type MouseInputEvent = MouseButtonInputEvent | MouseScrollInputEvent | MouseWheelInputEvent

/** Exact two-axis scroll totals for the current frame. */
export interface MouseScrollState {
    line120X: number
    line120Y: number
    pixelX: number
    pixelY: number
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
    /** Exact two-axis scroll accumulated independently by coordinate unit. */
    scroll: MouseScrollState
    /** Normalized pointer motion magnitude per second. */
    velocity: number
    /** Ordered button, scroll, and compatibility wheel events captured this frame. */
    events: MouseInputEvent[]
}

/** Input declaration, routing, and source lifecycle state. */
export interface InputAvailability {
    /** Whether the effect declares interactive input capability. */
    declared: boolean
    /** Whether an input source is routed to this effect. */
    routed: boolean
    /** Whether the routed source is operational. */
    healthy: boolean
    /** Whether the routed source data is inside its freshness contract. */
    fresh: boolean
    /** Whether the routed source is operating with reduced capability. */
    degraded: boolean
    /**
     * Whether input is routed and healthy.
     *
     * @deprecated Read `routed && healthy` instead. This alias will be removed
     * in version 0.4.0.
     */
    available: boolean
}

/** Typed per-frame input snapshot returned by `getInputData()`. */
export interface InputData extends InputAvailability {
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
