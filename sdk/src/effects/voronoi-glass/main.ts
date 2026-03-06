import { canvas, combo } from '@hypercolor/sdk'

// ── Types ────────────────────────────────────────────────────────────────

interface ParticleSeed {
    orbitOffset: number
    radialOffset: number
    speedBias: number
    sizeBias: number
    jitter: number
    twinkle: number
}

interface RGB { r: number; g: number; b: number }

// ── Constants ────────────────────────────────────────────────────────────

const ROTATION_MODES = ['Clockwise', 'Counter-Clockwise', 'Alternating', 'Pulse']
const COLOR_MODES = ['RGB Split', 'RGB Cycle', 'Prism', 'Mono']
const DEFAULT_BACKGROUND = '#04060f'
const SAFE_HUES = [190, 220, 250, 285, 320, 18]

// ── Helpers ──────────────────────────────────────────────────────────────

function clamp(value: number, min: number, max: number): number {
    return Math.max(min, Math.min(max, value))
}

function fract(value: number): number {
    return value - Math.floor(value)
}

function hash(value: number): number {
    const x = Math.sin(value * 127.1) * 43758.5453123
    return x - Math.floor(x)
}

function toRgba(color: RGB, alpha: number): string {
    return `rgba(${color.r},${color.g},${color.b},${clamp(alpha, 0, 1).toFixed(3)})`
}

function hslToRgb(h: number, s: number, l: number): RGB {
    const hue = ((h % 360) + 360) % 360
    const sat = clamp(s / 100, 0, 1)
    const light = clamp(l / 100, 0, 1)
    const c = (1 - Math.abs(2 * light - 1)) * sat
    const x = c * (1 - Math.abs(((hue / 60) % 2) - 1))
    const m = light - c / 2

    let r = 0, g = 0, b = 0
    if (hue < 60) { r = c; g = x }
    else if (hue < 120) { r = x; g = c }
    else if (hue < 180) { g = c; b = x }
    else if (hue < 240) { g = x; b = c }
    else if (hue < 300) { r = x; b = c }
    else { r = c; b = x }

    return {
        r: Math.round((r + m) * 255),
        g: Math.round((g + m) * 255),
        b: Math.round((b + m) * 255),
    }
}

function ledSafeHue(hue: number): number {
    const wrapped = ((hue % 360) + 360) % 360
    if (wrapped >= 30 && wrapped < 90) {
        return wrapped < 60 ? 24 : 120
    }
    return wrapped
}

function safeBandHue(value: number): number {
    const wrapped = ((value % 1) + 1) % 1
    const scaled = wrapped * SAFE_HUES.length
    const index = Math.floor(scaled) % SAFE_HUES.length
    const next = (index + 1) % SAFE_HUES.length
    const blend = scaled - Math.floor(scaled)
    return SAFE_HUES[index] + (SAFE_HUES[next] - SAFE_HUES[index]) * blend
}

function scaleColor(color: RGB, scale: number): RGB {
    return {
        r: clamp(Math.round(color.r * scale), 0, 255),
        g: clamp(Math.round(color.g * scale), 0, 255),
        b: clamp(Math.round(color.b * scale), 0, 255),
    }
}

function createSeed(index: number): ParticleSeed {
    const i = index + 1
    return {
        orbitOffset: hash(i * 1.137 + 0.29),
        radialOffset: hash(i * 2.413 + 3.18),
        speedBias: hash(i * 3.977 + 8.24),
        sizeBias: hash(i * 5.331 + 1.77),
        jitter: hash(i * 7.739 + 5.65),
        twinkle: hash(i * 11.129 + 9.92),
    }
}

function resolveDirection(mode: string, arm: number, time: number): number {
    switch (mode) {
        case 'Clockwise': return 1
        case 'Counter-Clockwise': return -1
        case 'Pulse': return Math.sin(time * 1.25 + arm * 0.75) >= 0 ? 1 : -1
        case 'Alternating':
        default: return arm % 2 === 0 ? 1 : -1
    }
}

