/**
 * Polar placement for round displays.
 */

import type { Point, Rect } from './rect'
import { center } from './rect'

/**
 * Point at `angle` radians and `radius` pixels from a center point.
 * Angle zero points right; positive angles go clockwise in canvas space.
 */
export function polar(centerPoint: Point, radius: number, angle: number): Point {
    return {
        x: centerPoint.x + Math.cos(angle) * radius,
        y: centerPoint.y + Math.sin(angle) * radius,
    }
}

export interface RingOptions {
    /** Ring radius as a fraction of the area's smaller half-extent. */
    radiusFrac?: number
    /** Angle of the first point in radians (default: top, -PI/2). */
    startAngle?: number
}

/**
 * `n` evenly spaced points around the center of `area`, starting at the
 * top and proceeding clockwise.
 */
export function ring(area: Rect, n: number, options: RingOptions = {}): Point[] {
    const count = Math.max(Math.floor(n), 1)
    const radiusFrac = options.radiusFrac ?? 1
    const startAngle = options.startAngle ?? -Math.PI / 2
    const centerPoint = center(area)
    const radius = (Math.min(area.width, area.height) / 2) * radiusFrac

    const points: Point[] = []
    for (let i = 0; i < count; i++) {
        const angle = startAngle + (i / count) * Math.PI * 2
        points.push(polar(centerPoint, radius, angle))
    }
    return points
}
