import { canvas, normalizeSpeed } from '@hypercolor/sdk'

// ── Types ────────────────────────────────────────────────────────────────

interface DashStar {
    x: number
    y: number
    size: number
    twinkle: number
    drift: number
    seed: number
    hueOffset: number
}

// ── Constants ────────────────────────────────────────────────────────────

const TRAIL_MODES = ['Classic', 'Comet', 'Pulse']

// ── Helpers ──────────────────────────────────────────────────────────────

function clamp(value: number, min: number, max: number): number {
    return Math.max(min, Math.min(max, value))
}

function snap(value: number): number {
    return Math.round(value)
}

function hash(value: number): number {
    const seeded = Math.sin(value * 127.1 + 311.7) * 43758.5453123
    return seeded - Math.floor(seeded)
}

function hslToHex(h: number, s: number, l: number): string {
    const hNorm = ((h % 360) + 360) % 360
    const sNorm = clamp(s, 0, 100) / 100
    const lNorm = clamp(l, 0, 100) / 100

    const c = (1 - Math.abs(2 * lNorm - 1)) * sNorm
    const x = c * (1 - Math.abs(((hNorm / 60) % 2) - 1))
    const m = lNorm - c / 2

    let r = 0
    let g = 0
    let b = 0

    if (hNorm < 60) [r, g, b] = [c, x, 0]
    else if (hNorm < 120) [r, g, b] = [x, c, 0]
    else if (hNorm < 180) [r, g, b] = [0, c, x]
    else if (hNorm < 240) [r, g, b] = [0, x, c]
    else if (hNorm < 300) [r, g, b] = [x, 0, c]
    else [r, g, b] = [c, 0, x]

    const toHex = (value: number) => Math.round((value + m) * 255).toString(16).padStart(2, '0')
    return `#${toHex(r)}${toHex(g)}${toHex(b)}`
}

function fillRect(
    ctx: CanvasRenderingContext2D,
    x: number,
    y: number,
    width: number,
    height: number,
    color: string,
): void {
    if (width <= 0 || height <= 0) return
    ctx.fillStyle = color
    ctx.fillRect(snap(x), snap(y), Math.max(1, Math.round(width)), Math.max(1, Math.round(height)))
}

// ── Star Management ─────────────────────────────────────────────────────

function buildStars(targetCount: number): DashStar[] {
    const stars: DashStar[] = []

    for (let i = 0; i < targetCount; i++) {
        const s1 = hash(i * 1.87 + 2.17)
        const s2 = hash(i * 2.93 + 6.11)
        const s3 = hash(i * 4.77 + 9.41)
        const s4 = hash(i * 8.13 + 1.29)
        const s5 = hash(i * 12.41 + 4.83)
        const s6 = hash(i * 16.53 + 5.09)

        stars.push({
            x: s1,
            y: s2,
            size: 1 + s3 * 2.3,
            twinkle: 1.1 + s4 * 2.6,
            drift: 4 + s5 * 28,
            seed: s6,
            hueOffset: s4 * 360,
        })
    }

    return stars
}

// ── Drawing Functions ───────────────────────────────────────────────────

function drawBackdrop(
    ctx: CanvasRenderingContext2D,
    width: number,
    height: number,
    cycleHue: number,
    colorCycle: boolean,
): void {
    const top = colorCycle ? hslToHex(cycleHue + 234, 72, 10) : '#080b24'
    const bottom = colorCycle ? hslToHex(cycleHue + 272, 68, 6) : '#03040e'

    const bg = ctx.createLinearGradient(0, 0, 0, height)
    bg.addColorStop(0, top)
    bg.addColorStop(1, bottom)
    ctx.fillStyle = bg
    ctx.fillRect(0, 0, width, height)

    const haze = ctx.createLinearGradient(0, height * 0.4, 0, height)
    haze.addColorStop(0, 'rgba(128, 255, 234, 0.01)')
    haze.addColorStop(1, 'rgba(225, 53, 255, 0.06)')
    ctx.fillStyle = haze
    ctx.fillRect(0, 0, width, height)
}

