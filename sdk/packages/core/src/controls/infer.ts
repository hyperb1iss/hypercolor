/**
 * Shape-based control type inference.
 *
 * Determines control type from the value shape in shorthand declarations:
 *   [1, 10, 5]              → number slider
 *   ['Fire', 'Ice']         → combobox
 *   false                   → toggle
 *   '#ff6ac1'               → color
 *   'hello'                 → text field
 *   42                      → number (range 0-100)
 */

import type { ControlSpec, ControlTypeName } from './specs'

/** All valid shorthand value shapes. */
export type ControlShorthand =
    | readonly [number, number, number]                 // slider [min, max, default]
    | readonly [number, number, number, number]         // slider with step [min, max, default, step]
    | readonly string[]                                 // combobox
    | boolean                                           // toggle
    | number                                            // simple slider (0-100, value = default)
    | string                                            // color (#hex) or text (non-hex)

/** A control map value is either a shorthand or an explicit ControlSpec. */
export type ControlMapValue = ControlShorthand | ControlSpec

/** The controls object passed to effect() / canvas(). */
export type ControlMap = Record<string, ControlMapValue>

/** Infer a ControlSpec from a shorthand value. */
export function inferControl(key: string, value: ControlShorthand, label: string): ControlSpec {
    // Number tuple: [min, max, default] or [min, max, default, step]
    if (Array.isArray(value) && value.length >= 3 && value.length <= 4 && typeof value[0] === 'number') {
        const [min, max, defaultValue, step] = value as [number, number, number, number?]
        return {
            __brand: 'ControlSpec',
            __type: 'number' as ControlTypeName,
            label,
            defaultValue,
            meta: { min, max, step },
        }
    }

    // String array: combobox
    if (Array.isArray(value) && value.length >= 1 && typeof value[0] === 'string') {
        return {
            __brand: 'ControlSpec',
            __type: 'combobox' as ControlTypeName,
            label,
            defaultValue: value[0],
            meta: { values: [...value] },
        }
    }

    // Boolean: toggle
    if (typeof value === 'boolean') {
        return {
            __brand: 'ControlSpec',
            __type: 'boolean' as ControlTypeName,
            label,
            defaultValue: value,
            meta: {},
        }
    }

    // String starting with #: color
    if (typeof value === 'string' && value.startsWith('#')) {
        return {
            __brand: 'ControlSpec',
            __type: 'color' as ControlTypeName,
            label,
            defaultValue: value,
            meta: {},
        }
    }

    // Plain string: text field
    if (typeof value === 'string') {
        return {
            __brand: 'ControlSpec',
            __type: 'textfield' as ControlTypeName,
            label,
            defaultValue: value,
            meta: {},
        }
    }

    // Plain number: slider 0-100
    if (typeof value === 'number') {
        return {
            __brand: 'ControlSpec',
            __type: 'number' as ControlTypeName,
            label,
            defaultValue: value,
            meta: { min: 0, max: 100 },
        }
    }

    throw new Error(
        `Cannot infer control type for "${key}". ` +
        `Expected [min, max, default], string[], boolean, '#hex', string, or number. ` +
        `Got: ${JSON.stringify(value)}`
    )
}
