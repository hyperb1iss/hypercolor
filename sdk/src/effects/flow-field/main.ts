import { canvas } from '@hypercolor/sdk'

// ── Types ────────────────────────────────────────────────────────────────

interface TrailPoint { x: number; y: number }

interface Firefly {
    x: number
    y: number
    vx: number
    vy: number
    phase: number
    hueOffset: number
    satOffset: number
    lightOffset: number
    sizeJitter: number
    speedBias: number
    wanderBias: number
    trail: TrailPoint[]
}

interface RGB { r: number; g: number; b: number }
interface HSL { h: number; s: number; l: number }

interface SceneTuning {
    speed: number
    wander: number
    swirl: number
    cohesion: number
    pulse: number
    trail: number
    sparkle: number
}

// ── Constants ────────────────────────────────────────────────────────────

const SCENES = ['Calm', 'Swarm', 'Pulse']
const COLOR_MODES = ['Single', 'Random', 'Rainbow']

const SCENE_TUNING: Record<string, SceneTuning> = {
    Calm: {
        speed: 0.72, wander: 0.55, swirl: 0.35, cohesion: 0.072,
        pulse: 0.2, trail: 10, sparkle: 0.7,
    },
    Swarm: {
        speed: 1.12, wander: 1.12, swirl: 0.8, cohesion: 0.096,
        pulse: 0.35, trail: 7, sparkle: 1.0,
    },
    Pulse: {
        speed: 0.9, wander: 0.74, swirl: 0.58, cohesion: 0.082,
        pulse: 1.0, trail: 9, sparkle: 1.22,
    },
}

// ── Helpers ──────────────────────────────────────────────────────────────

function clamp(value: number, min: number, max: number): number {
    if (Number.isNaN(value)) return min
    return Math.max(min, Math.min(max, value))
}

function rgba(color: RGB, alpha: number): string {
    return `rgba(${color.r}, ${color.g}, ${color.b}, ${clamp(alpha, 0, 1).toFixed(3)})`
}

function hexToRgb(hex: string): RGB {
    const normalized = hex.replace('#', '')
    const full = normalized.length === 3
        ? `${normalized[0]}${normalized[0]}${normalized[1]}${normalized[1]}${normalized[2]}${normalized[2]}`
        : normalized
    const parsed = parseInt(full, 16)
    if (Number.isNaN(parsed)) return { r: 184, g: 255, b: 79 }
    return { r: (parsed >> 16) & 255, g: (parsed >> 8) & 255, b: parsed & 255 }
}

function rgbToHsl(color: RGB): HSL {
    const r = color.r / 255
    const g = color.g / 255
    const b = color.b / 255
    const max = Math.max(r, g, b)
    const min = Math.min(r, g, b)
    const delta = max - min
    const l = (max + min) / 2

    if (delta === 0) return { h: 0, s: 0, l }

    const s = l > 0.5 ? delta / (2 - max - min) : delta / (max + min)
    let h: number
    if (max === r) h = (g - b) / delta + (g < b ? 6 : 0)
    else if (max === g) h = (b - r) / delta + 2
    else h = (r - g) / delta + 4
    h *= 60

    return { h, s, l }
}

function hslToRgb(h: number, sPercent: number, lPercent: number): RGB {
    const s = clamp(sPercent, 0, 100) / 100
    const l = clamp(lPercent, 0, 100) / 100
    const c = (1 - Math.abs(2 * l - 1)) * s
    const hPrime = ((h % 360) + 360) % 360 / 60
    const x = c * (1 - Math.abs((hPrime % 2) - 1))

    let r = 0, g = 0, b = 0
    if (hPrime < 1) [r, g, b] = [c, x, 0]
    else if (hPrime < 2) [r, g, b] = [x, c, 0]
    else if (hPrime < 3) [r, g, b] = [0, c, x]
    else if (hPrime < 4) [r, g, b] = [0, x, c]
    else if (hPrime < 5) [r, g, b] = [x, 0, c]
    else [r, g, b] = [c, 0, x]

    const m = l - c / 2
    return {
        r: Math.round((r + m) * 255),
        g: Math.round((g + m) * 255),
        b: Math.round((b + m) * 255),
    }
}

