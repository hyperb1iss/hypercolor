import { canvas } from '@hypercolor/sdk'

interface Blob {
    x: number
    y: number
    radius: number
    lane: number
    phase: number
    cycleRate: number
    sway: number
    crownDrift: number
    baseRadius: number
    topBias: number
    splitBias: number
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

interface LavaTones {
    shell: Rgb
    mid: Rgb
    core: Rgb
    highlight: Rgb
    contours: [Rgb, Rgb, Rgb]
}

const THEMES = ['Custom', 'Bubblegum', 'Lagoon', 'Toxic', 'Aurora', 'Molten', 'Synthwave', 'Citrus']

const THEME_PALETTES: Record<string, ThemePalette> = {
    Aurora: { color1: '#33f587', color2: '#3fdcff', color3: '#8c4bff' },
    Bubblegum: { color1: '#ff4f9a', color2: '#ff74c5', color3: '#8a5cff' },
    Citrus: { color1: '#ffb347', color2: '#ff7a2f', color3: '#ff5778' },
    Custom: { color1: '#16d1d9', color2: '#ff4fb4', color3: '#7d49ff' },
    Lagoon: { color1: '#3cf2df', color2: '#4a96ff', color3: '#163dff' },
    Molten: { color1: '#ff6329', color2: '#ff8d1f', color3: '#ff4b5c' },
    Synthwave: { color1: '#ff4ed6', color2: '#8f48ff', color3: '#42d9ff' },
    Toxic: { color1: '#36ff9a', color2: '#0ae0cb', color3: '#6c2bff' },
}

const TAU = Math.PI * 2
const STEP = 2
const THRESHOLD = 0.78
const CONTOUR_LEVELS = [0.94, 1.28, 1.72]

function clamp(value: number, min: number, max: number): number {
    if (Number.isNaN(value)) return min
    return Math.max(min, Math.min(max, value))
}

function mix(a: number, b: number, t: number): number {
    return a + (b - a) * clamp(t, 0, 1)
}

function smoothstep(edge0: number, edge1: number, value: number): number {
    if (edge0 === edge1) return value < edge0 ? 0 : 1
    const t = clamp((value - edge0) / (edge1 - edge0), 0, 1)
    return t * t * (3 - 2 * t)
}

function fract(value: number): number {
    return value - Math.floor(value)
}

function easeInCubic(value: number): number {
    const t = clamp(value, 0, 1)
    return t * t * t
}

function easeOutCubic(value: number): number {
    const t = 1 - clamp(value, 0, 1)
    return 1 - t * t * t
}

function easeInOutSine(value: number): number {
    return -(Math.cos(Math.PI * clamp(value, 0, 1)) - 1) * 0.5
}

function hash(n: number): number {
    const value = Math.sin(n * 127.1) * 43758.5453123
    return value - Math.floor(value)
}

function hashSigned(n: number): number {
    return hash(n) * 2 - 1
}

function hexToRgb(hex: string): Rgb {
    const normalized = hex.replace('#', '')
    const full =
        normalized.length === 3
            ? `${normalized[0]}${normalized[0]}${normalized[1]}${normalized[1]}${normalized[2]}${normalized[2]}`
            : normalized
    const value = Number.parseInt(full, 16)

    return {
        b: value & 255,
        g: (value >> 8) & 255,
        r: (value >> 16) & 255,
    }
}

function hexToRgba(hex: string, alpha: number): string {
    const rgb = hexToRgb(hex)
    return `rgba(${rgb.r},${rgb.g},${rgb.b},${clamp(alpha, 0, 1).toFixed(3)})`
}

function mixRgb(a: Rgb, b: Rgb, t: number): Rgb {
    const ratio = clamp(t, 0, 1)
    return {
        b: Math.round(a.b + (b.b - a.b) * ratio),
        g: Math.round(a.g + (b.g - a.g) * ratio),
        r: Math.round(a.r + (b.r - a.r) * ratio),
    }
}

function boostRgb(color: Rgb, amount: number): Rgb {
    return {
        b: Math.min(255, Math.round(color.b + amount)),
        g: Math.min(255, Math.round(color.g + amount)),
        r: Math.min(255, Math.round(color.r + amount)),
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

    if (delta === 0) return { h: 0, l, s: 0 }

    const s = l > 0.5 ? delta / (2 - max - min) : delta / (max + min)
    let h = 0

    if (max === r) h = (g - b) / delta + (g < b ? 6 : 0)
    else if (max === g) h = (b - r) / delta + 2
    else h = (r - g) / delta + 4

    return { h: h * 60, l, s }
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
        b: Math.round((b + m) * 255),
        g: Math.round((g + m) * 255),
        r: Math.round((r + m) * 255),
    }
}

function hslToHex(h: number, s: number, l: number): string {
    const rgb = hslToRgb(h, s, l)
    return `#${rgb.r.toString(16).padStart(2, '0')}${rgb.g.toString(16).padStart(2, '0')}${rgb.b.toString(16).padStart(2, '0')}`
}

function enrichRgb(color: Rgb, saturationBoost: number, lightnessOffset = 0): Rgb {
    const { h, s, l } = rgbToHsl(color)
    return hslToRgb(h, clamp(s + saturationBoost, 0, 1), clamp(l + lightnessOffset, 0, 1))
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
    const sizeScale = clamp(1.18 - count * 0.035, 0.56, 1.12)
    const laneCount = Math.max(2, Math.min(5, Math.round(count / 2)))
    const blobs: Blob[] = []

    for (let i = 0; i < count; i++) {
        const seed = hash(i * 17.17 + 1.13)
        const sizeBias = hash(i * 31.03 + 4.91)
        const laneIndex = i % laneCount
        const laneBase = laneCount === 1 ? 0.5 : laneIndex / Math.max(1, laneCount - 1)
        const lane = clamp(0.16 + laneBase * 0.68 + hashSigned(i * 7.37 + 2.11) * 0.07, 0.14, 0.86)
        const splitBias =
            i >= Math.ceil(count * 0.65) ? 0.72 + hash(i * 13.41 + 7.51) * 0.28 : hash(i * 13.41 + 7.51) * 0.62

        blobs.push({
            baseRadius: minDim * (0.082 + sizeBias * 0.06) * sizeScale * mix(1, 0.72, splitBias * 0.65),
            crownDrift: hashSigned(i * 14.51 + 3.66) * (0.12 + splitBias * 0.08),
            cycleRate: 0.055 + hash(i * 5.83 + 6.21) * 0.038 + splitBias * 0.018,
            lane,
            phase: hash(i * 9.71 + 8.13),
            radius: minDim * 0.1,
            seed,
            splitBias,
            sway: 0.03 + hash(i * 11.19 + 2.72) * 0.08,
            topBias: hash(i * 21.13 + 1.04),
            x: lane * w,
            y: h * (0.32 + seed * 0.5),
        })
    }

    return blobs
}

function updateBlobs(blobs: Blob[], time: number, width: number, height: number, speed: number): void {
    const motion = 0.48 + speed / 40

    // Shape the blobs into a repeating convection cycle so they rise, crown, and drip.
    for (let i = 0; i < blobs.length; i++) {
        const blob = blobs[i]
        const phase = fract(blob.phase + time * blob.cycleRate * motion)
        const laneCenter = clamp(blob.lane + Math.sin(time * 0.18 + blob.seed * TAU) * 0.035, 0.08, 0.92)
        const current = Math.sin(time * 0.92 + blob.seed * 13 + laneCenter * 8) * 0.024

        let xNorm = laneCenter
        let yNorm = 0.5
        let radiusScale = 1

        if (phase < 0.58) {
            const riseRaw = phase / 0.58
            const rise = easeOutCubic(riseRaw)
            const crownTarget = 0.22 - blob.topBias * 0.08
            const pull =
                Math.sin(riseRaw * Math.PI * (1.3 + blob.splitBias * 0.35) + blob.seed * 6.4 + time * 0.22) * blob.sway
            const columnLean = Math.sin(time * 0.24 + blob.seed * 4.7 + riseRaw * 2.6) * 0.016

            xNorm = laneCenter + pull + columnLean + current * (0.45 + rise * 0.65)
            yNorm = mix(1.08, crownTarget, rise)
            radiusScale = mix(0.74, 1.24 - blob.splitBias * 0.12, smoothstep(0.08, 0.88, riseRaw))
        } else if (phase < 0.82) {
            const crownRaw = (phase - 0.58) / 0.24
            const crown = easeInOutSine(crownRaw)
            const crownHeight = 0.22 - blob.topBias * 0.08
            const split = Math.sin(crownRaw * Math.PI)

            xNorm =
                laneCenter +
                blob.crownDrift * (0.35 + crown * 0.95) +
                Math.sin(time * 0.85 + blob.seed * 6.2 + crownRaw * 5.4) * blob.sway * 1.35
            yNorm = crownHeight - split * (0.06 + blob.topBias * 0.04)
            radiusScale = mix(1.2 - blob.splitBias * 0.1, 0.58 + blob.splitBias * 0.14, crown)
        } else {
            const fallRaw = (phase - 0.82) / 0.18
            const fall = easeInCubic(fallRaw)
            const startX = laneCenter + blob.crownDrift * 1.05
            const endX = laneCenter - blob.crownDrift * 0.25 + current

            xNorm = mix(startX, endX, easeInOutSine(fallRaw))
            yNorm = mix(0.18 + blob.topBias * 0.04, 1.1, fall)
            radiusScale = mix(0.58 + blob.splitBias * 0.14, 0.44 + blob.splitBias * 0.1, fall)
        }

        const floorPool = smoothstep(0.82, 1.02, yNorm)
        const pulse = 1 + Math.sin(time * (0.42 + blob.seed * 0.18) + blob.seed * 9.4) * 0.04

        blob.x = clamp(xNorm, 0.08, 0.92) * width
        blob.y = yNorm * height
        blob.radius = blob.baseRadius * (radiusScale + floorPool * 0.12) * pulse
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

    if (!needsResize) return { changed: false, cols, field, rows }

    return {
        changed: true,
        cols: nextCols,
        field: new Float32Array(nextCols * nextRows),
        rows: nextRows,
    }
}

function computeField(
    field: Float32Array,
    cols: number,
    rows: number,
    blobs: Blob[],
    width: number,
    height: number,
    time: number,
): void {
    const stride = cols

    for (let gy = 0; gy < rows; gy++) {
        const y = Math.min(height + STEP, gy * STEP)
        const bandWave = Math.sin(y * 0.028 + time * 1.18) * 1.8
        const basePool = smoothstep(height * 0.76, height * 1.02, y) * 0.18
        const row = gy * stride

        for (let gx = 0; gx < cols; gx++) {
            const x = Math.min(width + STEP, gx * STEP)
            const warpX = bandWave + Math.sin((x + y) * 0.02 - time * 0.85) * 1.1
            const warpY = Math.sin(y * 0.012 - time * 0.55) * 2.6 + Math.cos(x * 0.021 + time * 0.48) * 0.9
            const sampleX = x + warpX
            const sampleY = y + warpY
            let value = basePool

            for (let i = 0; i < blobs.length; i++) {
                const blob = blobs[i]
                const dx = sampleX - blob.x
                const dy = sampleY - blob.y
                const stretch = 1 + smoothstep(height * 0.16, height * 0.44, blob.y) * 0.16

                value += (blob.radius * blob.radius) / (dx * dx + dy * dy * stretch + blob.radius * 1.25)
            }

            field[row + gx] = value
        }
    }
}

function createLavaTones(colorA: Rgb, colorB: Rgb, colorC: Rgb): LavaTones {
    const shell = boostRgb(enrichRgb(mixRgb(colorA, colorB, 0.22), 0.08, -0.16), 4)
    const mid = enrichRgb(mixRgb(colorA, colorB, 0.58), 0.14, -0.03)
    const core = enrichRgb(mixRgb(colorB, colorC, 0.38), 0.16, 0.05)
    const highlight = boostRgb(enrichRgb(mixRgb(colorA, colorC, 0.66), 0.08, 0.1), 18)

    return {
        contours: [
            enrichRgb(mixRgb(shell, mid, 0.42), 0.08, -0.02),
            enrichRgb(mixRgb(mid, core, 0.48), 0.12, 0.05),
            boostRgb(enrichRgb(mixRgb(core, highlight, 0.4), 0.04, 0.1), 14),
        ],
        core,
        highlight,
        mid,
        shell,
    }
}

function drawBackdrop(
    ctx: CanvasRenderingContext2D,
    width: number,
    height: number,
    bgColor: string,
    palette: ThemePalette,
): void {
    const colorA = hexToRgb(palette.color1)
    const colorB = hexToRgb(palette.color2)
    const colorC = hexToRgb(palette.color3)

    const thermal = ctx.createLinearGradient(0, height, 0, height * 0.12)
    thermal.addColorStop(0, `rgba(${colorB.r},${colorB.g},${colorB.b},0.10)`)
    thermal.addColorStop(0.34, `rgba(${colorA.r},${colorA.g},${colorA.b},0.04)`)
    thermal.addColorStop(1, 'rgba(0,0,0,0)')
    ctx.fillStyle = thermal
    ctx.fillRect(0, 0, width, height)

    const chamber = ctx.createRadialGradient(
        width * 0.5,
        height * 0.8,
        width * 0.06,
        width * 0.5,
        height * 0.8,
        width * 0.42,
    )
    chamber.addColorStop(0, `rgba(${colorC.r},${colorC.g},${colorC.b},0.045)`)
    chamber.addColorStop(1, 'rgba(0,0,0,0)')
    ctx.fillStyle = chamber
    ctx.fillRect(0, 0, width, height)

    const shadow = ctx.createLinearGradient(0, 0, 0, height)
    shadow.addColorStop(0, hexToRgba('#000000', 0.18))
    shadow.addColorStop(0.42, hexToRgba(bgColor, 0.02))
    shadow.addColorStop(1, hexToRgba('#000000', 0.24))
    ctx.fillStyle = shadow
    ctx.fillRect(0, 0, width, height)
}

function drawLavaCells(
    ctx: CanvasRenderingContext2D,
    field: Float32Array,
    cols: number,
    rows: number,
    time: number,
    tones: LavaTones,
): void {
    const stride = cols

    for (let gy = 0; gy < rows - 1; gy++) {
        const verticalMix = gy / Math.max(1, rows - 1)

        for (let gx = 0; gx < cols - 1; gx++) {
            const idx = gy * stride + gx
            const v0 = field[idx]
            const v1 = field[idx + 1]
            const v2 = field[idx + stride + 1]
            const v3 = field[idx + stride]
            const fieldCenter = (v0 + v1 + v2 + v3) * 0.25

            if (fieldCenter < THRESHOLD) continue

            const density = smoothstep(0.82, 2.16, fieldCenter)
            const rim = smoothstep(0.86, 1.04, fieldCenter) - smoothstep(1.08, 1.34, fieldCenter)
            const body = smoothstep(0.98, 1.56, fieldCenter)
            const core = smoothstep(1.42, 2.08, fieldCenter)
            const hot = smoothstep(1.88, 2.56, fieldCenter)
            const flow = 0.5 + 0.5 * Math.sin(time * 0.95 + gx * 0.07 - gy * 0.14)
            const toneDrift = clamp(verticalMix * 0.42 + flow * 0.18 + density * 0.16, 0, 1)

            const shellTone = mixRgb(tones.shell, tones.mid, toneDrift * 0.6)
            const bodyTone = mixRgb(tones.mid, tones.core, body * 0.72)

            let tone = mixRgb(shellTone, bodyTone, body)
            tone = mixRgb(tone, tones.highlight, core * 0.38 + hot * 0.5)

            if (rim > 0.02) {
                tone = boostRgb(tone, rim * 16)
            }

            const alpha = clamp(density * (0.28 + body * 0.28 + hot * 0.16) + rim * 0.1, 0, 0.96)
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
    tones: LavaTones,
): void {
    const stride = cols
    ctx.lineCap = 'round'
    ctx.lineJoin = 'round'

    for (let li = 0; li < CONTOUR_LEVELS.length; li++) {
        const level = CONTOUR_LEVELS[li]
        const edge = tones.contours[li]

        ctx.strokeStyle = `rgba(${edge.r},${edge.g},${edge.b},${(0.17 + li * 0.08).toFixed(3)})`
        ctx.lineWidth = 0.74 + li * 0.22
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

export default canvas.stateful(
    'Lava Lamp',
    {
        bCount: [1, 18, 7],
        bgColor: '#0b0312',
        bgCycle: false,
        color1: '#16d1d9',
        color2: '#ff4fb4',
        color3: '#7d49ff',
        cycleSpeed: [1, 100, 22],
        rainbow: false,
        speed: [1, 100, 22],
        theme: THEMES,
    },
    () => {
        let blobs = createBlobs(7, 320, 200)
        let blobCount = 7
        let blobWidth = 320
        let blobHeight = 200

        let field = new Float32Array(0)
        let cols = 0
        let rows = 0

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

            const grid = ensureFieldGrid(field, cols, rows, w, h)
            if (grid.changed) {
                field = grid.field
                cols = grid.cols
                rows = grid.rows
            }

            if (bCount !== blobCount || w !== blobWidth || h !== blobHeight) {
                blobs = createBlobs(bCount, w, h)
                blobCount = bCount
                blobWidth = w
                blobHeight = h
            }

            const palette = resolvePalette(theme, color1, color2, color3)
            const bgHue = time * cycleSpeed * 1.2
            const lavaHue = time * cycleSpeed * 2.1

            const backgroundColor = bgCycle ? shiftHexHue(bgColor, bgHue) : bgColor

            ctx.fillStyle = backgroundColor
            ctx.fillRect(0, 0, w, h)
            drawBackdrop(ctx, w, h, backgroundColor, palette)

            const lavaColorA = rainbow ? shiftHexHue(palette.color1, lavaHue) : palette.color1
            const lavaColorB = rainbow ? shiftHexHue(palette.color2, lavaHue + 132) : palette.color2
            const lavaColorC = rainbow ? shiftHexHue(palette.color3, lavaHue + 264) : palette.color3

            const tones = createLavaTones(hexToRgb(lavaColorA), hexToRgb(lavaColorB), hexToRgb(lavaColorC))

            updateBlobs(blobs, time, w, h, speed)
            computeField(field, cols, rows, blobs, w, h, time)
            drawLavaCells(ctx, field, cols, rows, time, tones)
            drawContours(ctx, field, cols, rows, tones)
        }
    },
    {
        description: 'Convection-driven metaballs with cleaner contour shells, darker glass, and saturated lava cores',
    },
)
