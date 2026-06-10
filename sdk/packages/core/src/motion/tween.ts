/**
 * Time-based value tracks.
 *
 * A tween maps elapsed seconds to an eased value between two endpoints.
 * Tweens are pure with respect to time — `.at(t)` can be called with any
 * clock, which keeps them correct at 15, 30, or 60fps.
 */

import { easeOutCubic } from '../math/easing'
import type { EasingFn } from './easing'

export class Tween {
    private readonly from: number
    private readonly to: number
    private readonly duration: number
    private readonly easing: EasingFn

    constructor(from: number, to: number, duration: number, easing: EasingFn = easeOutCubic) {
        this.from = from
        this.to = to
        this.duration = Math.max(duration, 0)
        this.easing = easing
    }

    /** Value at `elapsed` seconds since the tween started. */
    at(elapsed: number): number {
        if (this.duration === 0 || elapsed >= this.duration) return this.to
        if (elapsed <= 0) return this.from
        const progress = this.easing(elapsed / this.duration)
        return this.from + (this.to - this.from) * progress
    }

    /** Whether the tween has reached its end value at `elapsed` seconds. */
    done(elapsed: number): boolean {
        return elapsed >= this.duration
    }
}

/** Create a time-based track from `from` to `to` over `duration` seconds. */
export function tween(from: number, to: number, duration: number, easing?: EasingFn): Tween {
    return new Tween(from, to, duration, easing)
}
