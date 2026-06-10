import { describe, expect, test } from 'bun:test'
import type { Rect } from '../src/layout'
import { anchor, center, fitText, grid, inset, polar, rail, ring } from '../src/layout'

const STRIP: Rect = { height: 160, width: 960, x: 0, y: 0 }
const SQUARE: Rect = { height: 480, width: 480, x: 0, y: 0 }

describe('grid', () => {
    test('splits an area into row-major cells', () => {
        const cells = grid(SQUARE, 2, 2, 0)
        expect(cells).toHaveLength(4)
        expect(cells[0]).toEqual({ height: 240, width: 240, x: 0, y: 0 })
        expect(cells[1]).toEqual({ height: 240, width: 240, x: 240, y: 0 })
        expect(cells[2]).toEqual({ height: 240, width: 240, x: 0, y: 240 })
        expect(cells[3]).toEqual({ height: 240, width: 240, x: 240, y: 240 })
    })

    test('gap subtracts from cell sizes, not the area', () => {
        const cells = grid({ height: 100, width: 320, x: 10, y: 20 }, 3, 1, 10)
        expect(cells).toHaveLength(3)
        expect(cells[0]?.width).toBe(100)
        expect(cells[0]?.x).toBe(10)
        expect(cells[1]?.x).toBe(120)
        expect(cells[2]?.x).toBe(230)
        const last = cells[2]
        expect((last?.x ?? 0) + (last?.width ?? 0)).toBeCloseTo(330)
    })

    test('degenerate counts clamp to one cell', () => {
        const cells = grid(SQUARE, 0, 0)
        expect(cells).toHaveLength(1)
        expect(cells[0]).toEqual(SQUARE)
    })
})

describe('rail', () => {
    test('lays horizontal slots across a strip', () => {
        const slots = rail(STRIP, 4, 8)
        expect(slots).toHaveLength(4)
        for (const slot of slots) {
            expect(slot.height).toBe(160)
            expect(slot.y).toBe(0)
        }
        expect(slots[0]?.x).toBe(0)
        expect((slots[3]?.x ?? 0) + (slots[3]?.width ?? 0)).toBeCloseTo(960)
    })
})

describe('polar + ring', () => {
    test('polar places points on the circle', () => {
        const point = polar({ x: 240, y: 240 }, 100, 0)
        expect(point.x).toBeCloseTo(340)
        expect(point.y).toBeCloseTo(240)
    })

    test('ring starts at the top and spaces points evenly', () => {
        const points = ring(SQUARE, 4, { radiusFrac: 0.5 })
        expect(points).toHaveLength(4)
        expect(points[0]?.x).toBeCloseTo(240)
        expect(points[0]?.y).toBeCloseTo(120)
        expect(points[1]?.x).toBeCloseTo(360)
        expect(points[1]?.y).toBeCloseTo(240)
        expect(points[2]?.x).toBeCloseTo(240)
        expect(points[2]?.y).toBeCloseTo(360)
        expect(points[3]?.x).toBeCloseTo(120)
        expect(points[3]?.y).toBeCloseTo(240)
    })
})

describe('anchor', () => {
    const size = { height: 40, width: 100 }

    test('corners respect margins', () => {
        expect(anchor(SQUARE, 'top-left', size, 10)).toEqual({
            height: 40,
            width: 100,
            x: 10,
            y: 10,
        })
        expect(anchor(SQUARE, 'bottom-right', size, 10)).toEqual({
            height: 40,
            width: 100,
            x: 370,
            y: 430,
        })
    })

    test('center ignores margin', () => {
        expect(anchor(SQUARE, 'center', size)).toEqual({ height: 40, width: 100, x: 190, y: 220 })
    })

    test('edges center on the other axis', () => {
        expect(anchor(STRIP, 'left', size, 12)).toEqual({ height: 40, width: 100, x: 12, y: 60 })
        expect(anchor(STRIP, 'bottom', size, 4)).toEqual({ height: 40, width: 100, x: 430, y: 116 })
    })
})

describe('rect helpers', () => {
    test('center finds the midpoint', () => {
        expect(center(STRIP)).toEqual({ x: 480, y: 80 })
    })

    test('inset shrinks from all sides and clamps at zero', () => {
        expect(inset(SQUARE, 40)).toEqual({ height: 400, width: 400, x: 40, y: 40 })
        expect(inset({ height: 10, width: 10, x: 0, y: 0 }, 20)).toEqual({
            height: 0,
            width: 0,
            x: 20,
            y: 20,
        })
    })
})

describe('fitText', () => {
    /** measureText width proportional to font size: each char is 0.6em. */
    function measuringContext(): CanvasRenderingContext2D {
        const ctx = {
            font: '600 16px sans-serif',
            measureText(text: string) {
                const size = Number.parseFloat(/(\d+(?:\.\d+)?)px/.exec(ctx.font)?.[1] ?? '16')
                return { width: text.length * size * 0.6 } as TextMetrics
            },
        }
        return ctx as unknown as CanvasRenderingContext2D
    }

    test('converges on a size that fits the rect', () => {
        const ctx = measuringContext()
        const rect: Rect = { height: 100, width: 300, x: 0, y: 0 }
        const size = fitText(ctx, 'HELLO', rect)

        // 5 chars * 0.6em: width fits when size <= 100, height caps at 100.
        expect(size).toBeGreaterThanOrEqual(98)
        expect(size).toBeLessThanOrEqual(100)
        expect(ctx.font).toContain(`${size}px`)
    })

    test('long text shrinks below the height cap', () => {
        const ctx = measuringContext()
        const rect: Rect = { height: 100, width: 300, x: 0, y: 0 }
        const size = fitText(ctx, 'A MUCH LONGER LABEL', rect)
        expect(size).toBeLessThan(30)
        expect(size).toBeGreaterThanOrEqual(6)
    })

    test('maxSize wins when everything fits', () => {
        const ctx = measuringContext()
        const size = fitText(ctx, 'OK', { height: 400, width: 480, x: 0, y: 0 }, { maxSize: 48 })
        expect(size).toBe(48)
    })
})
