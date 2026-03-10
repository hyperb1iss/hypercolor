import { canvas } from '@hypercolor/sdk'

interface Firefly {
    x: number
    y: number
    vx: number
    vy: number
    life: number
    maxLife: number
    phase: number
    hueOffset: number
    satOffset: number
    lightOffset: number
    sizeJitter: number
    driftBias: number
}

interface RGB {
    r: number
    g: number
    b: number
}

interface HSL {
    h: number
    s: number
    l: number
}

interface SceneTuning {
    speed: number
    drift: number
    twinkle: number
}

const SCENES = ['Calm', 'Swarm', 'Pulse']
const COLOR_MODES = ['Single', 'Mint Orchid', 'Twilight', 'SilkCircuit', 'Ember', 'Random', 'Rainbow']

const TAU = Math.PI * 2

const PALETTE_STOPS: Record<string, RGB[]> = {
    Ember: [
        { b: 94, g: 255, r: 132 },
        { b: 71, g: 179, r: 255 },
        { b: 193, g: 106, r: 255 },
    ],
    'Mint Orchid': [
        { b: 94, g: 255, r: 132 },
        { b: 234, g: 255, r: 128 },
        { b: 193, g: 106, r: 255 },
        { b: 255, g: 73, r: 125 },
    ],
    SilkCircuit: [
        { b: 234, g: 255, r: 128 },
        { b: 255, g: 53, r: 225 },
        { b: 193, g: 106, r: 255 },
        { b: 255, g: 73, r: 125 },
    ],
    Twilight: [
        { b: 234, g: 255, r: 128 },
        { b: 255, g: 201, r: 93 },
        { b: 255, g: 73, r: 125 },
        { b: 193, g: 106, r: 255 },
    ],
}

const SCENE_TUNING: Record<string, SceneTuning> = {
    Calm: { drift: 0.38, speed: 0.84, twinkle: 0.56 },
    Pulse: { drift: 0.52, speed: 0.98, twinkle: 1.12 },
    Swarm: { drift: 0.62, speed: 1.08, twinkle: 0.84 },
}

function ledSafeHue(hue: number): number {
    const wrapped = ((hue % 360) + 360) % 360
    if (wrapped >= 30 && wrapped < 90) {
        return wrapped < 60 ? 24 : 120
    }
    return wrapped
}

function clamp(value: number, min: number, max: number): number {
    if (Number.isNaN(value)) return min
    return Math.max(min, Math.min(max, value))
}

function rgba(color: RGB, alpha: number): string {
    return `rgba(${color.r}, ${color.g}, ${color.b}, ${clamp(alpha, 0, 1)})`
}

function mixRgb(a: RGB, b: RGB, amount: number): RGB {
    const t = clamp(amount, 0, 1)
    return {
        b: Math.round(a.b + (b.b - a.b) * t),
        g: Math.round(a.g + (b.g - a.g) * t),
        r: Math.round(a.r + (b.r - a.r) * t),
    }
}

function hexToRgb(hex: string): RGB {
    const normalized = hex.replace('#', '')
    const full =
        normalized.length === 3
            ? `${normalized[0]}${normalized[0]}${normalized[1]}${normalized[1]}${normalized[2]}${normalized[2]}`
            : normalized
    const parsed = parseInt(full, 16)
    if (Number.isNaN(parsed)) return { b: 94, g: 255, r: 132 }
    return { b: parsed & 255, g: (parsed >> 8) & 255, r: (parsed >> 16) & 255 }
}

function rgbToHsl(color: RGB): HSL {
    const r = color.r / 255
    const g = color.g / 255
    const b = color.b / 255
    const max = Math.max(r, g, b)
    const min = Math.min(r, g, b)
    const delta = max - min
    const l = (max + min) / 2

    if (delta === 0) return { h: 0, l, s: 0 }

    const s = l > 0.5 ? delta / (2 - max - min) : delta / (max + min)
    let h = 0
    if (max === r) h = (g - b) / delta + (g < b ? 6 : 0)
    else if (max === g) h = (b - r) / delta + 2
    else h = (r - g) / delta + 4

    return { h: h * 60, l, s }
}

function hslToRgb(h: number, sPercent: number, lPercent: number): RGB {
    const s = clamp(sPercent, 0, 100) / 100
    const l = clamp(lPercent, 0, 100) / 100
    const c = (1 - Math.abs(2 * l - 1)) * s
    const hPrime = (((h % 360) + 360) % 360) / 60
    const x = c * (1 - Math.abs((hPrime % 2) - 1))

    let r = 0
    let g = 0
    let b = 0
    if (hPrime < 1) [r, g, b] = [c, x, 0]
    else if (hPrime < 2) [r, g, b] = [x, c, 0]
    else if (hPrime < 3) [r, g, b] = [0, c, x]
    else if (hPrime < 4) [r, g, b] = [0, x, c]
    else if (hPrime < 5) [r, g, b] = [x, 0, c]
    else [r, g, b] = [c, 0, x]

    const m = l - c / 2
    return {
        b: Math.round((b + m) * 255),
        g: Math.round((g + m) * 255),
        r: Math.round((r + m) * 255),
    }
}

