import { canvas, color, combo, num } from '@hypercolor/sdk'

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

interface Rgb {
    r: number
    g: number
    b: number
}

interface SceneTone {
    skyTop: Rgb
    skyBottom: Rgb
    star: Rgb
    trail: Rgb
}

// ── Constants ────────────────────────────────────────────────────────────

const PATHS = ['Diagonal', 'Vertical']
const SCENES = ['Aurora', 'Night']

// ── Helpers ──────────────────────────────────────────────────────────────

function hexToRgb(hex: string): Rgb {
    const normalized = hex.trim().replace('#', '')
    const expanded =
        normalized.length === 3
            ? normalized
                  .split('')
                  .map((char) => `${char}${char}`)
                  .join('')
            : normalized
    if (!/^[0-9a-fA-F]{6}$/.test(expanded)) return { b: 255, g: 255, r: 255 }
    const value = Number.parseInt(expanded, 16)
    return { b: value & 255, g: (value >> 8) & 255, r: (value >> 16) & 255 }
}

function mixRgb(a: Rgb, b: Rgb, t: number): Rgb {
    const ratio = Math.max(0, Math.min(1, t))
    return {
        b: Math.round(a.b + (b.b - a.b) * ratio),
        g: Math.round(a.g + (b.g - a.g) * ratio),
        r: Math.round(a.r + (b.r - a.r) * ratio),
    }
}

function rgbToCss(color: Rgb): string {
    return `rgb(${color.r}, ${color.g}, ${color.b})`
}

function rgbToRgba(color: Rgb, alpha: number): string {
    return `rgba(${color.r}, ${color.g}, ${color.b}, ${Math.max(0, Math.min(1, alpha))})`
}

function getSceneTone(_path: string, skyTop: string, skyBottom: string, starColor: string, scene: string): SceneTone {
    const top = hexToRgb(skyTop)
    const bottom = hexToRgb(skyBottom)
    const star = hexToRgb(starColor)

    if (scene === 'Aurora') {
        return {
            skyBottom: mixRgb(bottom, { b: 118, g: 78, r: 22 }, 0.26),
            skyTop: mixRgb(top, { b: 42, g: 14, r: 12 }, 0.34),
            star: mixRgb(star, { b: 255, g: 238, r: 150 }, 0.2),
            trail: mixRgb(star, { b: 190, g: 104, r: 255 }, 0.24),
        }
    }

    return {
        skyBottom: mixRgb(bottom, { b: 68, g: 30, r: 15 }, 0.2),
        skyTop: mixRgb(top, { b: 18, g: 6, r: 3 }, 0.32),
        star: mixRgb(star, { b: 255, g: 224, r: 162 }, 0.14),
        trail: mixRgb(star, { b: 255, g: 210, r: 92 }, 0.2),
    }
}

function computeBackgroundStarCount(density: number, starSize: number): number {
    return Math.max(22, Math.floor(24 + density * 1.1 - starSize * 0.4))
}

function computeMeteorCap(density: number, starSize: number): number {
    return Math.max(4, Math.floor(3 + density * 0.15 - starSize * 0.05))
}

function computeMeteorSpawnRate(density: number, speed: number): number {
    return 1 + density * 0.05 + speed * 0.8
}

// ── Effect ───────────────────────────────────────────────────────────────

