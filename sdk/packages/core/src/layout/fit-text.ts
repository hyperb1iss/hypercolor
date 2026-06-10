/**
 * Binary-search font sizing into a rect.
 */

import type { Rect } from './rect'

export interface FitTextOptions {
    /** Font family (default: sans-serif). */
    family?: string
    /** CSS font weight (default: 600). */
    weight?: number | string
    /** Smallest acceptable size in px (default: 6). */
    minSize?: number
    /** Largest acceptable size in px (default: the rect height). */
    maxSize?: number
}

/**
 * Largest font size (px) at which `text` fits inside `rect`, measured
 * with the canvas context. Sets `ctx.font` to the winning font so the
 * caller can draw immediately; returns the chosen size.
 */
export function fitText(ctx: CanvasRenderingContext2D, text: string, rect: Rect, options: FitTextOptions = {}): number {
    const family = options.family ?? 'sans-serif'
    const weight = options.weight ?? 600
    const minSize = Math.max(options.minSize ?? 6, 1)
    const maxSize = Math.max(options.maxSize ?? rect.height, minSize)

    const fits = (size: number): boolean => {
        ctx.font = `${weight} ${size}px ${family}`
        return ctx.measureText(text).width <= rect.width && size <= rect.height
    }

    if (fits(maxSize)) {
        return maxSize
    }

    let low = minSize
    let high = maxSize
    while (high - low > 0.5) {
        const mid = (low + high) / 2
        if (fits(mid)) {
            low = mid
        } else {
            high = mid
        }
    }

    const size = Math.floor(low)
    ctx.font = `${weight} ${size}px ${family}`
    return size
}
