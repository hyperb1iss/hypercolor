/**
 * Control factory functions — explicit API for effect control declarations.
 *
 * Each factory creates a ControlSpec that the effect() / canvas() functions
 * consume to auto-wire uniforms, meta tags, and runtime control reading.
 */

/** Discriminated union tag for control types. */
export type ControlTypeName = 'number' | 'combobox' | 'boolean' | 'color' | 'hue' | 'textfield'

/** Normalization hint applied to control values before use. */
export type NormalizeHint = 'speed' | 'percentage' | 'none'

/** Internal control specification — what factories produce. */
export interface ControlSpec<T extends ControlTypeName = ControlTypeName> {
    readonly __brand: 'ControlSpec'
    readonly __type: T
    readonly label: string
    readonly defaultValue: unknown
    readonly tooltip?: string
    readonly uniform?: string
    readonly normalize?: NormalizeHint
    readonly meta: Readonly<Record<string, unknown>>
}

function spec<T extends ControlTypeName>(
    type: T,
    label: string,
    defaultValue: unknown,
    meta: Record<string, unknown>,
    opts?: { tooltip?: string; uniform?: string; normalize?: NormalizeHint },
): ControlSpec<T> {
    return {
        __brand: 'ControlSpec',
        __type: type,
        label,
        defaultValue,
        tooltip: opts?.tooltip,
        uniform: opts?.uniform,
        normalize: opts?.normalize,
        meta,
    }
}

/** Check if a value is a ControlSpec (produced by a factory function). */
export function isControlSpec(value: unknown): value is ControlSpec {
    return (
        value !== null &&
        typeof value === 'object' &&
        (value as Record<string, unknown>).__brand === 'ControlSpec'
    )
}

// ── Factory Functions ────────────────────────────────────────────────────

export interface NumOptions {
    step?: number
    tooltip?: string
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
    return spec('number', label, defaultValue, {
        min: range[0],
        max: range[1],
        step: opts?.step,
    }, opts)
}

export interface ComboOptions {
    default?: string
    tooltip?: string
    uniform?: string
}

/** Combobox (dropdown) control. */
export function combo(
    label: string,
    values: readonly string[],
    opts?: ComboOptions,
): ControlSpec<'combobox'> {
    const defaultValue = opts?.default ?? values[0]
    return spec('combobox', label, defaultValue, {
        values: [...values],
    }, opts)
}

export interface ToggleOptions {
    tooltip?: string
    uniform?: string
}

/** Boolean toggle control. */
export function toggle(
    label: string,
    defaultValue: boolean,
    opts?: ToggleOptions,
): ControlSpec<'boolean'> {
    return spec('boolean', label, defaultValue, {}, opts)
}

export interface ColorOptions {
    tooltip?: string
    uniform?: string
}

/** Color picker control (hex string). */
export function color(
    label: string,
    defaultValue: string,
    opts?: ColorOptions,
): ControlSpec<'color'> {
    return spec('color', label, defaultValue, {}, opts)
}

export interface HueOptions {
    tooltip?: string
    uniform?: string
}

/** Hue picker control. */
export function hue(
    label: string,
    range: readonly [number, number],
    defaultValue: number,
    opts?: HueOptions,
): ControlSpec<'hue'> {
    return spec('hue', label, defaultValue, {
        min: range[0],
        max: range[1],
    }, opts)
}

export interface TextOptions {
    tooltip?: string
    uniform?: string
}

/** Text field control. */
export function text(
    label: string,
    defaultValue: string,
    opts?: TextOptions,
): ControlSpec<'textfield'> {
    return spec('textfield', label, defaultValue, {}, opts)
}
