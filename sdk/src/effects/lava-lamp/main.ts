import { canvas } from '@hypercolor/sdk'

interface Blob {
    x: number
    y: number
    vx: number
    vy: number
    radius: number
    phase: number
    seed: number
}

interface Rgb {
    r: number
    g: number
    b: number
}

interface ThemePalette {
    color1: string
    color2: string
    color3: string
}

const THEMES = ['Custom', 'Bubblegum', 'Lagoon', 'Toxic', 'Aurora', 'Molten', 'Synthwave', 'Citrus']

const THEME_PALETTES: Record<string, ThemePalette> = {
    Custom:    { color1: '#16d1d9', color2: '#ff4fb4', color3: '#7d49ff' },
    Bubblegum: { color1: '#ff4f9a', color2: '#ff74c5', color3: '#8a5cff' },
    Lagoon:    { color1: '#3cf2df', color2: '#4a96ff', color3: '#163dff' },
    Toxic:     { color1: '#36ff9a', color2: '#0ae0cb', color3: '#6c2bff' },
    Aurora:    { color1: '#33f587', color2: '#3fdcff', color3: '#8c4bff' },
    Molten:    { color1: '#ff6329', color2: '#ff8d1f', color3: '#ff4b5c' },
    Synthwave: { color1: '#ff4ed6', color2: '#8f48ff', color3: '#42d9ff' },
    Citrus:    { color1: '#ffb347', color2: '#ff7a2f', color3: '#ff5778' },
}

const STEP = 1
const THRESHOLD = 0.94
const CONTOUR_LEVELS = [1.00, 1.34, 1.78]

function clamp(value: number, min: number, max: number): number {
    return Math.max(min, Math.min(max, value))
}

function hash(n: number): number {
    const value = Math.sin(n * 127.1) * 43758.5453123
    return value - Math.floor(value)
}

function hexToRgb(hex: string): Rgb {
    const normalized = hex.replace('#', '')
    const full = normalized.length === 3
        ? `${normalized[0]}${normalized[0]}${normalized[1]}${normalized[1]}${normalized[2]}${normalized[2]}`
        : normalized
    const value = Number.parseInt(full, 16)

    return {
        r: (value >> 16) & 255,
        g: (value >> 8) & 255,
        b: value & 255,
    }
}

function hexToRgba(hex: string, alpha: number): string {
    const rgb = hexToRgb(hex)
    return `rgba(${rgb.r},${rgb.g},${rgb.b},${clamp(alpha, 0, 1).toFixed(3)})`
}

function mixRgb(a: Rgb, b: Rgb, t: number): Rgb {
    const ratio = clamp(t, 0, 1)
    return {
        r: Math.round(a.r + (b.r - a.r) * ratio),
        g: Math.round(a.g + (b.g - a.g) * ratio),
        b: Math.round(a.b + (b.b - a.b) * ratio),
    }
}

function boostRgb(color: Rgb, amount: number): Rgb {
    return {
        r: Math.min(255, Math.round(color.r + amount)),
        g: Math.min(255, Math.round(color.g + amount)),
        b: Math.min(255, Math.round(color.b + amount)),
    }
}

function rgbToHsl(rgb: Rgb): { h: number; s: number; l: number } {
    const r = rgb.r / 255
    const g = rgb.g / 255
    const b = rgb.b / 255

    const max = Math.max(r, g, b)
    const min = Math.min(r, g, b)
    const delta = max - min
    const l = (max + min) * 0.5

    if (delta === 0) {
        return { h: 0, s: 0, l }
    }

    const s = l > 0.5 ? delta / (2 - max - min) : delta / (max + min)
    let h = 0

    if (max === r) h = (g - b) / delta + (g < b ? 6 : 0)
    else if (max === g) h = (b - r) / delta + 2
    else h = (r - g) / delta + 4

    return { h: h * 60, s, l }
}

