/**
 * Shared geometry primitives for face layout.
 */

/** Pixel-space rectangle. */
export interface Rect {
    x: number
    y: number
    width: number
    height: number
}

/** Pixel-space point. */
export interface Point {
    x: number
    y: number
}

/** Center point of a rect. */
export function center(area: Rect): Point {
    return { x: area.x + area.width / 2, y: area.y + area.height / 2 }
}

/** Shrink a rect by `amount` pixels on every side (clamped at zero size). */
export function inset(area: Rect, amount: number): Rect {
    const width = Math.max(area.width - amount * 2, 0)
    const height = Math.max(area.height - amount * 2, 0)
    return { height, width, x: area.x + amount, y: area.y + amount }
}