function createFirefly(w: number, h: number): Firefly {
    const x = Math.random() * w
    const y = Math.random() * h
    return {
        x, y,
        vx: (Math.random() - 0.5) * 0.8,
        vy: (Math.random() - 0.5) * 0.8,
        phase: Math.random() * Math.PI * 2,
        hueOffset: Math.random() * 2 - 1,
        satOffset: Math.random() * 2 - 1,
        lightOffset: Math.random() * 2 - 1,
        sizeJitter: 0.68 + Math.random() * 0.72,
        speedBias: 0.75 + Math.random() * 0.55,
        wanderBias: 0.8 + Math.random() * 0.45,
        trail: [{ x, y }],
    }
}

function wrapFirefly(firefly: Firefly, w: number, h: number): boolean {
    const margin = 12
    let wrapped = false

    if (firefly.x < -margin) { firefly.x = w + margin; wrapped = true }
    else if (firefly.x > w + margin) { firefly.x = -margin; wrapped = true }

    if (firefly.y < -margin) { firefly.y = h + margin; wrapped = true }
    else if (firefly.y > h + margin) { firefly.y = -margin; wrapped = true }

    return wrapped
}

// ── Effect ───────────────────────────────────────────────────────────────

export default canvas.stateful('Flow Field', {
    scene:     SCENES,
    colorMode: COLOR_MODES,
    baseColor: '#b8ff4f',
    count:     [8, 220, 88],
    size:      [1, 10, 3],
    speed:     [1, 10, 5],
    wander:    [0, 100, 62],
    glow:      [0, 100, 72],
    bgColor:   '#05060a',
}, () => {
    let fireflies: Firefly[] = []
    let cachedBaseColor = '#b8ff4f'
    let cachedBaseHsl: HSL = { h: 90, s: 0.9, l: 0.64 }
    let lastTime = -1

    function updateBaseColorCache(color: string): void {
        if (color === cachedBaseColor) return
        cachedBaseColor = color
        cachedBaseHsl = rgbToHsl(hexToRgb(color))
    }

    function ensureFireflyCount(count: number, w: number, h: number): void {
        const target = Math.max(8, Math.min(220, count))
        if (fireflies.length < target) {
            while (fireflies.length < target) fireflies.push(createFirefly(w, h))
        } else if (fireflies.length > target) {
            fireflies.length = target
        }
    }

    function getTwinkle(firefly: Firefly, time: number, scene: SceneTuning): number {
        const pulse = Math.sin(time * (1.4 + scene.sparkle) + firefly.phase * 1.9)
        const shimmer = Math.sin(time * 3.4 + firefly.phase * 6.7) * 0.5
        return clamp(0.54 + pulse * 0.3 + shimmer * 0.2, 0.12, 1)
    }

    function resolveColor(
        firefly: Firefly, index: number, time: number, twinkle: number, count: number,
        colorMode: string,
    ): RGB {
        const base = cachedBaseHsl

        if (colorMode === 'Random') {
            const hue = (base.h + firefly.hueOffset * 260 + Math.sin(time * 0.24 + firefly.phase * 5.3) * 22 + 360) % 360
            const sat = clamp(72 + firefly.satOffset * 22, 48, 100)
            const light = clamp(43 + twinkle * 22 + firefly.lightOffset * 9, 30, 88)
            return hslToRgb(hue, sat, light)
        }

        if (colorMode === 'Rainbow') {
            const hue = (time * 36 + index * (360 / Math.max(count, 1)) + firefly.hueOffset * 40 + 360) % 360
            const sat = clamp(80 + firefly.satOffset * 16, 58, 100)
            const light = clamp(48 + twinkle * 20 + firefly.lightOffset * 6, 34, 90)
            return hslToRgb(hue, sat, light)
        }

        // Single
        const hue = (base.h + firefly.hueOffset * 20 + Math.sin(time * 0.4 + firefly.phase * 3.7) * 8 + 360) % 360
        const sat = clamp(base.s * 100 + 10 + firefly.satOffset * 6, 42, 100)
        const light = clamp(base.l * 100 + twinkle * 16 + 4, 24, 88)
        return hslToRgb(hue, sat, light)
    }

    function updateFirefly(
        firefly: Firefly, w: number, h: number, time: number, dt: number,
        scene: SceneTuning, wanderMix: number, glowMix: number, speed: number,
        sceneName: string,
    ): void {
        const centerX = w * 0.5 + Math.sin(time * 0.24) * w * 0.2
        const centerY = h * 0.5 + Math.cos(time * 0.19) * h * 0.16

        const dx = centerX - firefly.x
        const dy = centerY - firefly.y
        const distance = Math.max(1, Math.hypot(dx, dy))

        const dirX = dx / distance
        const dirY = dy / distance
        const swirlX = -dirY
        const swirlY = dirX

        const jitterA = Math.sin(time * 1.7 + firefly.phase * 8.3 + firefly.x * 0.03)
        const jitterB = Math.cos(time * 1.33 + firefly.phase * 6.1 + firefly.y * 0.027)
        const wanderForce = (0.02 + wanderMix * 0.12) * scene.wander * firefly.wanderBias

        firefly.vx += (jitterA * 0.8 + jitterB * 0.5) * wanderForce * dt
        firefly.vy += (jitterB * 0.9 - jitterA * 0.45) * wanderForce * dt

        const centerPull = scene.cohesion * (0.85 + (distance / Math.max(w, h)) * 0.3)
        firefly.vx += dirX * centerPull * dt
        firefly.vy += dirY * centerPull * dt

        const swirlForce = scene.swirl * (0.035 + glowMix * 0.02)
        firefly.vx += swirlX * swirlForce * dt
        firefly.vy += swirlY * swirlForce * dt

        if (sceneName === 'Pulse') {
            const pulse = Math.sin(time * 4.1 + firefly.phase * 7.2)
            const pulseForce = scene.pulse * (0.11 + firefly.speedBias * 0.05)
            firefly.vx += dirX * pulse * pulseForce * dt
            firefly.vy += dirY * pulse * pulseForce * dt
        }

        firefly.vx *= 0.93
        firefly.vy *= 0.93

        const speedScale = speed * scene.speed * (0.55 + firefly.speedBias * 0.6)
        const maxVelocity = Math.max(0.75, speedScale * 1.5)
        const velocity = Math.hypot(firefly.vx, firefly.vy)

        if (velocity > maxVelocity) {
            const scale = maxVelocity / velocity
            firefly.vx *= scale
            firefly.vy *= scale
        }

        firefly.x += firefly.vx * speedScale * dt
        firefly.y += firefly.vy * speedScale * dt

        if (wrapFirefly(firefly, w, h)) {
            firefly.trail = [{ x: firefly.x, y: firefly.y }]
            return
        }

        firefly.trail.unshift({ x: firefly.x, y: firefly.y })
        const trailLength = Math.max(6, Math.floor(scene.trail + glowMix * 5))
        if (firefly.trail.length > trailLength) {
            firefly.trail.length = trailLength
        }
    }

    function drawTrail(
        ctx: CanvasRenderingContext2D, firefly: Firefly, color: RGB,
        radius: number, glowMix: number, scene: SceneTuning, twinkle: number,
    ): void {
        const trail = firefly.trail
        if (trail.length < 2) return

        for (let i = 1; i < trail.length; i++) {
            const head = trail[i - 1]
            const tail = trail[i]
            const depth = 1 - i / trail.length
            const alpha = depth * depth * (0.07 + glowMix * 0.2) * (0.65 + twinkle * 0.7)
            const width = Math.max(0.45, radius * (0.28 + depth * (0.9 + scene.pulse * 0.16)))

            ctx.strokeStyle = rgba(color, alpha)
            ctx.lineWidth = width
            ctx.beginPath()
            ctx.moveTo(head.x, head.y)
            ctx.lineTo(tail.x, tail.y)
            ctx.stroke()
        }
    }

    function drawBody(
        ctx: CanvasRenderingContext2D, firefly: Firefly, color: RGB,
        radius: number, glowMix: number, twinkle: number,
    ): void {
        const haloRadius = radius * (1.7 + glowMix * 4.3)
        if (glowMix > 0.01) {
            const glow = ctx.createRadialGradient(firefly.x, firefly.y, 0, firefly.x, firefly.y, haloRadius)
            glow.addColorStop(0, rgba(color, (0.2 + glowMix * 0.4) * (0.55 + twinkle * 0.75)))
            glow.addColorStop(0.6, rgba(color, 0.08 + glowMix * 0.12))
            glow.addColorStop(1, rgba(color, 0))
            ctx.fillStyle = glow
            ctx.beginPath()
            ctx.arc(firefly.x, firefly.y, haloRadius, 0, Math.PI * 2)
            ctx.fill()
        }

        ctx.fillStyle = rgba(color, 0.82 + twinkle * 0.18)
        ctx.beginPath()
        ctx.arc(firefly.x, firefly.y, radius, 0, Math.PI * 2)
        ctx.fill()

        ctx.fillStyle = `rgba(255, 255, 236, ${Math.min(1, 0.24 + twinkle * 0.55).toFixed(3)})`
        ctx.beginPath()
        ctx.arc(firefly.x - radius * 0.24, firefly.y - radius * 0.28, radius * 0.38, 0, Math.PI * 2)
        ctx.fill()
    }

    function drawAtmosphere(
        ctx: CanvasRenderingContext2D, w: number, h: number,
        time: number, scene: SceneTuning, glowMix: number,
    ): void {
        const hueShift = Math.sin(time * 0.08) * 6
        const mistColor = hslToRgb((cachedBaseHsl.h + 12 + hueShift + 360) % 360, 82, 48)

        const mist = ctx.createRadialGradient(
            w * (0.5 + Math.sin(time * 0.04) * 0.08),
            h * (0.55 + Math.cos(time * 0.05) * 0.08),
            0, w * 0.5, h * 0.5, Math.max(w, h) * 0.9,
        )
        mist.addColorStop(0, rgba(mistColor, 0.08 + glowMix * 0.18))
        mist.addColorStop(0.58, rgba(mistColor, 0.03 + glowMix * 0.06))
        mist.addColorStop(1, rgba(mistColor, 0))
        ctx.fillStyle = mist
        ctx.fillRect(0, 0, w, h)

        const pulse = 0.035 + scene.pulse * 0.018 * (0.5 + 0.5 * Math.sin(time * 2.2))
        const veil = hslToRgb((cachedBaseHsl.h + 300) % 360, 70, 38)
        const gradient = ctx.createLinearGradient(0, 0, 0, h)
        gradient.addColorStop(0, rgba(veil, pulse))
        gradient.addColorStop(1, rgba(veil, 0))
        ctx.fillStyle = gradient
        ctx.fillRect(0, 0, w, h)

        const vignette = ctx.createRadialGradient(w * 0.5, h * 0.5, 20, w * 0.5, h * 0.5, Math.max(w, h) * 0.8)
        vignette.addColorStop(0, 'rgba(0, 0, 0, 0)')
        vignette.addColorStop(1, 'rgba(0, 0, 0, 0.42)')
        ctx.fillStyle = vignette
        ctx.fillRect(0, 0, w, h)
    }

    return (ctx, time, c) => {
        const sceneName = c.scene as string
        const colorMode = c.colorMode as string
        const baseColor = c.baseColor as string
        const count = Math.floor(c.count as number)
        const size = c.size as number
        const speed = c.speed as number
        const wander = c.wander as number
        const glow = c.glow as number
        const bgColor = c.bgColor as string
        const w = ctx.canvas.width
        const h = ctx.canvas.height
        const dt = lastTime < 0 ? 1 : clamp((time - lastTime) * 60, 0.55, 2.4)
        lastTime = time

        const scene = SCENE_TUNING[sceneName] ?? SCENE_TUNING.Calm
        const glowMix = clamp(glow / 100, 0, 1)
        const wanderMix = clamp(wander / 100, 0, 1)
        const baseSize = clamp(size, 1, 10)

        updateBaseColorCache(baseColor)
        ensureFireflyCount(count, w, h)

        // Clear with background
        ctx.fillStyle = bgColor
        ctx.fillRect(0, 0, w, h)

        drawAtmosphere(ctx, w, h, time, scene, glowMix)

        ctx.save()
        ctx.globalCompositeOperation = 'lighter'
        ctx.lineCap = 'round'
        ctx.lineJoin = 'round'

        const particleCount = fireflies.length

        for (let i = 0; i < particleCount; i++) {
            const firefly = fireflies[i]
            updateFirefly(firefly, w, h, time, dt, scene, wanderMix, glowMix, speed, sceneName)

            const twinkle = getTwinkle(firefly, time, scene)
            const color = resolveColor(firefly, i, time, twinkle, particleCount, colorMode)
            const radius = Math.max(0.8, baseSize * (0.42 + twinkle * 0.58) * firefly.sizeJitter)

            drawTrail(ctx, firefly, color, radius, glowMix, scene, twinkle)
            drawBody(ctx, firefly, color, radius, glowMix, twinkle)
        }

        ctx.restore()
    }
}, {
    description: 'Poison-glow firefly garden with crisp trails and scene moods',
})
