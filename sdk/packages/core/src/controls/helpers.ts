/**
 * Runtime control value helpers.
 * Reads control values from the window object (set by Hypercolor runtime).
 */

/** Base controls common to most effects. */
export interface BaseControls {
    speed: number
    colorIntensity: number
    colorSaturation: number
}

/** Read a control value from the window object. */
export function getControlValue<T>(propertyName: string, defaultValue: T): T {
    return (window[propertyName] as T) ?? defaultValue
}

/** Normalize speed from control range (1-10) to multiplier (0.2-3.0). */
export function normalizeSpeed(speed: number): number {
    if (typeof speed !== 'number' || Number.isNaN(speed)) return 1.0
    return Math.max(0.2, (speed / 5) ** 1.5)
}

/** Convert combobox string value to numeric index. */
export function comboboxValueToIndex(value: string | number, options: string[], defaultIndex = 0): number {
    if (typeof value === 'number') return value
    const index = options.indexOf(value)
    return index === -1 ? defaultIndex : index
}

/** Normalize percentage (0-200) to factor (0-2). */
export function normalizePercentage(value: number, defaultValue = 100, minValue = 0.01): number {
    const rawValue = typeof value === 'number' && !Number.isNaN(value) ? value : defaultValue
    return Math.max(minValue, rawValue / 100)
}

/** Convert boolean to 0 or 1. */
export function boolToInt(value: boolean | number): number {
    if (typeof value === 'number') return value === 0 ? 0 : 1
    return value ? 1 : 0
}

/** Fetch all control values from the window object. */
export function getAllControls<T extends Record<string, unknown>>(controls: T): T {
    const result: Record<string, unknown> = {}
    for (const [key, defaultValue] of Object.entries(controls)) {
        result[key] = getControlValue(key, defaultValue)
    }
    return result as T
}
