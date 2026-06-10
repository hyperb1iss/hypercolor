/**
 * Corner, edge, and center anchoring within an area.
 */

import type { Rect } from './rect'

export type AnchorPosition =
    | 'bottom'
    | 'bottom-left'
    | 'bottom-right'
    | 'center'
    | 'left'
    | 'right'
    | 'top'
    | 'top-left'
    | 'top-right'

export interface AnchorSize {
    width: number
    height: number
}

/**
 * Place a box of `size` at `position` within `area`, inset by `margin`
 * pixels from the touched edges.
 */
export function anchor(area: Rect, position: AnchorPosition, size: AnchorSize, margin = 0): Rect {
    const left = area.x + margin
    const right = area.x + area.width - size.width - margin
    const top = area.y + margin
    const bottom = area.y + area.height - size.height - margin
    const centerX = area.x + (area.width - size.width) / 2
    const centerY = area.y + (area.height - size.height) / 2

    const positions: Record<AnchorPosition, { x: number; y: number }> = {
        bottom: { x: centerX, y: bottom },
        'bottom-left': { x: left, y: bottom },
        'bottom-right': { x: right, y: bottom },
        center: { x: centerX, y: centerY },
        left: { x: left, y: centerY },
        right: { x: right, y: centerY },
        top: { x: centerX, y: top },
        'top-left': { x: left, y: top },
        'top-right': { x: right, y: top },
    }

    const spot = positions[position]
    return { height: size.height, width: size.width, x: spot.x, y: spot.y }
}
