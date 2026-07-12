import { canvas, color, combo, num } from 'hypercolor'

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

/** sRGB → Oklab (ported from the SDK palette runtime). */
function rgbToOklab(color: Rgb): [number, number, number] {
    const linear = (channel: number): number => {
        const c = channel / 255
        return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4
    }
    const lr = linear(color.r)
    const lg = linear(color.g)
    const lb = linear(color.b)

    const l = Math.cbrt(0.4122214708 * lr + 0.5363325363 * lg + 0.0514459929 * lb)
    const m = Math.cbrt(0.2119034982 * lr + 0.6806995451 * lg + 0.1073969566 * lb)
    const s = Math.cbrt(0.0883024619 * lr + 0.2817188376 * lg + 0.6299787005 * lb)

    return [
        0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s,
        1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s,
        0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s,
    ]
}

/** Oklab → sRGB (ported from the SDK palette runtime). */
function oklabToRgb(lightness: number, a: number, b: number): Rgb {
    const lRoot = lightness + 0.3963377774 * a + 0.2158037573 * b
    const mRoot = lightness - 0.1055613458 * a - 0.0638541728 * b
    const sRoot = lightness - 0.0894841775 * a - 1.291485548 * b

    const l = lRoot * lRoot * lRoot
    const m = mRoot * mRoot * mRoot
    const s = sRoot * sRoot * sRoot

    const compress = (channel: number): number => {
        const c = channel <= 0.0031308 ? 12.92 * channel : 1.055 * channel ** (1 / 2.4) - 0.055
        return Math.round(Math.max(0, Math.min(1, c)) * 255)
    }

    return {
        b: compress(-0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s),
        g: compress(-1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s),
        r: compress(4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s),
    }
}

/** Perceptual blend between two colors — avoids the muddy midpoints of sRGB mixing. */
function mixOklab(a: Rgb, b: Rgb, t: number): Rgb {
    const ratio = Math.max(0, Math.min(1, t))
    const from = rgbToOklab(a)
    const to = rgbToOklab(b)
    return oklabToRgb(
        from[0] + (to[0] - from[0]) * ratio,
        from[1] + (to[1] - from[1]) * ratio,
        from[2] + (to[2] - from[2]) * ratio,
    )
}

/** Clamp the whiteness ratio (min/max channel) so pastel picks stay vivid on LEDs. */
function ensureVivid(color: Rgb, maxWhiteness: number): Rgb {
    const max = Math.max(color.r, color.g, color.b)
    const min = Math.min(color.r, color.g, color.b)
    // Near-gray inputs have no hue to boost — leave intentional white/gray alone.
    if (max <= 0 || max - min < 1 || min <= max * maxWhiteness) return color
    const scale = (max * (1 - maxWhiteness)) / (max - min)
    return {
        b: Math.round(max - (max - color.b) * scale),
        g: Math.round(max - (max - color.g) * scale),
        r: Math.round(max - (max - color.r) * scale),
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
            skyBottom: mixOklab(bottom, { b: 118, g: 78, r: 22 }, 0.26),
            skyTop: mixOklab(top, { b: 42, g: 14, r: 12 }, 0.34),
            star: ensureVivid(mixOklab(star, { b: 255, g: 238, r: 150 }, 0.2), 0.4),
            trail: ensureVivid(mixOklab(star, { b: 190, g: 104, r: 255 }, 0.24), 0.4),
        }
    }

    return {
        skyBottom: mixOklab(bottom, { b: 68, g: 30, r: 15 }, 0.2),
        skyTop: mixOklab(top, { b: 18, g: 6, r: 3 }, 0.32),
        star: ensureVivid(mixOklab(star, { b: 255, g: 224, r: 162 }, 0.14), 0.4),
        trail: ensureVivid(mixOklab(star, { b: 255, g: 210, r: 92 }, 0.2), 0.4),
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
        starColor: color('Star Color', '#5fc8ff', { group: 'Color' }),
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
            const headColor = mixOklab(tone.star, tone.trail, 0.34)

            for (const meteor of meteors) {
                const velocity = Math.hypot(meteor.vx, meteor.vy)
                if (velocity <= 0.0001) continue

                const ux = meteor.vx / velocity
                const uy = meteor.vy / velocity
                const dynamicTrail = meteor.trail * (0.84 + 0.16 * Math.sin(time * 4.6 + meteor.phase))
                const tailX = meteor.x - ux * dynamicTrail
                const tailY = meteor.y - uy * dynamicTrail

                ctx.lineCap = 'round'
                ctx.lineWidth = Math.max(1.15, meteor.size * 0.82)

                if (meteor.size >= 3.2) {
                    // Large meteors (few on screen): full gradient trails.
                    const mainTrail = ctx.createLinearGradient(meteor.x, meteor.y, tailX, tailY)
                    mainTrail.addColorStop(0, rgbToRgba(headColor, 0.92 * meteor.brightness))
                    mainTrail.addColorStop(0.24, rgbToRgba(tone.star, 0.58 * meteor.brightness))
                    mainTrail.addColorStop(1, rgbToRgba(tone.trail, 0))

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
                } else {
                    // Small meteors: two-stop solid strokes — gradients are baked to
                    // positions, so per-frame rebuilds cost more than they add here.
                    const midX = meteor.x - ux * dynamicTrail * 0.3
                    const midY = meteor.y - uy * dynamicTrail * 0.3

                    ctx.strokeStyle = rgbToRgba(headColor, 0.88 * meteor.brightness)
                    ctx.beginPath()
                    ctx.moveTo(meteor.x, meteor.y)
                    ctx.lineTo(midX, midY)
                    ctx.stroke()

                    ctx.strokeStyle = rgbToRgba(tone.star, 0.26 * meteor.brightness)
                    ctx.beginPath()
                    ctx.moveTo(midX, midY)
                    ctx.lineTo(tailX, tailY)
                    ctx.stroke()

                    ctx.lineWidth = Math.max(2.25, meteor.size * 2.05)
                    ctx.strokeStyle = rgbToRgba(tone.trail, 0.12 * meteor.brightness)
                    ctx.beginPath()
                    ctx.moveTo(meteor.x, meteor.y)
                    ctx.lineTo(tailX, tailY)
                    ctx.stroke()
                }

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
                    starColor: '#5fc8ff',
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
                    starColor: '#c060ff',
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
                    starColor: '#ffb347',
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
                    starColor: '#ffc860',
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
