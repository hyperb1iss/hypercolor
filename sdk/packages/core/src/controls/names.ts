/**
 * Name derivation and normalization hints for effect controls.
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
        .replace(/([A-Z])/g, ' $1') // insert space before uppercase
        .replace(/^./, (c) => c.toUpperCase()) // capitalize first letter
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
    return `i${key.charAt(0).toUpperCase()}${key.slice(1)}`
}

/** Magic control names that trigger automatic normalization. */
const MAGIC_NAMES: Record<string, NormalizeHint> = {
    speed: 'speed',
}

/**
 * Get the automatic normalization hint for a key, if any.
 * Returns 'none' if no magic normalization applies.
 */
export function getMagicNormalize(key: string): NormalizeHint {
    return MAGIC_NAMES[key] ?? 'none'
}

const warnedSpeedRanges = new Set<string>()

/**
 * normalizeSpeed() is calibrated for a raw 1-10 slider: max(0.2, (v/5)^1.5).
 * A control that picks up the magic `speed` normalization with any other
 * declared range is almost certainly a bug — the whole [0,1] span clamps to a
 * constant 0.2 and negative values become NaN. Warn loudly at build/metadata
 * time so it can't ship silently. Opt out with normalize: 'none'.
 */
function warnIfSpeedRangeMismatched(key: string, spec: ControlSpec, normalize: NormalizeHint): void {
    if (normalize !== 'speed' || spec.normalize === 'speed') return
    const min = spec.meta.min
    const max = spec.meta.max
    if (typeof min !== 'number' || typeof max !== 'number') return
    if (min === 1 && max === 10) return
    const dedupeKey = `${key}:${min}:${max}`
    if (warnedSpeedRanges.has(dedupeKey)) return
    warnedSpeedRanges.add(dedupeKey)
    console.warn(
        `[hypercolor] Control "${key}" declares range [${min}, ${max}] but its name triggers the magic speed ` +
            'normalization calibrated for [1, 10] — values are transformed by max(0.2, (v/5)^1.5) before your ' +
            `code sees them (negative inputs become NaN). Pass normalize: 'none' or declare the range as [1, 10].`,
    )
}

/**
 * Resolve all naming and normalization behavior for a control.
 * Returns the final label, uniform name, and normalization hint.
 */
export function resolveControlNames(
    key: string,
    spec: ControlSpec,
): {
    label: string
    uniformName: string
    normalize: NormalizeHint
} {
    const normalize = spec.normalize ?? getMagicNormalize(key)
    warnIfSpeedRangeMismatched(key, spec, normalize)
    return {
        label: spec.label,
        normalize,
        uniformName: spec.uniform ?? deriveUniformName(key),
    }
}
