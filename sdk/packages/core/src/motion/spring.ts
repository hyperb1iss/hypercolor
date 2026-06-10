/**
 * Damped spring for organic gauge motion.
 *
 * Semi-implicit Euler integration with fixed substeps keeps the spring
 * stable across the face frame-rate range (15–60fps) even with stiff
 * parameters.
 */

const MAX_SUBSTEP = 1 / 120

export interface SpringOptions {
    /** Spring constant — higher snaps harder toward the target. */
    stiffness?: number
    /** Velocity damping — higher settles faster with less ring. */
    damping?: number
}

export class Spring {
    /** Current position. */
    value: number
    /** Current velocity in units per second. */
    velocity = 0
    private readonly stiffness: number
    private readonly damping: number

    constructor(initial: number, options: SpringOptions = {}) {
        this.value = initial
        this.stiffness = options.stiffness ?? 170
        this.damping = options.damping ?? 26
    }

    /** Advance toward `target` by `dt` seconds and return the new value. */
    update(target: number, dt: number): number {
        let remaining = Math.max(dt, 0)
        while (remaining > 0) {
            const step = Math.min(remaining, MAX_SUBSTEP)
            const acceleration = this.stiffness * (target - this.value) - this.damping * this.velocity
            this.velocity += acceleration * step
            this.value += this.velocity * step
            remaining -= step
        }
        return this.value
    }

    /** Jump directly to `value` and zero the velocity. */
    snap(value: number): void {
        this.value = value
        this.velocity = 0
    }

    /** Whether the spring has effectively settled at `target`. */
    settled(target: number, epsilon = 0.001): boolean {
        return Math.abs(this.value - target) < epsilon && Math.abs(this.velocity) < epsilon
    }
}

/** Create a damped spring starting at `initial`. */
export function spring(initial: number, options?: SpringOptions): Spring {
    return new Spring(initial, options)
}