function fract(value: number): number {
    return value - Math.floor(value)
}

function samplePalette(stops: RGB[], t: number): RGB {
    if (stops.length === 0) return { b: 255, g: 255, r: 255 }
    if (stops.length === 1) return stops[0]

    const phase = clamp(fract(t), 0, 0.9999)
    const scaled = phase * (stops.length - 1)
    const index = Math.floor(scaled)
    const localT = scaled - index

    return mixRgb(stops[index], stops[index + 1], localT)
}

function spawnFirefly(w: number, h: number): Firefly {
    const x = Math.random() * w
    const y = Math.random() * h
    const angle = Math.random() * TAU
    const speed = 0.08 + Math.random() * 0.36

    return {
        driftBias: 0.76 + Math.random() * 0.52,
        hueOffset: Math.random() * 2 - 1,
        life: 60 + Math.random() * 120,
        lightOffset: Math.random() * 2 - 1,
        maxLife: 60 + Math.random() * 120,
        phase: Math.random() * TAU,
        satOffset: Math.random() * 2 - 1,
        sizeJitter: 0.78 + Math.random() * 0.48,
        vx: Math.cos(angle) * speed,
        vy: Math.sin(angle) * speed,
        x,
        y,
    }
}

function resetFirefly(firefly: Firefly, w: number, h: number): void {
    const next = spawnFirefly(w, h)
    Object.assign(firefly, next)
}

