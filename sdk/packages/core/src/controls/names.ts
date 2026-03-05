/**
 * Name derivation and magic name detection for effect controls.
 */

import type { ControlSpec, NormalizeHint } from './specs'

/**
 * Derive a human-readable label from a camelCase key.
 *
 *   speed        → "Speed"
 *   trailLength  → "Trail Length"
 *   edgeGlow     → "Edge Glow"
 *   gridDensity  → "Grid Density"
 */
export function deriveLabel(key: string): string {
    return key
        .replace(/([A-Z])/g, ' $1')      // insert space before uppercase
        .replace(/^./, (c) => c.toUpperCase())  // capitalize first letter
        .trim()
}

/**
 * Derive a GLSL uniform name from a control key.
 *
 *   speed        → iSpeed
 *   trailLength  → iTrailLength
 *   palette      → iPalette
 */
export function deriveUniformName(key: string): string {
    return 'i' + key.charAt(0).toUpperCase() + key.slice(1)
}

/** Magic control names that trigger automatic normalization. */
const MAGIC_NAMES: Record<string, NormalizeHint> = {
    speed: 'speed',
}

/** Magic control names that trigger special value transforms (e.g. combobox → index). */
const MAGIC_TRANSFORMS = new Set(['palette'])

/**
 * Get the automatic normalization hint for a key, if any.
 * Returns 'none' if no magic normalization applies.
 */
export function getMagicNormalize(key: string): NormalizeHint {
    return MAGIC_NAMES[key] ?? 'none'
}

/** Check if a key has a magic transform (e.g. 'palette' → comboboxValueToIndex). */
export function hasMagicTransform(key: string): boolean {
    return MAGIC_TRANSFORMS.has(key)
}

/**
 * Resolve all naming and magic behavior for a control.
 * Returns the final label, uniform name, and normalization hint.
 */
export function resolveControlNames(key: string, spec: ControlSpec): {
    label: string
    uniformName: string
    normalize: NormalizeHint
} {
    return {
        label: spec.label,
        uniformName: spec.uniform ?? deriveUniformName(key),
        normalize: spec.normalize ?? getMagicNormalize(key),
    }
}
