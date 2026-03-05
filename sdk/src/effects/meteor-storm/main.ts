import { canvas } from '@hypercolor/sdk'

// ── Types ────────────────────────────────────────────────────────────────

interface Star {
    x: number
    y: number
    size: number
    phase: number
    depth: number
}

interface Meteor {
    x: number
    y: number
    vx: number
    vy: number
    size: number
    trail: number
    phase: number
    brightness: number
}

interface Rgb { r: number; g: number; b: number }

interface SceneTone {
    skyTop: Rgb
    skyBottom: Rgb
    star: Rgb
    trail: Rgb
}

// ── Constants ────────────────────────────────────────────────────────────

const PATHS = ['Diagonal', 'Vertical']
const SCENES = ['Night', 'Pastel']

// ── Helpers ──────────────────────────────────────────────────────────────

function hexToRgb(hex: string): Rgb {
    const normalized = hex.trim().replace('#', '')
    const expanded = normalized.length === 3
        ? normalized.split('').map((char) => `${char}${char}`).join('')
        : normalized
    if (!/^[0-9a-fA-F]{6}$/.test(expanded)) return { r: 255, g: 255, b: 255 }
    const value = Number.parseInt(expanded, 16)
    return { r: (value >> 16) & 255, g: (value >> 8) & 255, b: value & 255 }
}

function mixRgb(a: Rgb, b: Rgb, t: number): Rgb {
    const ratio = Math.max(0, Math.min(1, t))
    return {
        r: Math.round(a.r + (b.r - a.r) * ratio),
        g: Math.round(a.g + (b.g - a.g) * ratio),
        b: Math.round(a.b + (b.b - a.b) * ratio),
    }
}

function rgbToCss(color: Rgb): string {
    return `rgb(${color.r}, ${color.g}, ${color.b})`
}

function rgbToRgba(color: Rgb, alpha: number): string {
    return `rgba(${color.r}, ${color.g}, ${color.b}, ${Math.max(0, Math.min(1, alpha))})`
}

function getSceneTone(path: string, skyTop: string, skyBottom: string, starColor: string, scene: string): SceneTone {
    const top = hexToRgb(skyTop)
    const bottom = hexToRgb(skyBottom)
    const star = hexToRgb(starColor)

    if (scene === 'Pastel') {
        return {
            skyTop: mixRgb(top, { r: 255, g: 227, b: 246 }, 0.24),
            skyBottom: mixRgb(bottom, { r: 210, g: 230, b: 255 }, 0.2),
            star: mixRgb(star, { r: 255, g: 255, b: 255 }, 0.24),
            trail: mixRgb(star, { r: 255, g: 214, b: 242 }, 0.34),
        }
    }

    return {
        skyTop: mixRgb(top, { r: 3, g: 6, b: 18 }, 0.32),
        skyBottom: mixRgb(bottom, { r: 15, g: 30, b: 68 }, 0.2),
        star: mixRgb(star, { r: 255, g: 255, b: 255 }, 0.16),
        trail: mixRgb(star, { r: 146, g: 198, b: 255 }, 0.24),
    }
}

function computeBackgroundStarCount(density: number, starSize: number): number {
    return Math.max(36, Math.floor(40 + density * 1.9 - starSize * 0.55))
}

function computeMeteorCap(density: number, starSize: number): number {
    return Math.max(4, Math.floor(3 + density * 0.15 - starSize * 0.05))
}

function computeMeteorSpawnRate(density: number, speed: number): number {
    return 1 + density * 0.05 + speed * 0.8
}

// ── Effect ───────────────────────────────────────────────────────────────