function hslToRgb(h: number, s: number, l: number): Rgb {
    const c = (1 - Math.abs(2 * l - 1)) * s
    const hp = h / 60
    const x = c * (1 - Math.abs((hp % 2) - 1))

    let r = 0
    let g = 0
    let b = 0

    if (hp >= 0 && hp < 1) [r, g, b] = [c, x, 0]
    else if (hp < 2) [r, g, b] = [x, c, 0]
    else if (hp < 3) [r, g, b] = [0, c, x]
    else if (hp < 4) [r, g, b] = [0, x, c]
    else if (hp < 5) [r, g, b] = [x, 0, c]
    else [r, g, b] = [c, 0, x]

    const m = l - c / 2
    return {
        r: Math.round((r + m) * 255),
        g: Math.round((g + m) * 255),
        b: Math.round((b + m) * 255),
    }
}

function hslToHex(h: number, s: number, l: number): string {
    const c = (1 - Math.abs(2 * l - 1)) * s
    const hp = h / 60
    const x = c * (1 - Math.abs((hp % 2) - 1))

    let r = 0
    let g = 0
    let b = 0

    if (hp >= 0 && hp < 1) [r, g, b] = [c, x, 0]
    else if (hp < 2) [r, g, b] = [x, c, 0]
    else if (hp < 3) [r, g, b] = [0, c, x]
    else if (hp < 4) [r, g, b] = [0, x, c]
    else if (hp < 5) [r, g, b] = [x, 0, c]
    else [r, g, b] = [c, 0, x]

    const m = l - c / 2
    const toHex = (value: number) => Math.round((value + m) * 255).toString(16).padStart(2, '0')

    return `#${toHex(r)}${toHex(g)}${toHex(b)}`
}

function enrichRgb(color: Rgb, saturationBoost: number, lightnessOffset = 0): Rgb {
    const { h, s, l } = rgbToHsl(color)
    return hslToRgb(
        h,
        clamp(s + saturationBoost, 0, 1),
        clamp(l + lightnessOffset, 0, 1),
    )
}

function shiftHexHue(hex: string, deltaDegrees: number): string {
    const { h, s, l } = rgbToHsl(hexToRgb(hex))
    const shifted = (h + deltaDegrees + 360) % 360
    const safeHue = shifted >= 30 && shifted < 90 ? (shifted < 60 ? 24 : 120) : shifted
    return hslToHex(safeHue, s, l)
}

function resolvePalette(theme: string, color1: string, color2: string, color3: string): ThemePalette {
    if (theme !== 'Custom') {
        return THEME_PALETTES[theme] ?? THEME_PALETTES.Custom
    }

    return { color1, color2, color3 }
}

function createBlobs(count: number, width: number, height: number): Blob[] {
    const w = width || 320
    const h = height || 200
    const minDim = Math.min(w, h)
    const blobs: Blob[] = []

    for (let i = 0; i < count; i++) {
        const s1 = hash(i * 13.37 + 1.17)
        const s2 = hash(i * 19.11 + 4.28)
        const s3 = hash(i * 29.87 + 8.72)
        const radius = minDim * (0.072 + s1 * 0.088)

        blobs.push({
            x: radius + s2 * Math.max(8, w - radius * 2),
            y: radius + s3 * Math.max(8, h - radius * 2),
            vx: (hash(i * 5.91 + 0.43) * 2 - 1) * (0.42 + s1 * 0.62),
            vy: (hash(i * 8.27 + 2.83) * 2 - 1) * (0.36 + s2 * 0.78),
            radius,
            phase: hash(i * 31.7 + 6.14) * Math.PI * 2,
            seed: s1,
        })
    }

    return blobs
}