export default canvas.stateful(
    'Meteor Storm',
    {
        scene: combo('Scene', SCENES, { group: 'Scene' }),
        path: combo('Path', PATHS, { group: 'Scene' }),
        skyTop: color('Sky Top', '#0b1233', { group: 'Color' }),
        skyBottom: color('Sky Bottom', '#1a3f89', { group: 'Color' }),
        starColor: color('Star Color', '#8dd6ff', { group: 'Color' }),
        speed: num('Speed', [1, 10], 5, { group: 'Motion' }),
        trail: num('Trail', [10, 100], 72, { group: 'Motion' }),
        starSize: num('Star Size', [1, 20], 8, { group: 'Stars' }),
        density: num('Density', [10, 100], 52, { group: 'Stars' }),
    },
    () => {
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
                    depth: 0.25 + Math.random() * 0.95,
                    phase: Math.random() * Math.PI * 2,
                    size: 0.55 + Math.random() * 1.6,
                    x: Math.random() * w,
                    y: Math.random() * h,
                })
            }
        }

        function spawnMeteor(
            w: number,
            h: number,
            speed: number,
            starSize: number,
            trailCtrl: number,
            path: string,
        ): Meteor {
            const baseSpeed = 65 + speed * 120
            const size = Math.max(1.2, 0.65 + starSize * 0.16) * (0.72 + Math.random() * 0.65)
            const trail = (12 + trailCtrl * 1.45 + size * 4.5) * (0.82 + Math.random() * 0.36)

            if (path === 'Vertical') {
                const vy = baseSpeed * (0.85 + Math.random() * 0.8)
                const vx = (Math.random() - 0.5) * baseSpeed * 0.09
                return {
                    brightness: 1,
                    phase: Math.random() * Math.PI * 2,
                    size,
                    trail,
                    vx,
                    vy,
                    x: Math.random() * (w + 24) - 12,
                    y: -trail - Math.random() * h * 0.32,
                }
            }

            const vy = baseSpeed * (0.78 + Math.random() * 0.76)
            const direction = Math.random() < 0.86 ? 1 : -1
            const vx = baseSpeed * (0.3 + Math.random() * 0.34) * direction
            return {
                brightness: 1,
                phase: Math.random() * Math.PI * 2,
                size,
                trail,
                vx,
                vy,
                x: Math.random() * (w + 32) - 16,
                y: -trail - Math.random() * h * 0.42,
            }
        }

        function drawSky(
            ctx: CanvasRenderingContext2D,
            w: number,
            h: number,
            tone: SceneTone,
            trailCtrl: number,
        ): void {
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
            ctx: CanvasRenderingContext2D,
            w: number,
            h: number,
            time: number,
            tone: SceneTone,
            speed: number,
            starSize: number,
            path: string,
        ): void {
            const drift = 8 + speed * 18
            const driftX = path === 'Diagonal' ? drift * 0.38 : drift * 0.04
            const driftY = path === 'Vertical' ? drift * 0.52 : drift * 0.34
            const sizeScale = 0.6 + starSize * 0.06

            for (const star of stars) {
                const x = (star.x + time * driftX * star.depth) % w
                const y = (star.y + time * driftY * (0.4 + star.depth)) % h
                const twinkle = 0.5 + 0.5 * Math.sin(time * (1.2 + star.depth * 1.6) + star.phase * 2.4)
                const alpha = (0.1 + twinkle * 0.34) * (0.7 + star.depth * 0.24)
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
            const headColor = mixRgb(tone.star, tone.trail, 0.34)

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
    },
    {
        description:
            'Blazing meteors tear across a gradient night sky. Directional trails streak and fade as the cosmos rains light.',
        presets: [
            {
                controls: {
                    density: 88,
                    path: 'Diagonal',
                    scene: 'Night',
                    skyBottom: '#0e1e52',
                    skyTop: '#050818',
                    speed: 7,
                    starColor: '#c8e8ff',
                    starSize: 12,
                    trail: 90,
                },
                description:
                    'Peak meteor shower on a clear November night. Fat streaks tear through a blue-black sky over endless prairie.',
                name: 'Leonids Over Montana',
            },
            {
                controls: {
                    density: 45,
                    path: 'Vertical',
                    scene: 'Aurora',
                    skyBottom: '#143868',
                    skyTop: '#0a1030',
                    speed: 3,
                    starColor: '#96eaff',
                    starSize: 16,
                    trail: 85,
                },
                description:
                    'Soft luminous streaks dissolve into a northern lights wash. The sky weeping color over a frozen lake.',
                name: 'Aurora Teardrops',
            },
            {
                controls: {
                    density: 100,
                    path: 'Diagonal',
                    scene: 'Night',
                    skyBottom: '#2a1854',
                    skyTop: '#0f0a1e',
                    speed: 10,
                    starColor: '#e0b8ff',
                    starSize: 3,
                    trail: 40,
                },
                description:
                    'Tiny pinprick stars race above the city glow. Fast, clinical, the sky buzzing like fluorescent tubes.',
                name: 'Tokyo Rooftop 3am',
            },
            {
                controls: {
                    density: 30,
                    path: 'Diagonal',
                    scene: 'Night',
                    skyBottom: '#1a2040',
                    skyTop: '#0d0c14',
                    speed: 4,
                    starColor: '#ffd4a0',
                    starSize: 18,
                    trail: 100,
                },
                description:
                    'Warm amber meteors crawl across a velvet desert sky. Campfire smoke blurs the constellations.',
                name: 'Perseid Campfire',
            },
            {
                controls: {
                    density: 72,
                    path: 'Vertical',
                    scene: 'Night',
                    skyBottom: '#1a0828',
                    skyTop: '#08030f',
                    speed: 9,
                    starColor: '#ff88dd',
                    starSize: 6,
                    trail: 55,
                },
                description:
                    'A dying star sheds its atmosphere in vertical streaks of magenta fire. Gravity pulls the light straight down into oblivion.',
                name: 'Supernova Curtain Call',
            },
            {
                controls: {
                    density: 18,
                    path: 'Diagonal',
                    scene: 'Aurora',
                    skyBottom: '#0c2a3f',
                    skyTop: '#061420',
                    speed: 1,
                    starColor: '#ffe8c0',
                    starSize: 20,
                    trail: 100,
                },
                description:
                    'Ancient light arrives from the edge of the observable universe. Colossal golden bolides drift through aurora fog in geological slow motion.',
                name: 'Deep Time Observatory',
            },
        ],
    },
)
