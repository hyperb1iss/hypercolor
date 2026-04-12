/**
 * Arc gauge — animated circular progress indicator.
 *
 * Draws a thick arc from a configurable start angle, filling proportionally
 * to the value. Supports gradient fills, neon glow, and smooth animation.
 */

import { withGlow } from '../faces/tokens'

export interface ArcGaugeOptions {
    /** Center X in canvas coordinates. */
    cx: number
    /** Center Y in canvas coordinates. */
    cy: number
    /** Outer radius of the gauge arc. */
    radius: number
    /** Arc thickness (stroke width). */
    thickness: number
    /** Value 0–1 to fill. */
    value: number
    /** Fill color — single hex or [start, end] for gradient along arc. */
    fillColor: string | readonly [string, string]
    /** Background track color (default: subtle white). */
    trackColor?: string
    /** Start angle in radians (default: 0.75π — bottom-left, sweeping clockwise). */
    startAngle?: number
    /** Total sweep angle in radians (default: 1.5π — 270°). */
    sweep?: number
    /** Glow intensity 0–1 (0 = no glow). */
    glow?: number
    /** Line cap style (default: 'round'). */
    cap?: CanvasLineCap
}

const DEFAULT_START = Math.PI * 0.75
const DEFAULT_SWEEP = Math.PI * 1.5

export function arcGauge(ctx: CanvasRenderingContext2D, opts: ArcGaugeOptions): void {
    const {
        cx,
        cy,
        radius,
        thickness,
        value,
        fillColor,
        trackColor = 'rgba(255, 255, 255, 0.06)',
        startAngle = DEFAULT_START,
        sweep = DEFAULT_SWEEP,
        glow = 0,
        cap = 'round',
    } = opts

    const clamped = Math.max(0, Math.min(1, value))
    const endAngle = startAngle + sweep
    const fillEnd = startAngle + sweep * clamped

    ctx.lineCap = cap
    ctx.lineWidth = thickness

    // Background track
    ctx.beginPath()
    ctx.arc(cx, cy, radius, startAngle, endAngle)
    ctx.strokeStyle = trackColor
    ctx.stroke()

    if (clamped <= 0) return

    // Fill arc
    const resolveColor = (): string | CanvasGradient => {
        if (typeof fillColor === 'string') return fillColor
        // Gradient along the arc — approximate with linear gradient
        const [a, b] = fillColor
        const gradStart = {
            x: cx + Math.cos(startAngle) * radius,
            y: cy + Math.sin(startAngle) * radius,
        }
        const gradEnd = {
            x: cx + Math.cos(fillEnd) * radius,
            y: cy + Math.sin(fillEnd) * radius,
        }
        const grad = ctx.createLinearGradient(gradStart.x, gradStart.y, gradEnd.x, gradEnd.y)
        grad.addColorStop(0, a)
        grad.addColorStop(1, b)
        return grad
    }

    const draw = () => {
        ctx.beginPath()
        ctx.arc(cx, cy, radius, startAngle, fillEnd)
        ctx.strokeStyle = resolveColor()
        ctx.stroke()
    }

    if (glow > 0) {
        const glowColor = typeof fillColor === 'string' ? fillColor : fillColor[0]
        withGlow(ctx, glowColor, glow, draw)
    } else {
        draw()
    }
}