function updateBlobs(blobs: Blob[], time: number, width: number, height: number, speed: number): void {
    const speedScale = speed / 25
    const centerX = width * 0.5
    const centerY = height * 0.5

    for (let i = 0; i < blobs.length; i++) {
        const blob = blobs[i]

        const wobbleX = Math.sin(time * (0.75 + blob.seed * 0.7) + blob.phase) * 0.33
        const wobbleY = Math.cos(time * (0.57 + blob.seed * 0.6) + blob.phase * 1.41) * 0.31

        blob.x += (blob.vx + wobbleX) * speedScale
        blob.y += (blob.vy + wobbleY) * speedScale

        blob.x += (centerX - blob.x) * 0.0015 * speedScale
        blob.y += (centerY - blob.y) * 0.0011 * speedScale

        if (blob.x >= width - blob.radius) {
            if (blob.vx > 0) blob.vx = -blob.vx
            blob.x = width - blob.radius
        } else if (blob.x <= blob.radius) {
            if (blob.vx < 0) blob.vx = -blob.vx
            blob.x = blob.radius
        }

        if (blob.y >= height - blob.radius) {
            if (blob.vy > 0) blob.vy = -blob.vy
            blob.y = height - blob.radius
        } else if (blob.y <= blob.radius) {
            if (blob.vy < 0) blob.vy = -blob.vy
            blob.y = blob.radius
        }
    }
}

function ensureFieldGrid(
    field: Float32Array,
    cols: number,
    rows: number,
    width: number,
    height: number,
): { field: Float32Array; cols: number; rows: number; changed: boolean } {
    const nextCols = Math.floor(width / STEP) + 3
    const nextRows = Math.floor(height / STEP) + 3
    const needsResize = nextCols !== cols || nextRows !== rows || field.length === 0

    if (!needsResize) return { field, cols, rows, changed: false }

    return {
        field: new Float32Array(nextCols * nextRows),
        cols: nextCols,
        rows: nextRows,
        changed: true,
    }
}

function computeField(field: Float32Array, cols: number, rows: number, blobs: Blob[], width: number, height: number): void {
    const stride = cols
    const bubbleFalloff = 28

    for (let gy = 0; gy < rows; gy++) {
        const y = Math.min(height, gy * STEP)
        const row = gy * stride

        for (let gx = 0; gx < cols; gx++) {
            const x = Math.min(width, gx * STEP)
            let value = 0

            for (let i = 0; i < blobs.length; i++) {
                const blob = blobs[i]
                const dx = x - blob.x
                const dy = y - blob.y
                value += (blob.radius * blob.radius) / (dx * dx + dy * dy + bubbleFalloff)
            }

            field[row + gx] = value
        }
    }
}

function drawLavaCells(
    ctx: CanvasRenderingContext2D,
    field: Float32Array,
    cols: number,
    rows: number,
    time: number,
    colorA: Rgb,
    colorB: Rgb,
    colorC: Rgb,
): void {
    const stride = cols

    for (let gy = 0; gy < rows - 1; gy++) {
        for (let gx = 0; gx < cols - 1; gx++) {
            const idx = gy * stride + gx
            const v0 = field[idx]
            const v1 = field[idx + 1]
            const v2 = field[idx + stride + 1]
            const v3 = field[idx + stride]
            const fieldCenter = (v0 + v1 + v2 + v3) * 0.25

            if (fieldCenter < 0.70) continue

            const verticalMix = gy / Math.max(1, rows - 1)
            const flowMix = 0.5 + 0.5 * Math.sin(time * 1.45 + gx * 0.24 - gy * 0.18)
            const mixRatio = clamp(verticalMix * 0.6 + flowMix * 0.4, 0, 1)
            const hotCore = clamp((fieldCenter - 0.96) / 1.32, 0, 1)

            const base = mixRgb(colorA, colorB, mixRatio)
            const baseTone = mixRgb(base, colorC, hotCore * 0.68)
            const brightnessBoost = clamp((fieldCenter - THRESHOLD) * 32, 0, 42)

            let band = 0
            if (fieldCenter > 2.18) band = 3
            else if (fieldCenter > 1.52) band = 2
            else if (fieldCenter > 1.02) band = 1

            const brightTone = boostRgb(baseTone, brightnessBoost + band * 10)
            const tone = enrichRgb(brightTone, 0.16 + hotCore * 0.14 + band * 0.04, -0.04 + hotCore * 0.02)
            const alpha = band === 3 ? 0.94 : band === 2 ? 0.82 : band === 1 ? 0.64 : 0.42

            ctx.fillStyle = `rgba(${tone.r},${tone.g},${tone.b},${alpha.toFixed(3)})`
            ctx.fillRect(gx * STEP, gy * STEP, STEP, STEP)
        }
    }
}

