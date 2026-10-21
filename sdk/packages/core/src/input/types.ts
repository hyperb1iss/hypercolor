/**
 * Interactive input data types.
 *
 * The daemon captures keyboard/mouse input and touches and buttons on
 * driver-owned devices, aggregates them per frame, and injects state plus
 * ordered, capture-timestamped events into `engine.keyboard`,
 * `engine.mouse`, `engine.touch`, and `engine.deviceButtons` when the effect
 * declares `input: true`.
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

/** Mouse event ordered by `seq` and stamped with monotonic capture time. */
export type MouseInputEvent = MouseButtonInputEvent | MouseScrollInputEvent

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
    /** Exact two-axis scroll accumulated independently by coordinate unit. */
    scroll: MouseScrollState
    /** Normalized pointer motion magnitude per second. */
    velocity: number
    /** Ordered button and exact scroll events captured this frame. */
    events: MouseInputEvent[]
}

/**
 * Where a device-surface point lands on this effect's canvas.
 *
 * Resolved through the spatial layout the canvas is sampled with: the point
 * is the canvas position that the LED under the finger samples. It is in the
 * canvas the zone samples, so a scene layer drawn with a non-default
 * transform sees it offset.
 */
export interface TouchCanvasPoint {
    /** Canvas 2D x of the sampled pixel's center, `nx * (width - 1) + 0.5`. */
    x: number
    /** Canvas 2D y of the sampled pixel's center, `ny * (height - 1) + 0.5`. */
    y: number
    /** Normalized canvas x, usually in [0, 1]. */
    nx: number
    /** Normalized canvas y, usually in [0, 1]. */
    ny: number
}

/**
 * One contact held on a device's touch surface, such as a ROLI Lightpad or
 * an Ableton Push 2 pad.
 */
export interface TouchContact {
    /** Hypercolor device id of the touched surface. */
    device: string
    /** Contact identity, stable from touch start to end on that device. */
    contact: number
    /**
     * Device-surface x in [0, 1], in the frame the touched segment's LED
     * order defines: a matrix's first LED is its top left, and a strip runs
     * along x from its first LED.
     */
    x: number
    /**
     * Device-surface y in [0, 1], from the row holding the segment's first
     * LED. That row need not be the physical top: on a Push 2 the first pad
     * row, y = 0, is the bottom one. Draw at `canvas` to land where the
     * finger is.
     */
    y: number
    /** Contact pressure in [0, 1]. */
    pressure: number
    /**
     * Canvas position of the contact, or `null` when the touched surface has
     * no single output in this canvas's layout. A device with several
     * segments, such as Push 2 pads and its touch strip, is placed per
     * segment, so a touch is `null` only when its own segment is not placed,
     * or is placed more than once. Surface coordinates stay valid either way.
     */
    canvas: TouchCanvasPoint | null
}

/** Lifecycle edge of a contact. `cancelled` is a lost hold, not a lift. */
export type TouchPhase = 'began' | 'ended' | 'cancelled'

/**
 * A contact landing on or leaving a device surface, ordered by `seq`.
 * Movement between the two is held state in `touches`, not an event.
 */
export interface TouchInputEvent extends TouchContact {
    kind: 'touch'
    /** Input source that produced the event. */
    source: string
    phase: TouchPhase
    /** Capture timestamp in monotonic milliseconds. */
    atMs: number
    /** Strictly increasing sequence number. */
    seq: number
    /** Backend-neutral physical control code when the source provides one. */
    physicalCode?: string
    /** Number of equivalent ordered events represented by this entry. */
    repeatCount: number
}

/** One control button held on a device. */
export interface DeviceButtonHold {
    /** Hypercolor device id that owns the button. */
    device: string
    /** Button name as the driver reports it, such as `mode`. */
    button: string
}

/** A device control button changing state, ordered by `seq`. */
export interface DeviceButtonInputEvent extends DeviceButtonHold {
    kind: 'device_button'
    /** Input source that produced the event. */
    source: string
    state: KeyEventState
    /** Capture timestamp in monotonic milliseconds. */
    atMs: number
    /** Strictly increasing sequence number. */
    seq: number
    /** Backend-neutral physical control code when the source provides one. */
    physicalCode?: string
    /** Number of equivalent ordered events represented by this entry. */
    repeatCount: number
}

/** Touch snapshot for the current frame, across every touch-capable device. */
export interface TouchInputState {
    /** Contacts currently held, with live positions. */
    touches: TouchContact[]
    /** True while any contact is held. */
    down: boolean
    /** Ordered contact begin and end events captured this frame. */
    events: TouchInputEvent[]
}

/** Device control-button snapshot for the current frame. */
export interface DeviceButtonInputState {
    /** Held button names, from any device. */
    buttons: Record<string, boolean>
    /** Held buttons with the device that holds each. */
    held: DeviceButtonHold[]
    /** Ordered button events captured this frame. */
    events: DeviceButtonInputEvent[]
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
}

/** Typed per-frame input snapshot returned by `getInputData()`. */
export interface InputData extends InputAvailability {
    keyboard: KeyboardInputState
    mouse: MouseInputState
    /** Touches on driver-owned touch surfaces. */
    touch: TouchInputState
    /** Control buttons on driver-owned devices. */
    deviceButtons: DeviceButtonInputState
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

/**
 * Touch contract injected by the daemon at `engine.touch`, including the
 * helper function pre-installed by the LightScript runtime.
 */
export interface EngineTouch extends TouchInputState {
    /** True while `device` has a contact down; with no argument, true while any contact is down. */
    isDown(device?: string): boolean
}

/**
 * Device-button contract injected by the daemon at `engine.deviceButtons`,
 * including the helper function pre-installed by the LightScript runtime.
 */
export interface EngineDeviceButtons extends DeviceButtonInputState {
    /**
     * True while `button` is held, on `device` when given; with no button,
     * true while any button is held (on `device` when given).
     */
    isDown(button?: string, device?: string): boolean
}
