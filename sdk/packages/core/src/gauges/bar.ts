/**
 * Bar gauge — horizontal or vertical fill bar with gradient.
 */

import { withGlow } from '../faces/tokens'

export interface BarGaugeOptions {
    /** Bar left edge. */
    x: number
    /** Bar top edge. */
    y: number
    /** Total bar width. */
    width: number
    /** Total bar height. */
    height: number
    /** Value 0–1. */
    value: number
    /** Fill color — single hex or [start, end] gradient. */
    fillColor: string | readonly [string, string]
    /** Background track color. */
    trackColor?: string
    /** Corner radius (default: half the bar's shorter dimension). */
    borderRadius?: number
    /** Fill direction (default: 'horizontal'). */
    direction?: 'horizontal' | 'vertical'
    /** Glow intensity 0–1. */
    glow?: number
    /** Label text drawn to the right of a horizontal bar. */
    label?: string
    /** Label font. */
    labelFont?: string
    /** Label color. */
    labelColor?: string
}

function roundRect(ctx: CanvasRenderingContext2D, x: number, y: number, w: number, h: number, r: number): void {
    const cr = Math.min(r, w / 2, h / 2)
    ctx.beginPath()
    ctx.moveTo(x + cr, y)
    ctx.lineTo(x + w - cr, y)
    ctx.arcTo(x + w, y, x + w, y + cr, cr)
    ctx.lineTo(x + w, y + h - cr)
    ctx.arcTo(x + w, y + h, x + w - cr, y + h, cr)
    ctx.lineTo(x + cr, y + h)
    ctx.arcTo(x, y + h, x, y + h - cr, cr)
    ctx.lineTo(x, y + cr)
    ctx.arcTo(x, y, x + cr, y, cr)
    ctx.closePath()
}

export function barGauge(ctx: CanvasRenderingContext2D, opts: BarGaugeOptions): void {
    const {
        x,
        y,
        width,
        height,
        value,
        fillColor,
        trackColor = 'rgba(255, 255, 255, 0.06)',
        borderRadius,
        direction = 'horizontal',
        glow = 0,
        label,
        labelFont,
        labelColor = 'rgba(255, 255, 255, 0.5)',
    } = opts

    const clamped = Math.max(0, Math.min(1, value))
    const r = borderRadius ?? Math.min(width, height) / 2

    // Track
    roundRect(ctx, x, y, width, height, r)
    ctx.fillStyle = trackColor
    ctx.fill()

    if (clamped <= 0) return

    // Fill
    const fillW = direction === 'horizontal' ? width * clamped : width
    const fillH = direction === 'vertical' ? height * clamped : height
    const fillX = x
    const fillY = direction === 'vertical' ? y + height - fillH : y

    const resolveColor = (): string | CanvasGradient => {
        if (typeof fillColor === 'string') return fillColor
        const [a, b] = fillColor
        if (direction === 'horizontal') {
            const grad = ctx.createLinearGradient(x, y, x + fillW, y)
            grad.addColorStop(0, a)
            grad.addColorStop(1, b)
            return grad
        }
        const grad = ctx.createLinearGradient(x, y + height, x, fillY)
        grad.addColorStop(0, a)
        grad.addColorStop(1, b)
        return grad
    }

    const draw = () => {
        ctx.save()
        // Clip to track shape so fill respects border radius
        roundRect(ctx, x, y, width, height, r)
        ctx.clip()
        roundRect(ctx, fillX, fillY, fillW, fillH, r)
        ctx.fillStyle = resolveColor()
        ctx.fill()
        ctx.restore()
    }

    if (glow > 0) {
        const glowColor = typeof fillColor === 'string' ? fillColor : fillColor[0]
        withGlow(ctx, glowColor, glow, draw)
    } else {
        draw()
    }

    // Label
    if (label) {
        if (direction === 'horizontal') {
            const fontSize = labelFont ? 0 : Math.round(height * 0.7)
            ctx.font = labelFont ?? `${fontSize}px 'Inter', sans-serif`
            ctx.fillStyle = labelColor
            ctx.textAlign = 'right'
            ctx.textBaseline = 'middle'
            ctx.fillText(label, x + width, y + height / 2)
        } else {
            const fontSize = labelFont ? 0 : Math.round(width * 0.25)
            ctx.font = labelFont ?? `${fontSize}px 'Inter', sans-serif`
            ctx.fillStyle = labelColor
            ctx.textAlign = 'center'
            ctx.textBaseline = 'top'
            ctx.fillText(label, x + width / 2, y + height + 4)
        }
    }
}
