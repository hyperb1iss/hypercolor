/**
 * Interpolation and range primitives.
 *
 * These mirror the common GLSL shader intrinsics so TypeScript effect
 * helpers read the same way their fragment counterparts do.
 */

/**
 * Clamp a value between min and max.
 *
 * Matches GLSL semantics: `NaN` passes through. Callers that need to
 * coerce `NaN` to a safe value should guard at the call site.
 */
export function clamp(value: number, min: number, max: number): number {
    return Math.max(min, Math.min(max, value))
}

/** Clamp a value to the [0, 1] range. */
export function saturate(value: number): number {
    return clamp(value, 0, 1)
}

/** Linear interpolation between `a` and `b` by factor `t`. */
export function mix(a: number, b: number, t: number): number {
    return a + (b - a) * t
}

/** Alias for {@link mix}. */
export const lerp = mix

/** Inverse lerp — returns the normalized position of `value` between `a` and `b`. */
export function inverseLerp(a: number, b: number, value: number): number {
    const range = b - a
    if (range === 0) return 0
    return (value - a) / range
}

/** GLSL-style step: `0` when `value < edge`, `1` otherwise. */
export function step(edge: number, value: number): number {
    return value < edge ? 0 : 1
}

/** GLSL-style smoothstep: hermite interpolation between two edges. */
export function smoothstep(edge0: number, edge1: number, value: number): number {
    const range = edge1 - edge0
    if (range === 0) return value < edge0 ? 0 : 1
    const t = saturate((value - edge0) / range)
    return t * t * (3 - 2 * t)
}
