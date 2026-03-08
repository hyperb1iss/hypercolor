import { canvas, audio } from '@hypercolor/sdk'
import type { AudioData } from '@hypercolor/sdk'

// ── Types ────────────────────────────────────────────────────────────────

interface Ring {
    x: number
    y: number
    radius: number
    speed: number
    width: number
    age: number
    life: number
    hueT: number
}

// ── Constants ────────────────────────────────────────────────────────────

const SCENES = ['Core Burst', 'Twin Burst', 'Cascade']
const TAU = Math.PI * 2

// ── Helpers ──────────────────────────────────────────────────────────────

function clamp(v: number, lo: number, hi: number): number {
    return Math.max(lo, Math.min(hi, v))
}

// ── Effect ───────────────────────────────────────────────────────────────

export default canvas.stateful('Bass Shockwave', {
    speed:     [1, 10, 6],
    intensity: [0, 100, 78],
    ringCount: [2, 12, 6],
    decay:     [0, 100, 52],
    palette:   ['SilkCircuit', 'Cyberpunk', 'Fire', 'Aurora', 'Ice'],
    scene:     SCENES,
}, () => {
    let rings: Ring[] = []
    let lastTime = -1
    let beatCooldown = 0
    let fallbackPhase = 0
    let hueCounter = 0

    function emitterPositions(scene: string, w: number, h: number): [number, number][] {
        const cx = w * 0.5
        const cy = h * 0.5
        if (scene === 'Twin Burst') {
            return [
                [cx - w * 0.22, cy],
                [cx + w * 0.22, cy],
            ]
        }
        if (scene === 'Cascade') {
            return [
                [cx, h * 0.12],
                [cx - w * 0.3, h * 0.55],
                [cx + w * 0.3, h * 0.55],
            ]
        }
        return [[cx, cy]]
    }

    function spawnRing(
        x: number, y: number, speedMul: number, intensityMix: number,
    ): Ring {
        // Bold ring widths — 10-24px at 320x200. Easily visible on LEDs.
        const width = 10 + intensityMix * 8 + Math.random() * 6
        hueCounter += 0.13 + Math.random() * 0.07
        return {
            x, y,
            radius: 4,
            speed: 60 + speedMul * 80 + Math.random() * 30,
            width,
            age: 0,
            life: 1.0 + Math.random() * 0.6 + (1 - intensityMix) * 0.4,
            hueT: hueCounter,
        }
    }

    function resolveAudio(a: AudioData): { shouldSpawn: boolean; pulse: number } {
        const audioPresent = a.level > 0.04 || a.bass > 0.04
        if (audioPresent) {
            // Threshold-based: clear visual event on beat, not proportional nudge
            const shouldSpawn = a.beatPulse > 0.45 || a.onsetPulse > 0.55
            const pulse = clamp(Math.max(a.bass, a.beatPulse * 0.8), 0, 1)
            return { shouldSpawn, pulse }
        }
        // Fallback — synthetic beats when no audio
        const fb = Math.pow(Math.max(0, Math.sin(fallbackPhase * 1.7)), 8)
        return { shouldSpawn: fb > 0.7, pulse: fb * 0.65 }
    }

    return (ctx, time, c) => {
        const speed = c.speed as number
        const intensity = clamp((c.intensity as number) / 100, 0, 1)
        const maxRings = Math.round(c.ringCount as number)
        const decay = clamp((c.decay as number) / 100, 0, 1)
        const palette = c.palette as (t: number, alpha?: number) => string
        const scene = c.scene as string

        const w = ctx.canvas.width
        const h = ctx.canvas.height
        const dt = lastTime < 0 ? 1 / 60 : Math.min(0.05, time - lastTime)
        lastTime = time

        fallbackPhase += dt * (0.8 + speed * 0.3)

        // Audio analysis
        const a = audio()
        const { shouldSpawn, pulse } = resolveAudio(a)

        // Spawn rings on threshold beat events
        beatCooldown = Math.max(0, beatCooldown - dt)
        if (shouldSpawn && beatCooldown <= 0) {
            const emitters = emitterPositions(scene, w, h)
            const speedMul = speed / 10
            for (const [ex, ey] of emitters) {
                if (rings.length < maxRings * emitters.length * 2) {
                    rings.push(spawnRing(ex, ey, speedMul, intensity))
                }
            }
            beatCooldown = 0.1 + (1 - speed / 10) * 0.18
        }

        // Trail/fade overlay — the universal LED animation technique.
        // Low decay = long comet trails (0.06), high decay = short snappy (0.28)
        const fadeAlpha = 0.06 + decay * 0.22
        ctx.fillStyle = `rgba(0, 0, 0, ${fadeAlpha.toFixed(3)})`
        ctx.fillRect(0, 0, w, h)

        // Additive blending — how overlapping light sources naturally combine
        ctx.save()
        ctx.globalCompositeOperation = 'lighter'

        for (const ring of rings) {
            ring.age += dt
            ring.radius += ring.speed * dt * (1 + intensity * 0.4)

            const lifeFrac = clamp(ring.age / ring.life, 0, 1)
            if (lifeFrac >= 1) continue

            // Sinusoidal easing: peaks at mid-life, fades organically to zero
            const alpha = Math.sin(lifeFrac * Math.PI) * (0.55 + intensity * 0.45)
            if (alpha < 0.02) continue

            const ringWidth = ring.width * (0.9 + lifeFrac * 0.4)

            // Main ring — wide bold stroke, easily readable on LED hardware
            ctx.lineWidth = ringWidth
            ctx.strokeStyle = palette(ring.hueT, alpha * 0.85)
            ctx.beginPath()
            ctx.arc(ring.x, ring.y, ring.radius, 0, TAU)
            ctx.stroke()

            // Broader soft halo — not fine-detail bloom, just a wider stroke
            ctx.lineWidth = ringWidth * 2.8
            ctx.strokeStyle = palette(ring.hueT + 0.08, alpha * 0.12)
            ctx.beginPath()
            ctx.arc(ring.x, ring.y, ring.radius, 0, TAU)
            ctx.stroke()
        }

        // Core glow at emitter positions — hot-spot focal points
        const emitters = emitterPositions(scene, w, h)
        const coreBrightness = 0.25 + pulse * 0.6

        for (const [ex, ey] of emitters) {
            const coreSize = 14 + coreBrightness * 28

            const grad = ctx.createRadialGradient(ex, ey, 0, ex, ey, coreSize)
            grad.addColorStop(0, palette(hueCounter + 0.5, coreBrightness * 0.8))
            grad.addColorStop(0.4, palette(hueCounter + 0.6, coreBrightness * 0.25))
            grad.addColorStop(1, 'rgba(0,0,0,0)')
            ctx.fillStyle = grad
            ctx.beginPath()
            ctx.arc(ex, ey, coreSize, 0, TAU)
            ctx.fill()
        }

        ctx.restore()

        // Cull dead rings
        rings = rings.filter(r => r.age < r.life)
    }
}, {
    description: 'Bold bass-reactive shockwave rings with threshold-triggered bursts',
})
