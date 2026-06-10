/**
 * Frame-rate-independent exponential smoothing.
 *
 * `Smoothed` replaces ad-hoc `value += delta * 0.08` lerps, which converge
 * at different speeds depending on frame rate. The half-life formulation
 * (`factor = 1 - 0.5^(dt / halflife)`) closes exactly half the remaining
 * distance every `halflife` seconds regardless of how many frames that
 * takes.
 */

export class Smoothed {
    /** Current smoothed value. */
    value: number
    private readonly halflife: number

    /**
     * @param initial  starting value
     * @param halflife seconds to close half the distance to the target
     */
    constructor(initial: number, halflife: number) {
        this.value = initial
        this.halflife = Math.max(halflife, 0)
    }

    /** Advance toward `target` by `dt` seconds and return the new value. */
    update(target: number, dt: number): number {
        if (this.halflife === 0 || !Number.isFinite(this.halflife)) {
            this.value = target
            return this.value
        }
        const factor = 1 - 0.5 ** (Math.max(dt, 0) / this.halflife)
        this.value += (target - this.value) * factor
        return this.value
    }

    /** Jump directly to `value` with no smoothing. */
    snap(value: number): void {
        this.value = value
    }
}

/** Create a half-life smoother starting at `initial`. */
export function smoothed(initial: number, halflife: number): Smoothed {
    return new Smoothed(initial, halflife)
}
