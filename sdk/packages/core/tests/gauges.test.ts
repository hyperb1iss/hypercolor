import { describe, expect, test } from 'bun:test'

import { createArcGauge, sparkline } from '../src/gauges'
import { linear } from '../src/motion'

/** Canvas context mock that records draw operations. */
function recordingContext() {
    const ops: Array<{ op: string; args: unknown[] }> = []
    const record =
        (op: string) =>
        (...args: unknown[]) => {
            ops.push({ args, op })
        }
    const ctx = {
        arc: record('arc'),
        beginPath: record('beginPath'),
        closePath: record('closePath'),
        createLinearGradient: () => ({
            addColorStop: (_offset: number, stopColor: string) => {
                if (!/^#[0-9a-f]{6,8}$/i.test(stopColor) && !stopColor.startsWith('rgba(')) {
                    throw new SyntaxError('The string did not match the expected pattern.')
                }
                if (/^rgba\(.*\)[0-9a-f]{2}$/i.test(stopColor)) {
                    throw new SyntaxError('The string did not match the expected pattern.')
                }
            },
        }),
        fill: record('fill'),
        fillStyle: '' as unknown,
        lineCap: 'round',
        lineJoin: 'round',
        lineTo: record('lineTo'),
        lineWidth: 1,
        moveTo: record('moveTo'),
        ops,
        stroke: record('stroke'),
        strokeHistory: [] as string[],
        get strokeStyle() {
            return this.strokeHistory.at(-1) ?? ''
        },
        set strokeStyle(value: string) {
            this.strokeHistory.push(value)
        },
    }
    return ctx as unknown as CanvasRenderingContext2D & {
        ops: typeof ops
        strokeHistory: string[]
    }
}

describe('createArcGauge', () => {
    test('sweeps in from zero on appear', () => {
        const gauge = createArcGauge(
            { cx: 0, cy: 0, fillColor: '#80ffea', radius: 100, thickness: 10 },
            { duration: 1, easing: linear },
        )
        const ctx = recordingContext()

        gauge.draw(ctx, 0.8, 0)
        expect(gauge.value()).toBeCloseTo(0)

        gauge.draw(ctx, 0.8, 0.5)
        expect(gauge.value()).toBeCloseTo(0.4)

        gauge.draw(ctx, 0.8, 1)
        expect(gauge.value()).toBeCloseTo(0.8)
    })

    test('value changes glide instead of jumping', () => {
        const gauge = createArcGauge(
            { cx: 0, cy: 0, fillColor: '#80ffea', radius: 100, thickness: 10 },
            { duration: 1, easing: linear },
        )
        const ctx = recordingContext()

        gauge.draw(ctx, 0.4, 0)
        gauge.draw(ctx, 0.4, 2)
        expect(gauge.value()).toBeCloseTo(0.4)

        // Retarget begins the glide at this timestamp.
        gauge.draw(ctx, 0.9, 2.5)
        expect(gauge.value()).toBeCloseTo(0.4)
        gauge.draw(ctx, 0.9, 3)
        expect(gauge.value()).toBeCloseTo(0.65)
        gauge.draw(ctx, 0.9, 3.5)
        expect(gauge.value()).toBeCloseTo(0.9)
    })
})

describe('sparkline threshold bands', () => {
    const base = {
        color: '#ffffff',
        fill: false,
        height: 100,
        range: [0, 100] as [number, number],
        width: 200,
        x: 0,
        y: 0,
    }

    test('segments take the color of their band', () => {
        const ctx = recordingContext()
        sparkline(ctx, {
            ...base,
            bands: [
                { color: '#50fa7b', min: 0 },
                { color: '#ff6363', min: 80 },
            ],
            values: [10, 20, 30, 90, 95],
        })

        expect(ctx.strokeHistory).toEqual(['#50fa7b', '#ff6363'])
    })

    test('uniform values stroke once with one color', () => {
        const ctx = recordingContext()
        sparkline(ctx, {
            ...base,
            bands: [{ color: '#50fa7b', min: 0 }],
            values: [10, 20, 30],
        })

        expect(ctx.strokeHistory).toEqual(['#50fa7b'])
    })

    test('without bands the line uses the base color', () => {
        const ctx = recordingContext()
        sparkline(ctx, { ...base, values: [10, 20, 30] })
        expect(ctx.strokeHistory).toEqual(['#ffffff'])
    })
})

describe('sparkline draw-in', () => {
    test('reveals a prefix of the series without stretching', () => {
        const ctx = recordingContext()
        const values = [0, 25, 50, 75, 100]
        sparkline(ctx, {
            color: '#80ffea',
            drawIn: 0.6,
            fill: false,
            height: 100,
            range: [0, 100],
            values,
            width: 400,
            x: 0,
            y: 0,
        })

        // ceil(5 * 0.6) = 3 points revealed: one moveTo + two lineTo.
        const lineOps = ctx.ops.filter((op) => op.op === 'lineTo')
        expect(lineOps).toHaveLength(2)
        // Spacing anchored to the full series: third point at x = 200.
        expect(lineOps.at(-1)?.args[0]).toBeCloseTo(200)
    })

    test('drawIn of zero draws nothing', () => {
        const ctx = recordingContext()
        sparkline(ctx, {
            color: '#80ffea',
            drawIn: 0,
            fill: false,
            height: 100,
            range: [0, 100],
            values: [0, 50, 100],
            width: 400,
            x: 0,
            y: 0,
        })

        expect(ctx.ops.filter((op) => op.op === 'stroke')).toHaveLength(0)
    })

    test('full drawIn matches the default rendering', () => {
        const full = recordingContext()
        const implicit = recordingContext()
        const options = {
            color: '#80ffea',
            fill: false,
            height: 100,
            range: [0, 100] as [number, number],
            values: [0, 50, 100],
            width: 400,
            x: 0,
            y: 0,
        }
        sparkline(full, { ...options, drawIn: 1 })
        sparkline(implicit, options)

        expect(full.ops).toEqual(implicit.ops)
    })
})

describe('sparkline fill color handling', () => {
    const base = {
        color: '#80ffea',
        height: 100,
        range: [0, 100] as [number, number],
        values: [10, 50, 90],
        width: 300,
        x: 0,
        y: 0,
    }

    test('hex colors keep the gradient fade', () => {
        const ctx = recordingContext()
        sparkline(ctx, { ...base, fill: true })
        expect(ctx.ops.some((op) => op.op === 'fill')).toBe(true)
    })

    test('rgba colors fill without throwing', () => {
        const ctx = recordingContext()
        // Regression: addColorStop('rgba(...)cc') threw SyntaxError in
        // Servo and killed face rendering every frame.
        expect(() => sparkline(ctx, { ...base, color: 'rgba(128, 255, 234, 0.8)', fill: true })).not.toThrow()
        expect(ctx.ops.some((op) => op.op === 'fill')).toBe(true)
    })
})