export default canvas.stateful('Meteor Storm', {
    path:      PATHS,
    speed:     [1, 10, 5],
    starSize:  [1, 20, 8],
    density:   [10, 100, 58],
    trail:     [10, 100, 72],
    skyTop:    '#0b1233',
    skyBottom: '#1a3f89',
    starColor: '#fff6bd',
    scene:     SCENES,
}, () => {
    let stars: Star[] = []
    let meteors: Meteor[] = []
    let spawnBudget = 0
    let lastWidth = 0
    let lastHeight = 0
    let lastStarCount = 0
    let lastTime = -1

    let prevPath = 'Diagonal'

    function syncStarfield(w: number, h: number, density: number, starSize: number, force = false): void {
        const targetCount = computeBackgroundStarCount(density, starSize)
        const sizeChanged = lastWidth !== w || lastHeight !== h
        if (!force && !sizeChanged && targetCount === lastStarCount) return

        lastWidth = w
        lastHeight = h
        lastStarCount = targetCount
        stars = []

        for (let i = 0; i < targetCount; i++) {
            stars.push({
                x: Math.random() * w,
                y: Math.random() * h,
                size: 0.35 + Math.random() * 1.4,
                phase: Math.random() * Math.PI * 2,
                depth: 0.25 + Math.random() * 0.95,
            })
        }
    }

    function spawnMeteor(w: number, h: number, speed: number, starSize: number, trailCtrl: number, path: string): Meteor {
        const baseSpeed = 65 + speed * 120
        const size = Math.max(1.2, 0.65 + starSize * 0.16) * (0.72 + Math.random() * 0.65)
        const trail = (12 + trailCtrl * 1.45 + size * 4.5) * (0.82 + Math.random() * 0.36)

        if (path === 'Vertical') {
            const vy = baseSpeed * (0.85 + Math.random() * 0.8)
            const vx = (Math.random() - 0.5) * baseSpeed * 0.09
            return {
                x: Math.random() * (w + 24) - 12,
                y: -trail - Math.random() * h * 0.32,
                vx, vy, size, trail,
                phase: Math.random() * Math.PI * 2,
                brightness: 1,
            }
        }

        const vy = baseSpeed * (0.78 + Math.random() * 0.76)
        const direction = Math.random() < 0.86 ? 1 : -1
        const vx = baseSpeed * (0.3 + Math.random() * 0.34) * direction
        return {
            x: Math.random() * (w + 32) - 16,
            y: -trail - Math.random() * h * 0.42,
            vx, vy, size, trail,
            phase: Math.random() * Math.PI * 2,
            brightness: 1,
        }
    }

    function drawSky(ctx: CanvasRenderingContext2D, w: number, h: number, tone: SceneTone, trailCtrl: number): void {
        const gradient = ctx.createLinearGradient(0, 0, 0, h)
        gradient.addColorStop(0, rgbToCss(tone.skyTop))
        gradient.addColorStop(1, rgbToCss(tone.skyBottom))
        ctx.fillStyle = gradient
        ctx.fillRect(0, 0, w, h)

        const haze = ctx.createLinearGradient(0, h * 0.35, 0, h)
        haze.addColorStop(0, rgbToRgba(tone.trail, 0))
        haze.addColorStop(1, rgbToRgba(tone.trail, 0.04 + trailCtrl * 0.0012))
        ctx.fillStyle = haze
        ctx.fillRect(0, 0, w, h)
    }

    function drawBackgroundStars(
        ctx: CanvasRenderingContext2D, w: number, h: number, time: number,
        tone: SceneTone, speed: number, starSize: number, path: string,
    ): void {
        const drift = 8 + speed * 18
        const driftX = path === 'Diagonal' ? drift * 0.38 : drift * 0.04
        const driftY = path === 'Vertical' ? drift * 0.52 : drift * 0.34
        const sizeScale = 0.6 + starSize * 0.06

        for (const star of stars) {
            const x = (star.x + time * driftX * star.depth) % w
            const y = (star.y + time * driftY * (0.4 + star.depth)) % h
            const twinkle = 0.5 + 0.5 * Math.sin(time * (1.2 + star.depth * 1.6) + star.phase * 2.4)
            const alpha = (0.15 + twinkle * 0.46) * (0.72 + star.depth * 0.28)
            const size = Math.max(1, (star.size + sizeScale * 0.22) * (0.8 + star.depth * 0.28))

            ctx.fillStyle = rgbToRgba(tone.star, alpha * 0.7)
            ctx.fillRect(x, y, size, size)

            if (size > 1.45 && twinkle > 0.76) {
                ctx.fillStyle = rgbToRgba(tone.star, alpha * 0.5)
                const streak = size * 2.2
                ctx.fillRect(x - streak * 0.5, y + size * 0.1, streak, 1)
                ctx.fillRect(x + size * 0.1, y - streak * 0.5, 1, streak)
            }
        }
    }

    function drawMeteors(ctx: CanvasRenderingContext2D, time: number, tone: SceneTone): void {
        const headColor = mixRgb(tone.star, { r: 255, g: 255, b: 255 }, 0.35)

        for (const meteor of meteors) {
            const velocity = Math.hypot(meteor.vx, meteor.vy)
            if (velocity <= 0.0001) continue

            const ux = meteor.vx / velocity
            const uy = meteor.vy / velocity
            const dynamicTrail = meteor.trail * (0.84 + 0.16 * Math.sin(time * 4.6 + meteor.phase))
            const tailX = meteor.x - ux * dynamicTrail
            const tailY = meteor.y - uy * dynamicTrail

            const mainTrail = ctx.createLinearGradient(meteor.x, meteor.y, tailX, tailY)
            mainTrail.addColorStop(0, rgbToRgba(headColor, 0.92 * meteor.brightness))
            mainTrail.addColorStop(0.24, rgbToRgba(tone.star, 0.58 * meteor.brightness))
            mainTrail.addColorStop(1, rgbToRgba(tone.trail, 0))

            ctx.lineCap = 'round'
            ctx.lineWidth = Math.max(1.15, meteor.size * 0.82)
            ctx.strokeStyle = mainTrail
            ctx.beginPath()
            ctx.moveTo(meteor.x, meteor.y)
            ctx.lineTo(tailX, tailY)
            ctx.stroke()

            const bloomTrail = ctx.createLinearGradient(meteor.x, meteor.y, tailX, tailY)
            bloomTrail.addColorStop(0, rgbToRgba(tone.trail, 0.28 * meteor.brightness))
            bloomTrail.addColorStop(1, rgbToRgba(tone.trail, 0))
            ctx.lineWidth = Math.max(2.25, meteor.size * 2.05)
            ctx.strokeStyle = bloomTrail
            ctx.beginPath()
            ctx.moveTo(meteor.x, meteor.y)
            ctx.lineTo(tailX, tailY)
            ctx.stroke()

            const headSize = Math.max(1.5, meteor.size * 0.92)
            ctx.fillStyle = rgbToRgba(headColor, 0.98)
            ctx.fillRect(meteor.x - headSize * 0.5, meteor.y - headSize * 0.5, headSize, headSize)

            const flare = headSize * 1.85
            ctx.fillStyle = rgbToRgba(headColor, 0.56 * meteor.brightness)
            ctx.fillRect(meteor.x - flare * 0.5, meteor.y - 0.5, flare, 1)
            ctx.fillRect(meteor.x - 0.5, meteor.y - flare * 0.5, 1, flare)
        }
    }

    return (ctx, time, c) => {
        const path = c.path as string
        const speed = c.speed as number
        const starSize = c.starSize as number
        const density = c.density as number
        const trailCtrl = c.trail as number
        const skyTop = c.skyTop as string
        const skyBottom = c.skyBottom as string
        const starColor = c.starColor as string
        const scene = c.scene as string
        const w = ctx.canvas.width
        const h = ctx.canvas.height
        const dt = lastTime < 0 ? 1 / 60 : Math.min(0.05, time - lastTime)
        lastTime = time

        // Detect path change — clear meteors
        if (path !== prevPath) {
            meteors = []
            spawnBudget = 0
            prevPath = path
        }

        syncStarfield(w, h, density, starSize)

        const tone = getSceneTone(path, skyTop, skyBottom, starColor, scene)

        // Draw sky (also serves as canvas clear)
        drawSky(ctx, w, h, tone, trailCtrl)
        drawBackgroundStars(ctx, w, h, time, tone, speed, starSize, path)

        // Update meteors
        const maxMeteors = computeMeteorCap(density, starSize)
        const spawnRate = computeMeteorSpawnRate(density, speed)

        spawnBudget += dt * spawnRate
        while (spawnBudget >= 1 && meteors.length < maxMeteors) {
            meteors.push(spawnMeteor(w, h, speed, starSize, trailCtrl, path))
            spawnBudget -= 1
        }

        if (meteors.length < maxMeteors && Math.random() < dt * spawnRate * 0.4) {
            meteors.push(spawnMeteor(w, h, speed, starSize, trailCtrl, path))
        }

        for (const meteor of meteors) {
            meteor.x += meteor.vx * dt
            meteor.y += meteor.vy * dt
            meteor.brightness = 0.72 + 0.28 * Math.sin(meteor.phase + meteor.y * 0.03)
        }

        meteors = meteors.filter(
            (meteor) =>
                meteor.x > -meteor.trail - 45 &&
                meteor.x < w + meteor.trail + 45 &&
                meteor.y < h + meteor.trail + 55,
        )

        drawMeteors(ctx, time, tone)
    }
}, {
    description: 'Crisp falling stars with directional hyperspace trails over a gradient night sky',
})
