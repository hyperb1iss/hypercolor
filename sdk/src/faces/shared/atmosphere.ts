/**
 * Shared cinematic atmosphere kit for display faces.
 *
 * Every face composes the same primitives: a color-graded nebula field,
 * slow particle drift, and eased entrance choreography. All motion is
 * time-based and tuned for the 15-30fps Servo range — slow, eased, never
 * strobing.
 */

import { easeOutCubic, lerpColor, withAlpha } from '@hypercolor/sdk'
import { clamp01 } from './dom'

/** Eased 0→1 progress for staggered boot choreography. */
export function entrance(time: number, delay: number, duration = 0.9): number {
    return easeOutCubic(clamp01((time - delay) / duration))
}

/** Full-bleed graded field: deep base rising into two drifting glow masses. */
export function drawNebulaField(
    c: CanvasRenderingContext2D,
    W: number,
    H: number,
    time: number,
    primary: string,
    secondary: string,
    intensity: number,
): void {
    const base = c.createLinearGradient(0, H, 0, 0)
    base.addColorStop(0, withAlpha(primary, 0.1 * intensity))
    base.addColorStop(0.6, withAlpha(secondary, 0.04 * intensity))
    base.addColorStop(1, withAlpha(secondary, 0))
    c.fillStyle = base
    c.fillRect(0, 0, W, H)

    const blobs = [
        { color: primary, phase: 0, radius: 0.58, x: 0.26, y: 0.74 },
        { color: secondary, phase: 2.3, radius: 0.5, x: 0.72, y: 0.4 },
        { color: lerpColor(primary, secondary, 0.5), phase: 4.1, radius: 0.42, x: 0.5, y: 0.88 },
    ]
    for (const blob of blobs) {
        const bx = W * (blob.x + 0.07 * Math.sin(time * 0.1 + blob.phase))
        const by = H * (blob.y + 0.06 * Math.cos(time * 0.08 + blob.phase * 1.6))
        const radius = Math.max(W, H) * blob.radius
        const gradient = c.createRadialGradient(bx, by, 0, bx, by, radius)
        gradient.addColorStop(0, withAlpha(blob.color, 0.085 * intensity))
        gradient.addColorStop(1, withAlpha(blob.color, 0))
        c.fillStyle = gradient
        c.fillRect(0, 0, W, H)
    }
}

export interface Drifter {
    seed: number
    lane: number
}

/** Deterministic particle set — golden-ratio lanes, no RNG in the loop. */
export function makeDrifters(count: number): Drifter[] {
    return Array.from({ length: count }, (_, index) => ({
        lane: (index * 0.618_03) % 1,
        seed: index * 137.508,
    }))
}

/** Slow motes drifting upward across the frame. */
export function drawRisingMotes(
    c: CanvasRenderingContext2D,
    W: number,
    H: number,
    time: number,
    drifters: Drifter[],
    color: string,
    intensity: number,
    activity = 0.5,
): void {
    const visible = Math.max(2, Math.floor(drifters.length * (0.3 + activity * 0.7)))
    for (let index = 0; index < visible; index += 1) {
        const mote = drifters[index]
        if (!mote) continue
        const speed = 0.02 + (mote.seed % 1) * 0.03 + activity * 0.03
        const progress = (time * speed + mote.seed) % 1
        const x = mote.lane * W + Math.sin(time * 0.6 + mote.seed) * W * 0.012
        const y = H * (1.05 - progress * 1.15)
        if (y < -6 || y > H + 6) continue
        const fade = Math.sin(progress * Math.PI)
        c.fillStyle = withAlpha(color, fade * 0.22 * intensity)
        c.beginPath()
        c.arc(x, y, 1 + (mote.seed % 1.8), 0, Math.PI * 2)
        c.fill()
    }
}

/** Comet: a bright head with a fading arc tail along a circle. */
export function drawCometRing(
    c: CanvasRenderingContext2D,
    cx: number,
    cy: number,
    radius: number,
    angle: number,
    color: string,
    intensity: number,
    tailSweep = Math.PI * 0.55,
): void {
    const segments = 26
    for (let index = 0; index < segments; index += 1) {
        const fraction = index / segments
        const alpha = (1 - fraction) ** 2 * 0.6 * intensity
        if (alpha <= 0.01) continue
        const from = angle - fraction * tailSweep
        const to = angle - (fraction + 1 / segments) * tailSweep
        c.strokeStyle = withAlpha(color, alpha)
        c.lineWidth = 2.4 * (1 - fraction * 0.7)
        c.beginPath()
        c.arc(cx, cy, radius, to, from)
        c.stroke()
    }
    c.save()
    c.shadowColor = color
    c.shadowBlur = 14 * intensity
    c.fillStyle = '#ffffff'
    c.beginPath()
    c.arc(cx + Math.cos(angle) * radius, cy + Math.sin(angle) * radius, 3, 0, Math.PI * 2)
    c.fill()
    c.restore()
}

/** Comet running along a horizontal rail (the strip counterpart). */
export function drawCometRail(
    c: CanvasRenderingContext2D,
    left: number,
    right: number,
    y: number,
    progress: number,
    color: string,
    intensity: number,
): void {
    const span = right - left
    const head = left + span * clamp01(progress)
    const tail = Math.max(left, head - span * 0.18)

    c.strokeStyle = withAlpha('#8a8fa8', 0.14)
    c.lineWidth = 1
    c.beginPath()
    c.moveTo(left, y)
    c.lineTo(right, y)
    c.stroke()

    const trail = c.createLinearGradient(tail, 0, head, 0)
    trail.addColorStop(0, withAlpha(color, 0))
    trail.addColorStop(1, withAlpha(color, 0.7 * intensity))
    c.strokeStyle = trail
    c.lineWidth = 2
    c.beginPath()
    c.moveTo(tail, y)
    c.lineTo(head, y)
    c.stroke()

    c.save()
    c.shadowColor = color
    c.shadowBlur = 12 * intensity
    c.fillStyle = '#ffffff'
    c.beginPath()
    c.arc(head, y, 2.6, 0, Math.PI * 2)
    c.fill()
    c.restore()
}