function drawStars(
    ctx: CanvasRenderingContext2D,
    stars: DashStar[],
    width: number,
    height: number,
    time: number,
    speed: number,
    cycleHue: number,
    colorCycle: boolean,
): void {
    const driftScale = 0.35 + speed * 0.16

    for (let i = 0; i < stars.length; i++) {
        const star = stars[i]
        const x = (star.x * width + time * star.drift * driftScale) % width
        const y = star.y * height + Math.sin(time * (0.7 + star.seed) + star.seed * 11.3) * (2 + star.size)
        const twinkle = 0.5 + 0.5 * Math.sin(time * star.twinkle + star.seed * 23.4)
        const alpha = 0.16 + twinkle * 0.82
        const size = Math.max(1, Math.round(star.size + twinkle * 0.8))

        const color = colorCycle
            ? hslToHex(cycleHue + star.hueOffset, 96, 66)
            : '#ecf2ff'

        ctx.globalAlpha = alpha
        ctx.fillStyle = color
        ctx.fillRect(snap(x), snap(y), size, size)

        if (twinkle > 0.72) {
            const arm = Math.max(2, Math.round(size * 1.6))
            ctx.fillRect(snap(x - arm), snap(y), arm * 2 + 1, 1)
            ctx.fillRect(snap(x), snap(y - arm), 1, arm * 2 + 1)
        }

        if (twinkle > 0.93) {
            const pop = Math.max(2, Math.round(size * 2.2))
            ctx.globalAlpha = 0.25 + alpha * 0.42
            ctx.fillRect(snap(x - pop), snap(y - pop), pop * 2 + 1, 1)
            ctx.fillRect(snap(x - pop), snap(y + pop), pop * 2 + 1, 1)
            ctx.fillRect(snap(x - pop), snap(y - pop), 1, pop * 2 + 1)
            ctx.fillRect(snap(x + pop), snap(y - pop), 1, pop * 2 + 1)
        }
    }

    ctx.globalAlpha = 1
}

function drawTrail(
    ctx: CanvasRenderingContext2D,
    width: number,
    catLeft: number,
    catCenterY: number,
    time: number,
    unit: number,
    cycleHue: number,
    colorCycle: boolean,
    trailMode: string,
    animationSpeed: number,
): void {
    const trailLength = Math.max(0, catLeft + 4 * unit)
    if (trailLength <= 0) return

    const baseBands = ['#ff3f8e', '#ff8656', '#ffb347', '#74f2a8', '#5dc9ff', '#9380ff']
    const bandHeight = Math.max(1, Math.round(unit * 2.35))
    const top = catCenterY - bandHeight * 3
    const segment = Math.max(2, Math.round(unit * 3.4))
    const mode = trailMode
    const pulseClock = time * (6 + animationSpeed * 0.7)
    const dt = 1 / 60

    for (let bandIndex = 0; bandIndex < baseBands.length; bandIndex++) {
        const color = colorCycle
            ? hslToHex(cycleHue + bandIndex * 42, 96, 58)
            : baseBands[bandIndex]
        const yBase = top + bandIndex * bandHeight

        for (let x = -segment * 2; x < trailLength + segment; x += segment) {
            const wave = Math.sin(time * 4.2 + x * 0.048 + bandIndex * 0.72) * bandHeight * 0.2
            let modeWave = wave
            let stretch = 1

            if (mode === 'Pulse') {
                stretch = 0.7 + 0.32 * (0.5 + 0.5 * Math.sin(pulseClock + x * 0.035 + bandIndex))
            } else if (mode === 'Comet') {
                modeWave += Math.sin(time * 8.2 + x * 0.11 + bandIndex * 1.4) * bandHeight * 0.42
                stretch = 0.84 + 0.18 * (0.5 + 0.5 * Math.sin(pulseClock + x * 0.04 + bandIndex * 0.6))
            }

            const h = Math.max(1, Math.round(bandHeight * stretch))
            const y = yBase + modeWave + (bandHeight - h) * 0.5
            const alpha = mode === 'Comet' ? 0.78 + 0.2 * (0.5 + 0.5 * Math.sin(pulseClock + x * 0.06)) : 0.92

            ctx.globalAlpha = alpha
            ctx.fillStyle = color
            ctx.fillRect(snap(x), snap(y), segment + 1, h)

            if (mode === 'Comet' && (bandIndex === 0 || bandIndex === 5)) {
                const marker = Math.floor(x / segment) + Math.floor((time + dt) * 15)
                if (marker % 8 === 0) {
                    ctx.globalAlpha = 0.6
                    const sparkleX = snap(x + segment * 0.5)
                    const sparkleY = snap(y + h * 0.5)
                    ctx.fillStyle = hslToHex(cycleHue + bandIndex * 42, 96, 64)
                    ctx.fillRect(sparkleX - 1, sparkleY, 3, 1)
                    ctx.fillRect(sparkleX, sparkleY - 1, 1, 3)
                }
            }
        }
    }

    ctx.globalAlpha = 1

    // Fade trail edge into the horizon for cleaner loops.
    const fade = ctx.createLinearGradient(Math.min(width, trailLength), 0, 0, 0)
    fade.addColorStop(0, 'rgba(0, 0, 0, 0)')
    fade.addColorStop(1, 'rgba(0, 0, 0, 0.22)')
    ctx.fillStyle = fade
    ctx.fillRect(0, top - bandHeight, trailLength, bandHeight * 8)
}