export default canvas.stateful(
    'Fiberflies',
    {
        baseColor: '#84ff5e',
        bgColor: '#14071f',
        colorMode: COLOR_MODES,
        count: [8, 80, 30],
        glow: [0, 100, 56],
        scene: SCENES,
        size: [1, 8, 2],
        speed: [1, 10, 5],
        wander: [0, 100, 42],
    },
    () => {
        const fireflies: Firefly[] = []
        let cachedBaseColor = '#84ff5e'
        let cachedBaseHsl: HSL = { h: 106, l: 0.68, s: 1 }
        let lastTime = -1

        function updateBaseColorCache(color: string): void {
            if (color === cachedBaseColor) return
            cachedBaseColor = color
            cachedBaseHsl = rgbToHsl(hexToRgb(color))
        }

        function ensureFireflyCount(count: number, w: number, h: number): void {
            const target = Math.max(8, Math.min(80, count))
            while (fireflies.length < target) fireflies.push(spawnFirefly(w, h))
            if (fireflies.length > target) fireflies.length = target
        }

        function resolveColor(
            firefly: Firefly,
            index: number,
            time: number,
            brightness: number,
            count: number,
            colorMode: string,
        ): RGB {
            const base = cachedBaseHsl

            if (colorMode in PALETTE_STOPS) {
                const palette = PALETTE_STOPS[colorMode]
                const palettePhase = time * 0.026 + index * 0.021 + firefly.phase / TAU + firefly.hueOffset * 0.02
                const sampled = samplePalette(palette, palettePhase)
                const lifted = rgbToHsl(sampled)
                const lightLift = clamp(brightness * 16 + firefly.lightOffset * 4, -4, 18)

                return hslToRgb(lifted.h, clamp(lifted.s * 100 + 6, 64, 100), clamp(lifted.l * 100 + lightLift, 18, 72))
            }

            if (colorMode === 'Random') {
                const hue = ledSafeHue(base.h + firefly.hueOffset * 160)
                const sat = clamp(84 + firefly.satOffset * 14, 58, 100)
                const light = clamp(24 + brightness * 16 + firefly.lightOffset * 5, 18, 62)
                return hslToRgb(hue, sat, light)
            }

            if (colorMode === 'Rainbow') {
                const hue = ledSafeHue(time * 22 + index * (360 / Math.max(count, 1)) + firefly.hueOffset * 24)
                const sat = clamp(92 + firefly.satOffset * 8, 70, 100)
                const light = clamp(26 + brightness * 14 + firefly.lightOffset * 3, 20, 64)
                return hslToRgb(hue, sat, light)
            }

            const hue = ledSafeHue(base.h + firefly.hueOffset * 8)
            const sat = clamp(base.s * 100 + 12 + firefly.satOffset * 4, 52, 100)
            const light = clamp(base.l * 82 + brightness * 14 + firefly.lightOffset * 3, 20, 66)
            return hslToRgb(hue, sat, light)
        }

        function updateFirefly(
            firefly: Firefly,
            w: number,
            h: number,
            time: number,
            dt: number,
            scene: SceneTuning,
            speed: number,
            wanderMix: number,
            sceneName: string,
        ): void {
            const speedScale = (0.3 + speed * 0.075) * scene.speed
            const driftScale = (0.08 + wanderMix * 0.36) * scene.drift * firefly.driftBias

            const breezeX =
                Math.sin(time * 0.7 + firefly.phase * 2.1 + firefly.y * 0.018) * driftScale +
                Math.sin(time * 0.18 + firefly.phase * 5.3) * driftScale * 0.45
            const breezeY =
                Math.cos(time * 0.6 + firefly.phase * 1.7 + firefly.x * 0.014) * driftScale * 0.84 +
                Math.cos(time * 0.22 + firefly.phase * 4.8) * driftScale * 0.36

            const pulseLift = sceneName === 'Pulse' ? Math.sin(time * 3.0 + firefly.phase * 4.4) * 0.16 : 0
            const swarmNudge = sceneName === 'Swarm' ? Math.sin(time * 0.9 + firefly.phase * 3.4) * 0.12 : 0

            firefly.x += (firefly.vx * speedScale + breezeX + swarmNudge) * dt * 5.4
            firefly.y += (firefly.vy * speedScale + breezeY - pulseLift) * dt * 5.4
            firefly.life -= dt * (0.62 + speed * 0.13)

            const outOfBounds = firefly.x < -18 || firefly.x > w + 18 || firefly.y < -18 || firefly.y > h + 18

            if (firefly.life <= 0 || outOfBounds) resetFirefly(firefly, w, h)
        }

        function drawBody(
            ctx: CanvasRenderingContext2D,
            firefly: Firefly,
            color: RGB,
            radius: number,
            brightness: number,
            glowMix: number,
        ): void {
            const haloColor = mixRgb(color, { b: 234, g: 255, r: 128 }, 0.1)
            const coreColor = mixRgb(color, { b: 255, g: 53, r: 225 }, 0.12)
            const haloRadius = radius * (2.5 + glowMix * 2.1)

            ctx.fillStyle = rgba(haloColor, (0.08 + glowMix * 0.22) * brightness)
            ctx.beginPath()
            ctx.arc(firefly.x, firefly.y, haloRadius, 0, TAU)
            ctx.fill()

            ctx.fillStyle = rgba(color, 0.52 + brightness * 0.4)
            ctx.beginPath()
            ctx.arc(firefly.x, firefly.y, radius * 1.3, 0, TAU)
            ctx.fill()

            ctx.fillStyle = rgba(coreColor, 0.44 + brightness * 0.2)
            ctx.beginPath()
            ctx.arc(firefly.x, firefly.y, Math.max(0.65, radius * 0.55), 0, TAU)
            ctx.fill()
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
            const dt = lastTime < 0 ? 1 : clamp((time - lastTime) * 60, 0.6, 1.8)
            lastTime = time

            const scene = SCENE_TUNING[sceneName] ?? SCENE_TUNING.Calm
            const glowMix = clamp(glow / 100, 0, 1)
            const wanderMix = clamp(wander / 100, 0, 1)
            const baseSize = clamp(size, 1, 8)

            updateBaseColorCache(baseColor)
            ensureFireflyCount(count, w, h)

            ctx.fillStyle = bgColor
            ctx.fillRect(0, 0, w, h)

            ctx.save()
            ctx.globalCompositeOperation = 'screen'

            const particleCount = fireflies.length
            for (let i = 0; i < particleCount; i++) {
                const firefly = fireflies[i]
                updateFirefly(firefly, w, h, time, dt, scene, speed, wanderMix, sceneName)

                const lifeMix = clamp(firefly.life / firefly.maxLife, 0, 1)
                const lifeFade = Math.sin(lifeMix * Math.PI)
                const twinkle = 0.5 + 0.5 * Math.sin(time * (1.2 + scene.twinkle) + firefly.phase * 5.2)
                const brightness = clamp(0.16 + lifeFade * 0.62 + twinkle * 0.22, 0.08, 1)
                const color = resolveColor(firefly, i, time, brightness, particleCount, colorMode)
                const radius = Math.max(0.8, baseSize * (0.72 + brightness * 0.48) * firefly.sizeJitter)

                drawBody(ctx, firefly, color, radius, brightness, glowMix)
            }

            ctx.restore()
        }
    },
    {
        description: 'Fiberflies with neon palette modes, soft glow, and gentle drift',
    },
)
