/**
 * Persistent gauge handles with eased motion.
 *
 * The one-shot gauge functions stay for static draws; these factories own
 * a Transition per gauge so every face gets appear sweeps and eased value
 * changes for free. Pass the face clock (seconds) to `draw` — motion is
 * clock-based and frame-rate independent.
 */

import type { EasingFn } from '../motion'
import { Transition } from '../motion'
import type { ArcGaugeOptions } from './arc'
import { arcGauge } from './arc'
import type { BarGaugeOptions } from './bar'
import { barGauge } from './bar'
import type { RingGaugeOptions } from './ring'
import { ringGauge } from './ring'

export interface GaugeAnimateOptions {
    /** Seconds per value glide and for the appear sweep (default: 0.6). */
    duration?: number
    easing?: EasingFn
}

const DEFAULT_GAUGE_DURATION = 0.6

/** Eases gauge values: sweeps from zero on first draw, glides on change. */
class AnimatedGaugeValue {
    private readonly transition: Transition
    private primed = false
    private current = 0

    constructor(animate: GaugeAnimateOptions = {}) {
        this.transition = new Transition(animate.duration ?? DEFAULT_GAUGE_DURATION, animate.easing)
    }

    resolve(target: number, time: number): number {
        if (!this.primed) {
            this.primed = true
            this.transition.update(0, time)
        }
        this.current = this.transition.update(target, time)
        return this.current
    }

    /** Latest eased value (for tests and value readouts). */
    value(): number {
        return this.current
    }
}

export interface AnimatedArcGauge {
    draw(ctx: CanvasRenderingContext2D, value: number, time: number): void
    value(): number
}

/** Arc gauge handle owning its eased value state. */
export function createArcGauge(base: Omit<ArcGaugeOptions, 'value'>, animate?: GaugeAnimateOptions): AnimatedArcGauge {
    const eased = new AnimatedGaugeValue(animate)
    return {
        draw(ctx, value, time) {
            arcGauge(ctx, { ...base, value: eased.resolve(value, time) })
        },
        value: () => eased.value(),
    }
}

export interface AnimatedBarGauge {
    draw(
        ctx: CanvasRenderingContext2D,
        value: number,
        time: number,
        overrides?: Partial<Omit<BarGaugeOptions, 'value'>>,
    ): void
    value(): number
}

/** Bar gauge handle owning its eased value state. Per-frame overrides
 *  cover layout-driven position changes without losing motion state. */
export function createBarGauge(base: Omit<BarGaugeOptions, 'value'>, animate?: GaugeAnimateOptions): AnimatedBarGauge {
    const eased = new AnimatedGaugeValue(animate)
    return {
        draw(ctx, value, time, overrides) {
            barGauge(ctx, { ...base, ...overrides, value: eased.resolve(value, time) })
        },
        value: () => eased.value(),
    }
}

export interface AnimatedRingGauge {
    draw(
        ctx: CanvasRenderingContext2D,
        value: number,
        time: number,
        overrides?: Partial<Omit<RingGaugeOptions, 'value'>>,
    ): void
    value(): number
}

/** Ring gauge handle owning its eased value state. */
export function createRingGauge(
    base: Omit<RingGaugeOptions, 'value'>,
    animate?: GaugeAnimateOptions,
): AnimatedRingGauge {
    const eased = new AnimatedGaugeValue(animate)
    return {
        draw(ctx, value, time, overrides) {
            ringGauge(ctx, { ...base, ...overrides, value: eased.resolve(value, time) })
        },
        value: () => eased.value(),
    }
}
