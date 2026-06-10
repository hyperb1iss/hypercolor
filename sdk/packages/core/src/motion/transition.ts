/**
 * Eased transitions for values that change in steps.
 *
 * Control and preset changes arrive as jumps; a transition retargets an
 * internal tween so the displayed value glides instead. Retargeting mid-
 * flight starts the new glide from the current eased position, so rapid
 * changes never snap.
 */

import { easeOutCubic } from '../math/easing'
import type { EasingFn } from './easing'

export class Transition {
    private readonly duration: number
    private readonly easing: EasingFn
    private from: number
    private to: number
    private startedAt: number
    private initialized = false

    constructor(duration: number, easing: EasingFn = easeOutCubic) {
        this.duration = Math.max(duration, 0)
        this.easing = easing
        this.from = 0
        this.to = 0
        this.startedAt = 0
    }

    /**
     * Feed the latest target and clock; returns the eased display value.
     * The first call adopts the target immediately.
     */
    update(target: number, now: number): number {
        if (!this.initialized) {
            this.initialized = true
            this.from = target
            this.to = target
            this.startedAt = now
            return target
        }

        if (target !== this.to) {
            this.from = this.valueAt(now)
            this.to = target
            this.startedAt = now
        }

        return this.valueAt(now)
    }

    private valueAt(now: number): number {
        if (this.duration === 0) return this.to
        const raw = (now - this.startedAt) / this.duration
        if (raw >= 1) return this.to
        if (raw <= 0) return this.from
        return this.from + (this.to - this.from) * this.easing(raw)
    }
}

/**
 * Create a transition that eases step changes over `duration` seconds.
 */
export function transitionOnChange(duration: number, easing?: EasingFn): Transition {
    return new Transition(duration, easing)
}