function resolveColor(
    mode: string, arm: number, index: number, arms: number,
    time: number, life: number, cycleRate: number,
): RGB {
    const rgbArmPalette: RGB[] = [
        { r: 255, g: 74, b: 86 },
        { r: 78, g: 255, b: 155 },
        { r: 82, g: 146, b: 255 },
    ]

    if (mode === 'RGB Split') {
        const base = rgbArmPalette[arm % rgbArmPalette.length]
        const boost = 0.78 + 0.22 * Math.sin(time * (2.6 + cycleRate * 4.2) + life * Math.PI * 2)
        return scaleColor(base, boost)
    }

    if (mode === 'RGB Cycle') {
        const hue = safeBandHue(time * (0.05 + cycleRate * 0.16) + arm / Math.max(arms, 1) + life * 0.18)
        return hslToRgb(ledSafeHue(hue), 94, 56)
    }

    if (mode === 'Prism') {
        const hue = safeBandHue(index * 0.173 + time * (0.03 + cycleRate * 0.08) + life * 0.22)
        return hslToRgb(ledSafeHue(hue), 92, 54)
    }

    // Mono
    const monoHue = 208 + Math.sin(time * (2.2 + cycleRate * 3.8) + arm) * 22
    return hslToRgb(monoHue, 84, 58)
}

// ── Effect ───────────────────────────────────────────────────────────────

