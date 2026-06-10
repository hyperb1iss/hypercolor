/**
 * Easing functions for face motion.
 *
 * Re-exports the shared cubic/quad easings from `math/easing` and adds the
 * expressive curves faces lean on for entrances and gauge sweeps. All
 * functions map `t` in [0, 1] to a progress value (overshoot curves may
 * briefly leave [0, 1] by design).
 */

export {
    easeInCubic,
    easeInOutCubic,
    easeInOutQuad,
    easeInQuad,
    easeOutCubic,
    easeOutQuad,
} from '../math/easing'

/** A normalized easing curve: progress in [0, 1] → eased progress. */
export type EasingFn = (t: number) => number

/** Identity easing. */
export function linear(t: number): number {
    return t
}

/** Elastic settle — overshoots and rings before landing. */
export function easeOutElastic(t: number): number {
    if (t <= 0) return 0
    if (t >= 1) return 1
    const c4 = (2 * Math.PI) / 3
    return 2 ** (-10 * t) * Math.sin((t * 10 - 0.75) * c4) + 1
}

/** Back-out — overshoots the target once, then returns. */
export function easeOutBack(t: number): number {
    const c1 = 1.70158
    const c3 = c1 + 1
    const f = t - 1
    return 1 + c3 * f * f * f + c1 * f * f
}