function interpolate(a: number, b: number, threshold: number): number {
    const denom = b - a
    if (Math.abs(denom) < 0.00001) return 0.5
    return clamp((threshold - a) / denom, 0, 1)
}

function traceSegment(ctx: CanvasRenderingContext2D, x1: number, y1: number, x2: number, y2: number): void {
    ctx.moveTo(x1, y1)
    ctx.lineTo(x2, y2)
}

function drawContours(
    ctx: CanvasRenderingContext2D,
    field: Float32Array,
    cols: number,
    rows: number,
    colorA: Rgb,
    colorB: Rgb,
    colorC: Rgb,
): void {
    const stride = cols

    for (let li = 0; li < CONTOUR_LEVELS.length; li++) {
        const level = CONTOUR_LEVELS[li]
        const mid = mixRgb(colorA, colorB, 0.18 + li * 0.20)
        const tone = mixRgb(mid, colorC, 0.28 + li * 0.16)
        const edge = enrichRgb(boostRgb(tone, 60 + li * 14), 0.12 + li * 0.05, -0.02)

        ctx.strokeStyle = `rgba(${edge.r},${edge.g},${edge.b},${(0.12 + li * 0.07).toFixed(3)})`
        ctx.lineWidth = 0.46 + li * 0.14
        ctx.beginPath()

        for (let gy = 0; gy < rows - 1; gy++) {
            for (let gx = 0; gx < cols - 1; gx++) {
                const idx = gy * stride + gx
                const v0 = field[idx]
                const v1 = field[idx + 1]
                const v2 = field[idx + stride + 1]
                const v3 = field[idx + stride]

                let mask = 0
                if (v0 > level) mask |= 1
                if (v1 > level) mask |= 2
                if (v2 > level) mask |= 4
                if (v3 > level) mask |= 8

                if (mask === 0 || mask === 15) continue

                const x = gx * STEP
                const y = gy * STEP

                const topX = x + interpolate(v0, v1, level) * STEP
                const topY = y
                const rightX = x + STEP
                const rightY = y + interpolate(v1, v2, level) * STEP
                const bottomX = x + interpolate(v3, v2, level) * STEP
                const bottomY = y + STEP
                const leftX = x
                const leftY = y + interpolate(v0, v3, level) * STEP

                switch (mask) {
                    case 1:
                    case 14:
                        traceSegment(ctx, leftX, leftY, topX, topY)
                        break
                    case 2:
                    case 13:
                        traceSegment(ctx, topX, topY, rightX, rightY)
                        break
                    case 3:
                    case 12:
                        traceSegment(ctx, leftX, leftY, rightX, rightY)
                        break
                    case 4:
                    case 11:
                        traceSegment(ctx, rightX, rightY, bottomX, bottomY)
                        break
                    case 6:
                    case 9:
                        traceSegment(ctx, topX, topY, bottomX, bottomY)
                        break
                    case 7:
                    case 8:
                        traceSegment(ctx, leftX, leftY, bottomX, bottomY)
                        break
                    case 5:
                        traceSegment(ctx, leftX, leftY, bottomX, bottomY)
                        traceSegment(ctx, topX, topY, rightX, rightY)
                        break
                    case 10:
                        traceSegment(ctx, leftX, leftY, topX, topY)
                        traceSegment(ctx, rightX, rightY, bottomX, bottomY)
                        break
                }
            }
        }

        ctx.stroke()
    }
}