export default canvas.stateful('Voronoi Glass', {
    arms:         [1, 10, 5],
    count:        [16, 220, 110],
    particleSize: [1, 14, 5],
    growth:       [0, 100, 62],
    rotationMode: combo('Rotation Mode', ROTATION_MODES, { default: 'Alternating' }),
    colorMode:    combo('Color Mode', COLOR_MODES, { default: 'RGB Split' }),
    cycleSpeed:   [0, 100, 48],
    background:   DEFAULT_BACKGROUND,
}, () => {
    let particles: ParticleSeed[] = []
    let particleCount = 0

    function ensureParticleCount(count: number): void {
        const target = clamp(Math.round(count), 16, 220)
        if (target === particleCount && particles.length === target) return

        if (target > particles.length) {
            for (let i = particles.length; i < target; i++) {
                particles.push(createSeed(i))
            }
        } else {
            particles.length = target
        }

        particleCount = target
    }

    function drawBacklight(
        ctx: CanvasRenderingContext2D, width: number, height: number,
        time: number, cycleRate: number,
    ): void {
        const centerX = width * (0.5 + Math.sin(time * 0.16) * 0.04)
        const centerY = height * (0.5 + Math.cos(time * 0.18) * 0.04)
        const radius = Math.max(width, height) * 0.84
        const hue = (210 + time * (16 + cycleRate * 70)) % 360
        const base = hslToRgb(hue, 82, 53)
        const accent = hslToRgb(ledSafeHue(hue + 130), 88, 54)

        const glow = ctx.createRadialGradient(centerX, centerY, 0, centerX, centerY, radius)
        glow.addColorStop(0, toRgba(base, 0.11))
        glow.addColorStop(0.52, toRgba(accent, 0.05))
        glow.addColorStop(1, 'rgba(0,0,0,0)')
        ctx.fillStyle = glow
        ctx.fillRect(0, 0, width, height)
    }

    function drawCore(
        ctx: CanvasRenderingContext2D, cx: number, cy: number,
        coreRadius: number, time: number, cycleRate: number,
    ): void {
        const hue = (time * (40 + cycleRate * 90) + 6) % 360
        const coreA = hslToRgb(ledSafeHue(hue), 94, 56)
        const coreB = hslToRgb(ledSafeHue(hue + 150), 92, 52)
        const pulse = 1 + Math.sin(time * (4.2 + cycleRate * 4.8)) * 0.16
        const radius = coreRadius * pulse

        const coreGradient = ctx.createRadialGradient(cx, cy, 0, cx, cy, radius * 3.1)
        coreGradient.addColorStop(0, toRgba(coreA, 0.72))
        coreGradient.addColorStop(0.35, toRgba(coreB, 0.38))
        coreGradient.addColorStop(1, 'rgba(0,0,0,0)')
        ctx.fillStyle = coreGradient
        ctx.beginPath()
        ctx.arc(cx, cy, radius * 3.1, 0, Math.PI * 2)
        ctx.fill()

        ctx.strokeStyle = toRgba(coreB, 0.34)
        ctx.lineWidth = 1.2
        ctx.beginPath()
        ctx.arc(cx, cy, radius * 1.8, 0, Math.PI * 2)
        ctx.stroke()
    }

    return (ctx, time, c) => {
        const arms = Math.max(1, Math.round(c.arms as number))
        const count = Math.round(c.count as number)
        const particleSizeCtrl = c.particleSize as number
        const growth = c.growth as number
        const rotationMode = c.rotationMode as string
        const colorMode = c.colorMode as string
        const cycleSpeed = c.cycleSpeed as number
        const background = c.background as string
        const w = ctx.canvas.width
        const h = ctx.canvas.height
        const cx = w * 0.5
        const cy = h * 0.5
        const minDim = Math.min(w, h)

        ensureParticleCount(count)
        if (particles.length === 0) return

        const growthMix = growth / 100
        const cycleRate = cycleSpeed / 100
        const rotationVelocity = 0.4 + cycleRate * 2.2
        const spawnVelocity = 0.65 + cycleRate * 2.1 + particles.length / 220
        const maxRadius = minDim * (0.18 + growthMix * 0.58)
        const coreRadius = minDim * (0.036 + growthMix * 0.02)
        const laneTwist = 2.1 + growthMix * 5.2

        // Clear with background
        ctx.fillStyle = background
        ctx.fillRect(0, 0, w, h)

        drawBacklight(ctx, w, h, time, cycleRate)
        drawCore(ctx, cx, cy, coreRadius, time, cycleRate)

        ctx.save()
        ctx.globalCompositeOperation = 'lighter'

        for (let i = 0; i < particles.length; i++) {
            const seed = particles[i]
            const arm = i % arms
            const direction = resolveDirection(rotationMode, arm, time)
            const life = fract(time * spawnVelocity * (0.55 + seed.speedBias) + seed.orbitOffset)
            const radialCurve = Math.pow(life, 0.36 + (1 - growthMix) * 0.92)
            const radius = coreRadius + radialCurve * maxRadius + (seed.radialOffset - 0.5) * minDim * 0.045

            const laneBase = (arm / arms) * Math.PI * 2
            const orbital = direction * (time * rotationVelocity + life * Math.PI * 2 * laneTwist)
            const wobble = Math.sin(time * (1.6 + seed.speedBias * 1.9) + seed.twinkle * 9.1) * seed.jitter * 0.42
            const angle = laneBase + orbital + wobble

            const x = cx + Math.cos(angle) * radius
            const y = cy + Math.sin(angle) * radius * (0.9 + seed.jitter * 0.08)
            const color = resolveColor(colorMode, arm, i, arms, time, life, cycleRate)

            const pulse = 0.62 + 0.38 * Math.sin(time * (4.5 + seed.speedBias * 3.1) + seed.twinkle * 11.0)
            const size = Math.max(0.7, particleSizeCtrl * (0.5 + seed.sizeBias * 1.05) * (0.65 + radialCurve * 0.8))
            const alpha = clamp((0.24 + 0.9 * Math.sin(Math.PI * life)) * pulse, 0.08, 1)

            ctx.fillStyle = toRgba(color, 0.16 * alpha)
            ctx.beginPath()
            ctx.arc(x, y, size * 2.2, 0, Math.PI * 2)
            ctx.fill()

            ctx.fillStyle = toRgba(color, 0.78 * alpha)
            ctx.beginPath()
            ctx.arc(x, y, size, 0, Math.PI * 2)
            ctx.fill()

            ctx.fillStyle = toRgba(scaleColor(color, 1.08), 0.18 * alpha)
            ctx.beginPath()
            ctx.arc(x, y, Math.max(0.55, size * 0.28), 0, Math.PI * 2)
            ctx.fill()
        }

        ctx.restore()
    }
}, {
    description: 'Community Swirl Reactor style orbital particles with crisp RGB trails',
})
