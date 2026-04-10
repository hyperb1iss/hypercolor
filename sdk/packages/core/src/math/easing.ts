/**
 * Time-based smoothing and easing primitives.
 *
 * `smoothApproach` and `smoothAsymmetric` are frame-rate-independent
 * exponential smoothers — the de facto state lowpass filter used across
 * the Hypercolor TypeScript effects.
 */

/**
 * Exponentially approach `target` from `current`.
 *
 * `lambda` is the inverse time constant: higher values converge faster.
 * Frame-rate independent via `1 - exp(-lambda * dt)`.
 */
export function smoothApproach(current: number, target: number, lambda: number, dt: number): number {
    if (!Number.isFinite(lambda) || lambda <= 0) return target
    const factor = 1 - Math.exp(-lambda * Math.max(dt, 0))
    return current + (target - current) * factor
}

/**
 * Exponential smoothing with separate attack and decay rates.
 *
 * Rising values use `attackLambda`, falling values use `decayLambda`.
 * Useful for envelope followers that should snap up but fall slowly
 * (or vice versa).
 */
export function smoothAsymmetric(
    current: number,
    target: number,
    attackLambda: number,
    decayLambda: number,
    dt: number,
): number {
    const lambda = target > current ? attackLambda : decayLambda
    return smoothApproach(current, target, lambda, dt)
}

/** Quadratic ease-in. */
export function easeInQuad(t: number): number {
    return t * t
}

/** Quadratic ease-out. */
export function easeOutQuad(t: number): number {
    return t * (2 - t)
}

/** Quadratic ease-in-out. */
export function easeInOutQuad(t: number): number {
    return t < 0.5 ? 2 * t * t : -1 + (4 - 2 * t) * t
}

/** Cubic ease-in. */
export function easeInCubic(t: number): number {
    return t * t * t
}

/** Cubic ease-out. */
export function easeOutCubic(t: number): number {
    const f = t - 1
    return f * f * f + 1
}

/** Cubic ease-in-out. */
export function easeInOutCubic(t: number): number {
    if (t < 0.5) return 4 * t * t * t
    const f = 2 * t - 2
    return 0.5 * f * f * f + 1
}
