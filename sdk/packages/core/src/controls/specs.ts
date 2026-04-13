/**
 * Control factory functions — explicit API for effect control declarations.
 *
 * Each factory creates a ControlSpec that the effect() / canvas() functions
 * consume to auto-wire uniforms, meta tags, and runtime control reading.
 */

/** Discriminated union tag for control types. */
export type ControlTypeName = 'number' | 'combobox' | 'boolean' | 'color' | 'hue' | 'textfield' | 'sensor' | 'rect'

export interface RectValue {
    x: number
    y: number
    width: number
    height: number
}

/** Normalization hint applied to control values before use. */
export type NormalizeHint = 'speed' | 'percentage' | 'none'

/** Internal control specification — what factories produce. */
export interface ControlSpec<T extends ControlTypeName = ControlTypeName> {
    readonly __brand: 'ControlSpec'
    readonly __type: T
    readonly label: string
    readonly defaultValue: unknown
    readonly tooltip?: string
    readonly group?: string
    readonly uniform?: string
    readonly normalize?: NormalizeHint
    readonly meta: Readonly<Record<string, unknown>>
}

function spec<T extends ControlTypeName>(
    type: T,
    label: string,
    defaultValue: unknown,
    meta: Record<string, unknown>,
    opts?: { tooltip?: string; group?: string; uniform?: string; normalize?: NormalizeHint },
): ControlSpec<T> {
    return {
        __brand: 'ControlSpec',
        __type: type,
        defaultValue,
        group: opts?.group,
        label,
        meta,
        normalize: opts?.normalize,
        tooltip: opts?.tooltip,
        uniform: opts?.uniform,
    }
}

/** Check if a value is a ControlSpec (produced by a factory function). */
export function isControlSpec(value: unknown): value is ControlSpec {
    return value !== null && typeof value === 'object' && (value as Record<string, unknown>).__brand === 'ControlSpec'
}

// ── Factory Functions ────────────────────────────────────────────────────

export interface NumOptions {
    step?: number
    tooltip?: string
    group?: string
    normalize?: NormalizeHint
    uniform?: string
}

/** Number slider control. */
export function num(
    label: string,
    range: readonly [number, number],
    defaultValue: number,
    opts?: NumOptions,
): ControlSpec<'number'> {
    return spec(
        'number',
        label,
        defaultValue,
        {
            max: range[1],
            min: range[0],
            step: opts?.step,
        },
        opts,
    )
}

export interface ComboOptions {
    default?: string
    tooltip?: string
    group?: string
    uniform?: string
}

/** Combobox (dropdown) control. */
export function combo(label: string, values: readonly string[], opts?: ComboOptions): ControlSpec<'combobox'> {
    const defaultValue = opts?.default ?? values[0]
    return spec(
        'combobox',
        label,
        defaultValue,
        {
            values: [...values],
        },
        opts,
    )
}

export interface ToggleOptions {
    tooltip?: string
    group?: string
    uniform?: string
}

/** Boolean toggle control. */
export function toggle(label: string, defaultValue: boolean, opts?: ToggleOptions): ControlSpec<'boolean'> {
    return spec('boolean', label, defaultValue, {}, opts)
}

export interface ColorOptions {
    tooltip?: string
    group?: string
    uniform?: string
}

/** Color picker control (hex string). */
export function color(label: string, defaultValue: string, opts?: ColorOptions): ControlSpec<'color'> {
    return spec('color', label, defaultValue, {}, opts)
}

export interface HueOptions {
    tooltip?: string
    group?: string
    uniform?: string
}

/** Hue picker control. */
export function hue(
    label: string,
    range: readonly [number, number],
    defaultValue: number,
    opts?: HueOptions,
): ControlSpec<'hue'> {
    return spec(
        'hue',
        label,
        defaultValue,
        {
            max: range[1],
            min: range[0],
        },
        opts,
    )
}

export interface TextOptions {
    tooltip?: string
    group?: string
    uniform?: string
}

/** Text field control. */
export function text(label: string, defaultValue: string, opts?: TextOptions): ControlSpec<'textfield'> {
    return spec('textfield', label, defaultValue, {}, opts)
}

export interface SensorOptions {
    tooltip?: string
    group?: string
}

/** Sensor picker — user selects from available system sensors.
 *
 *  The runtime value is a sensor label string (e.g., "cpu_temp", "gpu_load").
 *  Pass it to `engine.getSensorValue(label)` to get the live reading.
 *
 *  @example
 *  ```typescript
 *  import { face, sensor } from '@hypercolor/sdk'
 *  export default face('Temp', {
 *      target: sensor('Sensor', 'cpu_temp'),
 *  }, ...)
 *  ```
 */
export function sensor(label: string, defaultValue: string, opts?: SensorOptions): ControlSpec<'sensor'> {
    return spec('sensor', label, defaultValue, {}, opts)
}

export interface RectOptions {
    tooltip?: string
    group?: string
    aspectLock?: number
    preview?: 'screen' | 'web' | 'canvas'
}

/** Interactive viewport rectangle control. */
export function rect(label: string, defaultValue: RectValue, opts?: RectOptions): ControlSpec<'rect'> {
    return spec(
        'rect',
        label,
        defaultValue,
        {
            aspectLock: opts?.aspectLock,
            preview: opts?.preview,
        },
        opts,
    )
}

export interface FontOptions {
    tooltip?: string
    group?: string
    /** Available font families. Defaults to a curated set if omitted. */
    families?: string[]
}

const DEFAULT_FONT_FAMILIES = [
    'JetBrains Mono',
    'Inter',
    'Orbitron',
    'Roboto Condensed',
    'Space Grotesk',
]

/** Font family picker — combobox with font family names.
 *
 *  Syntactic sugar over `combo()` — produces a combobox control whose values
 *  are font family names. The face runtime loads the selected font before
 *  first render.
 */
export function font(label: string, defaultFamily: string, opts?: FontOptions): ControlSpec<'combobox'> {
    const families = opts?.families ?? DEFAULT_FONT_FAMILIES
    // Auto-prepend the default family if it's not already in the list
    const values = families.includes(defaultFamily) ? [...families] : [defaultFamily, ...families]
    return spec('combobox', label, defaultFamily, { values }, opts)
}