function drawCat(
    ctx: CanvasRenderingContext2D,
    centerX: number,
    centerY: number,
    unit: number,
    time: number,
    cycleHue: number,
    colorCycle: boolean,
    animationSpeed: number,
): void {
    const outline = '#1e1636'
    const headColor = '#d6dcf8'
    const bodyColor = '#f4d0a3'
    const frosting = colorCycle ? hslToHex(cycleHue + 332, 88, 70) : '#ff8ed4'
    const frostingShade = colorCycle ? hslToHex(cycleHue + 320, 84, 62) : '#ff6ec4'

    const bodyW = 20 * unit
    const bodyH = 13 * unit
    const headSize = 10 * unit

    const bodyX = snap(centerX - bodyW * 0.5)
    const bodyY = snap(centerY - bodyH * 0.5)

    const tailWag = snap(Math.sin(time * (7 + animationSpeed * 0.45)) * unit)

    // Tail (stepped silhouette keeps visibility high on LED grids).
    fillRect(ctx, bodyX - 7 * unit, bodyY + 4 * unit + tailWag, 6 * unit, 3 * unit, outline)
    fillRect(ctx, bodyX - 6 * unit, bodyY + 5 * unit + tailWag, 4 * unit, unit, '#bcc2da')

    // Legs
    const stride = Math.sin(time * (8 + animationSpeed)) * unit * 0.8
    const legYs = [
        snap(bodyY + bodyH - unit + stride * 0.3),
        snap(bodyY + bodyH - unit - stride * 0.2),
        snap(bodyY + bodyH - unit + stride * 0.15),
        snap(bodyY + bodyH - unit - stride * 0.25),
    ]
    const legXs = [bodyX + 2 * unit, bodyX + 7 * unit, bodyX + 12 * unit, bodyX + 17 * unit]

    for (let i = 0; i < legXs.length; i++) {
        fillRect(ctx, legXs[i] - unit, legYs[i] - unit, 2 * unit + 1, 3 * unit, outline)
        fillRect(ctx, legXs[i], legYs[i], unit, 2 * unit, '#c8cde0')
    }

    // Body pastry
    fillRect(ctx, bodyX - unit, bodyY - unit, bodyW + 2 * unit, bodyH + 2 * unit, outline)
    fillRect(ctx, bodyX, bodyY, bodyW, bodyH, bodyColor)

    // Frosting slab + inner fill for depth.
    fillRect(ctx, bodyX + 2 * unit, bodyY + unit, bodyW - 4 * unit, bodyH - 4 * unit, frosting)
    fillRect(ctx, bodyX + 3 * unit, bodyY + 2 * unit, bodyW - 7 * unit, bodyH - 6 * unit, frostingShade)

    const sprinklePalette = colorCycle
        ? [
            hslToHex(cycleHue + 22, 96, 64),
            hslToHex(cycleHue + 120, 94, 70),
            hslToHex(cycleHue + 190, 96, 72),
            hslToHex(cycleHue + 276, 92, 74),
            hslToHex(cycleHue + 340, 94, 72),
        ]
        : ['#ffb347', '#74f3ff', '#7eff9a', '#b7a8ff', '#ffb4d9']

    const sprinkles: Array<[number, number, number]> = [
        [4, 3, 0],
        [8, 2, 1],
        [12, 4, 2],
        [6, 6, 3],
        [10, 7, 4],
        [14, 6, 0],
        [16, 3, 2],
        [5, 8, 1],
    ]

    for (let i = 0; i < sprinkles.length; i++) {
        const [sx, sy, colorIndex] = sprinkles[i]
        fillRect(
            ctx,
            bodyX + sx * unit,
            bodyY + sy * unit,
            2 * unit,
            Math.max(1, unit),
            sprinklePalette[colorIndex],
        )
    }

    // Head
    const headX = bodyX + bodyW - 2 * unit
    const headY = bodyY - unit
    fillRect(ctx, headX - unit, headY - unit, headSize + 2 * unit, headSize + 2 * unit, outline)
    fillRect(ctx, headX, headY, headSize, headSize, headColor)

    // Ears (blocky stepped ears for a custom silhouette).
    fillRect(ctx, headX + unit, headY - 4 * unit, 3 * unit, 3 * unit, outline)
    fillRect(ctx, headX + 2 * unit, headY - 3 * unit, unit, unit, '#ffc0da')

    fillRect(ctx, headX + 6 * unit, headY - 4 * unit, 3 * unit, 3 * unit, outline)
    fillRect(ctx, headX + 7 * unit, headY - 3 * unit, unit, unit, '#ffc0da')

    // Face details
    const blink = Math.sin(time * 2.8 + centerX * 0.01) > 0.94
    const eyeColor = '#1a1830'

    if (blink) {
        fillRect(ctx, headX + 2 * unit, headY + 4 * unit, 2 * unit, 1, eyeColor)
        fillRect(ctx, headX + 6 * unit, headY + 4 * unit, 2 * unit, 1, eyeColor)
    } else {
        fillRect(ctx, headX + 2 * unit, headY + 3 * unit, unit, 2 * unit, eyeColor)
        fillRect(ctx, headX + 7 * unit, headY + 3 * unit, unit, 2 * unit, eyeColor)
    }

    fillRect(ctx, headX + 4 * unit, headY + 5 * unit, 2 * unit, unit, '#ff7bb4')
    fillRect(ctx, headX + 3 * unit, headY + 6 * unit, unit, 1, eyeColor)
    fillRect(ctx, headX + 6 * unit, headY + 6 * unit, unit, 1, eyeColor)

    // Whiskers
    fillRect(ctx, headX - unit, headY + 4 * unit, 2 * unit, 1, outline)
    fillRect(ctx, headX - unit, headY + 6 * unit, 2 * unit, 1, outline)
    fillRect(ctx, headX + headSize - 1, headY + 4 * unit, 2 * unit, 1, outline)
    fillRect(ctx, headX + headSize - 1, headY + 6 * unit, 2 * unit, 1, outline)
}