function drawBackdrop(
    ctx: CanvasRenderingContext2D,
    width: number,
    height: number,
    palette: ThemePalette,
): void {
    const colorA = hexToRgb(palette.color1)
    const colorB = hexToRgb(palette.color2)
    const colorC = hexToRgb(palette.color3)

    const wash = ctx.createRadialGradient(width * 0.34, height * 0.24, 0, width * 0.34, height * 0.24, width * 0.72)
    wash.addColorStop(0, `rgba(${colorA.r},${colorA.g},${colorA.b},0.16)`)
    wash.addColorStop(0.52, `rgba(${colorB.r},${colorB.g},${colorB.b},0.08)`)
    wash.addColorStop(1, 'rgba(0,0,0,0)')
    ctx.fillStyle = wash
    ctx.fillRect(0, 0, width, height)

    const haze = ctx.createLinearGradient(0, height, width, 0)
    haze.addColorStop(0, `rgba(${colorC.r},${colorC.g},${colorC.b},0.09)`)
    haze.addColorStop(1, 'rgba(0,0,0,0)')
    ctx.fillStyle = haze
    ctx.fillRect(0, 0, width, height)
}

export default canvas.stateful('Lava Lamp', {
    bgColor:    '#0b0312',
    bgCycle:    false,
    theme:      THEMES,
    color1:     '#16d1d9',
    color2:     '#ff4fb4',
    color3:     '#7d49ff',
    rainbow:    false,
    speed:      [1, 100, 22],
    cycleSpeed: [1, 100, 22],
    bCount:     [1, 18, 6],
}, () => {
    let blobs = createBlobs(6, 320, 200)
    let blobCount = 6
    let bgHue = 0
    let lavaHue = 0

    let field = new Float32Array(0)
    let cols = 0
    let rows = 0

    let lastTime = 0

    return (ctx, time, c) => {
        const bgColor = c.bgColor as string
        const bgCycle = c.bgCycle as boolean
        const theme = c.theme as string
        const color1 = c.color1 as string
        const color2 = c.color2 as string
        const color3 = c.color3 as string
        const rainbow = c.rainbow as boolean
        const speed = c.speed as number
        const cycleSpeed = c.cycleSpeed as number
        const bCount = Math.round(c.bCount as number)

        const w = ctx.canvas.width
        const h = ctx.canvas.height
        const dt = lastTime > 0 ? Math.min(time - lastTime, 0.05) : 1 / 60
        lastTime = time

        const grid = ensureFieldGrid(field, cols, rows, w, h)
        if (grid.changed) {
            field = grid.field
            cols = grid.cols
            rows = grid.rows
        }

        if (bCount !== blobCount) {
            blobs = createBlobs(bCount, w, h)
            blobCount = bCount
        }

        bgHue = (bgHue + cycleSpeed * 1.2 * dt) % 360
        lavaHue = (lavaHue + cycleSpeed * 2.2 * dt) % 360

        updateBlobs(blobs, time, w, h, speed)

        const palette = resolvePalette(theme, color1, color2, color3)

        const backgroundColor = bgCycle
            ? shiftHexHue(bgColor, bgHue)
            : bgColor
        ctx.fillStyle = backgroundColor
        ctx.fillRect(0, 0, w, h)

        drawBackdrop(ctx, w, h, palette)

        const vignette = ctx.createLinearGradient(0, 0, 0, h)
        vignette.addColorStop(0, hexToRgba(bgColor, 0.17))
        vignette.addColorStop(0.5, hexToRgba('#000000', 0.0))
        vignette.addColorStop(1, hexToRgba('#000000', 0.2))
        ctx.fillStyle = vignette
        ctx.fillRect(0, 0, w, h)

        const lavaColorA = rainbow
            ? shiftHexHue(palette.color1, lavaHue)
            : palette.color1
        const lavaColorB = rainbow
            ? shiftHexHue(palette.color2, lavaHue + 140)
            : palette.color2
        const lavaColorC = rainbow
            ? shiftHexHue(palette.color3, lavaHue + 280)
            : palette.color3

        const colorA = hexToRgb(lavaColorA)
        const colorB = hexToRgb(lavaColorB)
        const colorC = hexToRgb(lavaColorC)

        computeField(field, cols, rows, blobs, w, h)
        drawLavaCells(ctx, field, cols, rows, time, colorA, colorB, colorC)
        drawContours(ctx, field, cols, rows, colorA, colorB, colorC)
    }
}, {
    description: 'Community-inspired contour metaballs with crisp RGB blends and merge/split motion',
})
