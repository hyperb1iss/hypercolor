import { describe, expect, test } from 'bun:test'

import { layoutStack, layoutStrip, type NowPlayingSizes } from '../../../src/faces/now-playing/layout'
import { clearWidth } from '../../../src/faces/shared/typography'
import type { FaceContext } from '../src/faces/context'
import { resolveDisplayInfo } from '../src/faces/context'

function panel(width: number, height: number, circular: boolean): FaceContext {
    return {
        circular,
        display: resolveDisplayInfo(width, height, circular, undefined),
        dpr: 1,
        height,
        scale: 1,
        width,
    } as FaceContext
}

const DEFAULTS: NowPlayingSizes = {
    artSize: 44,
    detailSize: 24,
    showArt: true,
    showProgress: true,
    showTimes: true,
    titleSize: 44,
}

/** Top and bottom of everything the stack draws, ring included. */
function stackExtent(ctx: FaceContext, sizes: NowPlayingSizes): { top: number; bottom: number } {
    const layout = layoutStack(ctx, sizes)
    const rows = sizes.showTimes ? [layout.title, layout.artist, layout.times] : [layout.title, layout.artist]
    const text = rows.reduce((sum, row) => sum + row.height, 0) + layout.gap * (rows.length - 1)
    const art = layout.art > 0 ? layout.art + 2 * layout.artOutset + layout.gap : 0
    const center = ctx.height / 2
    return { bottom: center + (art + text) / 2, top: center - (art + text) / 2 }
}

describe('clearWidth', () => {
    const round = panel(480, 480, true)

    test('a line through the middle of a round panel runs past the inscribed square', () => {
        expect(clearWidth(round, 230, 250, 0)).toBeGreaterThan(470)
        expect(clearWidth(round, 230, 250, 0)).toBeGreaterThan(round.display.safeArea.width)
    })

    test('a line narrows as it nears the edge and vanishes past it', () => {
        expect(clearWidth(round, 380, 420, 0)).toBeLessThan(clearWidth(round, 300, 340, 0))
        expect(clearWidth(round, 470, 500, 0)).toBe(0)
    })

    test('other shapes give every line the safe width less its insets', () => {
        expect(clearWidth(panel(960, 160, false), 10, 40, 12)).toBe(936)
    })
})

describe('Now Playing stack', () => {
    const round = panel(480, 480, true)

    test('the default sizes keep art, ring, and every line inside the safe area', () => {
        const layout = layoutStack(round, DEFAULTS)
        const extent = stackExtent(round, DEFAULTS)
        const safe = round.display.safeArea
        expect(layout.art).toBeGreaterThan(140)
        expect(layout.title.fontSize).toBe(44)
        expect(extent.top).toBeGreaterThanOrEqual(safe.y)
        expect(extent.bottom).toBeLessThanOrEqual(safe.y + safe.height)
    })

    test('larger type shrinks the art before it moves a line off the panel', () => {
        const sizes = { ...DEFAULTS, detailSize: 30, titleSize: 64 }
        const layout = layoutStack(round, sizes)
        const extent = stackExtent(round, sizes)
        const safe = round.display.safeArea
        expect(layout.title.fontSize).toBe(64)
        expect(layout.art).toBeGreaterThan(0)
        expect(layout.art).toBeLessThan(layoutStack(round, DEFAULTS).art)
        expect(extent.bottom).toBeLessThanOrEqual(safe.y + safe.height)
    })

    test('type too large for art beside it takes the panel and the ring moves to the edge', () => {
        const layout = layoutStack(round, { ...DEFAULTS, detailSize: 48, titleSize: 96 })
        expect(layout.art).toBe(0)
        expect(layout.ring.cx).toBe(240)
        expect(layout.ring.cy).toBe(240)
        expect(layout.ring.radius).toBeGreaterThan(225)
    })

    test('with art off the largest type still fits the safe height', () => {
        const sizes = { ...DEFAULTS, detailSize: 48, showArt: false, titleSize: 96 }
        const extent = stackExtent(round, sizes)
        const safe = round.display.safeArea
        expect(extent.top).toBeGreaterThanOrEqual(safe.y)
        expect(extent.bottom).toBeLessThanOrEqual(safe.y + safe.height)
    })

    test('the title may run wider than the inscribed square', () => {
        const layout = layoutStack(round, DEFAULTS)
        expect(layout.title.width).toBeGreaterThan(round.display.safeArea.width)
        expect(layout.title.width).toBeLessThan(480)
    })

    test('the ring orbits the art', () => {
        const layout = layoutStack(round, DEFAULTS)
        expect(layout.ring.cx).toBe(240)
        expect(layout.ring.radius).toBeGreaterThan(layout.art / 2)
        expect(layout.ring.cy).toBeLessThan(240)
    })
})

describe('Now Playing strip', () => {
    const strip = panel(960, 160, false)

    test('the default title lands near thirty pixels on a 160 strip', () => {
        const layout = layoutStrip(strip, DEFAULTS)
        expect(layout.titleSize).toBeCloseTo(30.07, 1)
        expect(layout.art).toBeCloseTo(124.8, 1)
    })

    test('the largest type scales down together to fit the strip', () => {
        const layout = layoutStrip(strip, { ...DEFAULTS, detailSize: 48, titleSize: 96 })
        const height = layout.titleSize * 1.16 + layout.artistSize * 1.3 + layout.timesSize * 1.3 + layout.gap * 2
        expect(height).toBeLessThanOrEqual(160 * 0.76 + 0.001)
        expect(layout.titleSize / layout.artistSize).toBeCloseTo(2, 5)
    })

    test('art off leaves the text the whole strip', () => {
        expect(layoutStrip(strip, { ...DEFAULTS, showArt: false }).art).toBe(0)
    })
})