// ── Effect ──────────────────────────────────────────────────────────────

canvas('Nyan Dash', {
    animationSpeed: [1, 10, 6],
    scale: [40, 180, 100],
    positionX: [-100, 100, 0],
    positionY: [-100, 100, 0],
    trailMode: TRAIL_MODES,
    colorCycle: true,
    cycleSpeed: [0, 100, 34],
    starDensity: [0, 100, 44],
}, () => {
    // Persistent state across frames
    let stars: DashStar[] = []
    let starCount = 0
    let canvasWidth = 0
    let canvasHeight = 0

    function syncStars(width: number, height: number, targetCount: number, force = false): void {
        const sizeChanged = canvasWidth !== width || canvasHeight !== height

        if (!force && !sizeChanged && targetCount === starCount) return

        canvasWidth = width
        canvasHeight = height
        starCount = targetCount
        stars = buildStars(targetCount)
    }

    // Initial sync
    syncStars(320, 200, Math.max(0, Math.floor(44 * 1.25)), true)

    return (ctx, time, controls) => {
        const width = ctx.canvas.width
        const height = ctx.canvas.height

        const rawAnimationSpeed = controls.animationSpeed as number
        const speed = normalizeSpeed(rawAnimationSpeed)
        const rawScale = clamp(controls.scale as number, 40, 180)
        const positionX = clamp(controls.positionX as number, -100, 100)
        const positionY = clamp(controls.positionY as number, -100, 100)
        const trailMode = TRAIL_MODES.includes(controls.trailMode as string)
            ? (controls.trailMode as string)
            : 'Classic'
        const colorCycle = controls.colorCycle as boolean
        const cycleSpeed = clamp(controls.cycleSpeed as number, 0, 100)
        const starDensity = clamp(controls.starDensity as number, 0, 100)

        const targetStarCount = Math.max(0, Math.floor(starDensity * 1.25))
        syncStars(width, height, targetStarCount)

        const scale = clamp(rawScale / 100, 0.4, 1.8)
        const unit = Math.max(1, Math.round(2 * scale))
        const cycleHue = colorCycle ? time * cycleSpeed * 0.9 : 0

        drawBackdrop(ctx, width, height, cycleHue, colorCycle)
        drawStars(ctx, stars, width, height, time, speed, cycleHue, colorCycle)

        const travelPadding = 70 * scale
        const travel = (time * speed * 0.14) % 1
        const loopX = travel * (width + travelPadding * 2) - travelPadding

        const offsetX = (positionX / 100) * width * 0.36
        const offsetY = (positionY / 100) * height * 0.36
        const bob = Math.sin(time * (2.6 + speed * 0.5)) * 4 * scale

        const catX = loopX + offsetX
        const catY = clamp(height * 0.55 + offsetY + bob, 18 * scale, height - 18 * scale)

        const bodyWidth = 20 * unit
        const catLeft = catX - bodyWidth * 0.5 - 2 * unit

        drawTrail(ctx, width, catLeft, catY, time, unit, cycleHue, colorCycle, trailMode, speed)
        drawCat(ctx, catX, catY, unit, time, cycleHue, colorCycle, speed)
    }
}, {
    description: 'Playful stylized cat dash with rainbow trail variants, star pops, and smooth looping motion',
})
