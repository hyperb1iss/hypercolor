/**
 * Sparkline — mini line chart from a rolling value buffer.
 *
 * Draws a smooth line with optional gradient fill underneath.
 * Pair with `ValueHistory` for automatic rolling buffer management.
 */

export interface SparklineOptions {
    /** Left edge. */
    x: number
    /** Top edge. */
    y: number
    /** Chart width. */
    width: number
    /** Chart height. */
    height: number
    /** Value buffer (newest value last). */
    values: number[]
    /** Value range [min, max]. Values outside the range are clamped. */
    range: [number, number]
    /** Line color. */
    color: string
    /** Line width (default: 1.5). */
    lineWidth?: number
    /** Fill area under the line (default: true). */
    fill?: boolean
    /** Fill gradient opacity (default: 0.15). */
    fillOpacity?: number
}

export function sparkline(ctx: CanvasRenderingContext2D, opts: SparklineOptions): void {
    const { x, y, width, height, values, range, color, lineWidth = 1.5, fill = true, fillOpacity = 0.15 } = opts

    if (values.length < 2) return

    const [rangeMin, rangeMax] = range
    const rangeSpan = rangeMax - rangeMin || 1

    const points: Array<{ px: number; py: number }> = []
    for (let i = 0; i < values.length; i++) {
        const normalized = Math.max(0, Math.min(1, (values[i] - rangeMin) / rangeSpan))
        points.push({
            px: x + (i / (values.length - 1)) * width,
            py: y + height - normalized * height,
        })
    }

    // Fill gradient under the line
    if (fill) {
        ctx.beginPath()
        ctx.moveTo(points[0].px, y + height)
        for (const pt of points) ctx.lineTo(pt.px, pt.py)
        ctx.lineTo(points[points.length - 1].px, y + height)
        ctx.closePath()

        const grad = ctx.createLinearGradient(x, y, x, y + height)
        grad.addColorStop(
            0,
            `${color}${Math.round(fillOpacity * 255)
                .toString(16)
                .padStart(2, '0')}`,
        )
        grad.addColorStop(1, `${color}00`)
        ctx.fillStyle = grad
        ctx.fill()
    }

    // Line
    ctx.beginPath()
    ctx.moveTo(points[0].px, points[0].py)
    for (let i = 1; i < points.length; i++) {
        ctx.lineTo(points[i].px, points[i].py)
    }
    ctx.strokeStyle = color
    ctx.lineWidth = lineWidth
    ctx.lineJoin = 'round'
    ctx.lineCap = 'round'
    ctx.stroke()
}

/**
 * Rolling value history buffer for sparklines.
 *
 * ```typescript
 * const history = new ValueHistory(60) // 60 samples
 * // In update loop:
 * history.push(sensorValue)
 * sparkline(ctx, { values: history.values(), ... })
 * ```
 */
export class ValueHistory {
    private buffer: number[]
    private capacity: number
    private count = 0

    constructor(capacity: number) {
        this.capacity = Math.max(2, capacity)
        this.buffer = new Array(this.capacity).fill(0)
    }

    /** Add a value. Drops the oldest if at capacity. */
    push(value: number): void {
        if (this.count < this.capacity) {
            this.buffer[this.count] = value
            this.count++
        } else {
            // Shift left and append
            for (let i = 1; i < this.capacity; i++) {
                this.buffer[i - 1] = this.buffer[i]
            }
            this.buffer[this.capacity - 1] = value
        }
    }

    /** Current values (oldest first, newest last). */
    values(): number[] {
        return this.buffer.slice(0, this.count)
    }

    /** Most recently pushed value. */
    latest(): number {
        return this.count > 0 ? this.buffer[this.count - 1] : 0
    }

    /** Number of values currently stored. */
    get length(): number {
        return this.count
    }
}
