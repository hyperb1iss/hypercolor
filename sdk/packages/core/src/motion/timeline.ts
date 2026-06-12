/**
 * Staggered entrance sequencing.
 *
 * A timeline names a set of segments, each with a start offset and
 * duration; faces query eased per-segment progress against any clock.
 * Typical use: cards that cascade in one after another on face load.
 */

import { easeOutCubic } from '../math/easing'
import type { EasingFn } from './easing'

interface TimelineSegment {
    start: number
    duration: number
    easing: EasingFn
}

export class Timeline {
    private readonly segments = new Map<string, TimelineSegment>()

    /** Add a named segment starting at `start` seconds. Chainable. */
    add(name: string, start: number, duration: number, easing: EasingFn = easeOutCubic): this {
        this.segments.set(name, {
            duration: Math.max(duration, 0),
            easing,
            start: Math.max(start, 0),
        })
        return this
    }

    /**
     * Add a segment that starts when the most recently added segment
     * ends, offset by `gap` seconds. Chainable.
     */
    after(name: string, duration: number, gap = 0, easing?: EasingFn): this {
        let end = 0
        for (const segment of this.segments.values()) {
            end = Math.max(end, segment.start + segment.duration)
        }
        return this.add(name, end + gap, duration, easing)
    }

    /** Eased progress of `name` in [0, 1] at `elapsed` seconds. */
    progress(name: string, elapsed: number): number {
        const segment = this.segments.get(name)
        if (!segment) return 0
        if (segment.duration === 0) return elapsed >= segment.start ? 1 : 0
        const raw = (elapsed - segment.start) / segment.duration
        if (raw <= 0) return 0
        if (raw >= 1) return 1
        return segment.easing(raw)
    }

    /** Whether every segment has completed at `elapsed` seconds. */
    done(elapsed: number): boolean {
        for (const segment of this.segments.values()) {
            if (elapsed < segment.start + segment.duration) return false
        }
        return true
    }

    /** Total duration from zero to the last segment's end. */
    duration(): number {
        let end = 0
        for (const segment of this.segments.values()) {
            end = Math.max(end, segment.start + segment.duration)
        }
        return end
    }
}

/** Create an empty timeline. */
export function timeline(): Timeline {
    return new Timeline()
}
