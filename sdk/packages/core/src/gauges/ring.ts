/**
 * Ring gauge — circular progress with centered value text.
 *
 * Thinner and cleaner than arc gauge. Shows the value and optional label
 * text centered inside the ring. Great for single-metric displays.
 */

import { withGlow } from '../faces/tokens'

export interface RingGaugeOptions {
    /** Center X. */
    cx: number
    /** Center Y. */
    cy: number
    /** Ring outer radius. */
    radius: number
    /** Ring thickness (default: radius * 0.12). */
    thickness?: number
    /** Value 0–1. */
    value: number
    /** Ring fill color. */
    color: string
    /** Background track color. */
    trackColor?: string
    /** Value text to display in the center (e.g., "65°"). */
    valueText?: string
    /** Value text font (default: bold, sized to fit). */
    valueFont?: string
    /** Value text color (default: same as ring color). */
    valueColor?: string
    /** Label below value text (e.g., "CPU"). */
    label?: string
    /** Label font. */
    labelFont?: string
    /** Label color. */
    labelColor?: string
    /** Glow intensity 0–1. */
    glow?: number
}

export function ringGauge(ctx: CanvasRenderingContext2D, opts: RingGaugeOptions): void {
    const {
        cx,
        cy,
        radius,
        thickness = Math.max(3, radius * 0.12),
        value,
        color,
        trackColor = 'rgba(255, 255, 255, 0.06)',
        valueText,
        valueFont,
        valueColor = color,
        label,
        labelFont,
        labelColor = 'rgba(255, 255, 255, 0.5)',
        glow = 0,
    } = opts

    const clamped = Math.max(0, Math.min(1, value))
    const startAngle = -Math.PI / 2
    const endAngle = startAngle + Math.PI * 2 * clamped

    ctx.lineCap = 'round'
    ctx.lineWidth = thickness

    // Track
    ctx.beginPath()
    ctx.arc(cx, cy, radius, 0, Math.PI * 2)
    ctx.strokeStyle = trackColor
    ctx.stroke()

    // Fill
    if (clamped > 0) {
        const draw = () => {
            ctx.beginPath()
            ctx.arc(cx, cy, radius, startAngle, endAngle)
            ctx.strokeStyle = color
            ctx.stroke()
        }
        if (glow > 0) {
            withGlow(ctx, color, glow, draw)
        } else {
            draw()
        }
    }

    // Value text
    if (valueText) {
        const fontSize = valueFont ? 0 : Math.round(radius * 0.55)
        ctx.font = valueFont ?? `bold ${fontSize}px 'JetBrains Mono', monospace`
        ctx.fillStyle = valueColor
        ctx.textAlign = 'center'
        ctx.textBaseline = label ? 'bottom' : 'middle'
        const textY = label ? cy - radius * 0.05 : cy
        ctx.fillText(valueText, cx, textY)
    }

    // Label
    if (label) {
        const labelSize = labelFont ? 0 : Math.round(radius * 0.22)
        ctx.font = labelFont ?? `${labelSize}px 'Inter', sans-serif`
        ctx.fillStyle = labelColor
        ctx.textAlign = 'center'
        ctx.textBaseline = 'top'
        ctx.fillText(label, cx, cy + radius * 0.12)
    }
}
