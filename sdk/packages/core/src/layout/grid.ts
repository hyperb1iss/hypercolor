/**
 * Grid and rail solvers — plain JS over the display descriptor.
 *
 * Servo does not lay out CSS grid (see the pixel-probe matrix in
 * hypercolor-core), so faces compute cell rects directly and draw or
 * absolutely position into them.
 */

import type { Rect } from './rect'

/**
 * Split `area` into `cols` x `rows` cells with `gap` pixels between
 * them. Returns cells in row-major order (left to right, top to bottom).
 */
export function grid(area: Rect, cols: number, rows: number, gap = 0): Rect[] {
    const columnCount = Math.max(Math.floor(cols), 1)
    const rowCount = Math.max(Math.floor(rows), 1)
    const cellWidth = (area.width - gap * (columnCount - 1)) / columnCount
    const cellHeight = (area.height - gap * (rowCount - 1)) / rowCount

    const cells: Rect[] = []
    for (let row = 0; row < rowCount; row++) {
        for (let col = 0; col < columnCount; col++) {
            cells.push({
                height: Math.max(cellHeight, 0),
                width: Math.max(cellWidth, 0),
                x: area.x + col * (cellWidth + gap),
                y: area.y + row * (cellHeight + gap),
            })
        }
    }
    return cells
}

/**
 * Split `area` into `n` horizontal slots with `gap` pixels between
 * them — the band layout for strip displays.
 */
export function rail(area: Rect, n: number, gap = 0): Rect[] {
    return grid(area, n, 1, gap)
}
