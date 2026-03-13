import { canvas, color, combo, num } from '@hypercolor/sdk'

type ThemeName = 'Blacklight' | 'Bus Seat' | 'Laser Lime' | 'Cotton Candy' | 'Arcade Heat' | 'Custom'
type SceneName = 'Pattern 1' | 'Pattern 2' | 'Pattern 3'
type ColorMode = 'Static' | 'Color Cycle'

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

interface Palette {
    background: string
    front: string
    squiggle: string
    accent: string
}

interface Fleck {
    x: number
    y: number
    size: number
    rotation: number
    variant: number
    colorIndex: number
    drift: number
    phase: number
}

interface Ornament {
    x: number
    y: number
    size: number
    rotation: number
    variant: number
    colorIndex: number
    phase: number
    drift: number
}

interface SquigglePoint {
    x: number
    y: number
}

const THEMES: ThemeName[] = ['Blacklight', 'Bus Seat', 'Laser Lime', 'Cotton Candy', 'Arcade Heat', 'Custom']
const SCENES: SceneName[] = ['Pattern 1', 'Pattern 2', 'Pattern 3']
const COLOR_MODES: ColorMode[] = ['Static', 'Color Cycle']

const THEME_PALETTES: Record<Exclude<ThemeName, 'Custom'>, Palette> = {
    'Arcade Heat': {
        accent: '#20ecff',
        background: '#0d0406',
        front: '#ff8d1f',
        squiggle: '#ff3d7e',
    },
    Blacklight: {
        accent: '#ffb347',
        background: '#05050b',
        front: '#ff52c8',
        squiggle: '#25e7ff',
    },
    'Bus Seat': {
        accent: '#ff8c24',
        background: '#11140a',
        front: '#00d3a8',
        squiggle: '#00b8ff',
    },
    'Cotton Candy': {
        accent: '#ffb347',
        background: '#0b0811',
        front: '#ff61bf',
        squiggle: '#6af2ff',
    },
    'Laser Lime': {
        accent: '#ff6ac1',
        background: '#060b04',
        front: '#68ff42',
        squiggle: '#00f0bc',
    },
}

function clamp(value: number, min: number, max: number): number {
    return Math.max(min, Math.min(max, value))
}

function lerp(start: number, end: number, amount: number): number {
    return start + (end - start) * clamp(amount, 0, 1)
}

function wrap(value: number, max: number): number {
    if (max <= 0) return 0
    return ((value % max) + max) % max
}

function hash(value: number): number {
    const s = Math.sin(value * 127.1 + 311.7) * 43758.5453123
    return s - Math.floor(s)
}

function hexToRgb(hex: string): RGB {
    const normalized = hex.trim().replace('#', '')
    const expanded =
        normalized.length === 3
            ? normalized
                  .split('')
                  .map((char) => `${char}${char}`)
                  .join('')
            : normalized

    if (!/^[0-9a-fA-F]{6}$/.test(expanded)) {
        return { b: 255, g: 255, r: 255 }
    }

    const value = Number.parseInt(expanded, 16)
    return {
        b: value & 255,
        g: (value >> 8) & 255,
        r: (value >> 16) & 255,
    }
}

function rgbToHex(rgb: RGB): string {
    const channel = (value: number) => clamp(Math.round(value), 0, 255).toString(16).padStart(2, '0')
    return `#${channel(rgb.r)}${channel(rgb.g)}${channel(rgb.b)}`
}

function mixRgb(a: RGB, b: RGB, amount: number): RGB {
    const t = clamp(amount, 0, 1)
    return {
        b: a.b + (b.b - a.b) * t,
        g: a.g + (b.g - a.g) * t,
        r: a.r + (b.r - a.r) * t,
    }
}

function mixHex(a: string, b: string, amount: number): string {
    return rgbToHex(mixRgb(hexToRgb(a), hexToRgb(b), amount))
}

function rgbToHsl(rgb: RGB): HSL {
    const r = rgb.r / 255
    const g = rgb.g / 255
    const b = rgb.b / 255

    const cmin = Math.min(r, g, b)
    const cmax = Math.max(r, g, b)
    const delta = cmax - cmin

    let h = 0
    if (delta !== 0) {
        if (cmax === r) h = ((g - b) / delta) % 6
        else if (cmax === g) h = (b - r) / delta + 2
        else h = (r - g) / delta + 4
        h = Math.round(h * 60)
        if (h < 0) h += 360
    }

    const l = (cmax + cmin) / 2
    const s = delta === 0 ? 0 : delta / (1 - Math.abs(2 * l - 1))

    return {
        h,
        l: l * 100,
        s: s * 100,
    }
}

function hslToRgb(hsl: HSL): RGB {
    const h = wrap(hsl.h, 360)
    const s = clamp(hsl.s, 0, 100) / 100
    const l = clamp(hsl.l, 0, 100) / 100

    const chroma = (1 - Math.abs(2 * l - 1)) * s
    const huePrime = h / 60
    const second = chroma * (1 - Math.abs((huePrime % 2) - 1))

    let r = 0
    let g = 0
    let b = 0

    if (huePrime >= 0 && huePrime < 1) {
        r = chroma
        g = second
    } else if (huePrime < 2) {
        r = second
        g = chroma
    } else if (huePrime < 3) {
        g = chroma
        b = second
    } else if (huePrime < 4) {
        g = second
        b = chroma
    } else if (huePrime < 5) {
        r = second
        b = chroma
    } else {
        r = chroma
        b = second
    }

    const match = l - chroma / 2
    return {
        b: (b + match) * 255,
        g: (g + match) * 255,
        r: (r + match) * 255,
    }
}

function ledSafeHue(hue: number): number {
    const wrapped = wrap(hue, 360)
    if (wrapped >= 30 && wrapped < 90) {
        const t = (wrapped - 30) / 60
        return lerp(24, 120, t * t * (3 - 2 * t))
    }
    return wrapped
}

function shiftHexHue(hex: string, degrees: number): string {
    const hsl = rgbToHsl(hexToRgb(hex))
    return rgbToHex({
        ...hslToRgb({
            h: ledSafeHue(hsl.h + degrees),
            l: hsl.l,
            s: hsl.s,
        }),
    })
}

function rgba(hex: string, alpha: number): string {
    const rgb = hexToRgb(hex)
    return `rgba(${rgb.r}, ${rgb.g}, ${rgb.b}, ${clamp(alpha, 0, 1).toFixed(3)})`
}

function scalePoints(points: SquigglePoint[], ox: number, oy: number, sx: number, sy: number): SquigglePoint[] {
    return points.map((point) => ({
        x: ox + point.x * sx,
        y: oy + point.y * sy,
    }))
}

function drawPolyline(ctx: CanvasRenderingContext2D, points: SquigglePoint[], color: string, width: number): void {
    if (points.length < 2) return
    ctx.strokeStyle = color
    ctx.lineWidth = width
    ctx.lineCap = 'round'
    ctx.lineJoin = 'round'
    ctx.beginPath()
    ctx.moveTo(points[0].x, points[0].y)
    for (let index = 1; index < points.length; index++) {
        ctx.lineTo(points[index].x, points[index].y)
    }
    ctx.stroke()
}

function drawTriangle(
    ctx: CanvasRenderingContext2D,
    x: number,
    y: number,
    size: number,
    rotation: number,
    color: string,
): void {
    ctx.save()
    ctx.translate(x, y)
    ctx.rotate(rotation)
    ctx.fillStyle = color
    ctx.beginPath()
    ctx.moveTo(0, -size)
    ctx.lineTo(size * 0.88, size)
    ctx.lineTo(-size * 0.88, size)
    ctx.closePath()
    ctx.fill()
    ctx.restore()
}

function drawDiamond(
    ctx: CanvasRenderingContext2D,
    x: number,
    y: number,
    size: number,
    rotation: number,
    color: string,
): void {
    ctx.save()
    ctx.translate(x, y)
    ctx.rotate(rotation)
    ctx.fillStyle = color
    ctx.beginPath()
    ctx.moveTo(0, -size)
    ctx.lineTo(size, 0)
    ctx.lineTo(0, size)
    ctx.lineTo(-size, 0)
    ctx.closePath()
    ctx.fill()
    ctx.restore()
}

function drawRing(
    ctx: CanvasRenderingContext2D,
    x: number,
    y: number,
    radius: number,
    width: number,
    color: string,
): void {
    ctx.strokeStyle = color
    ctx.lineWidth = width
    ctx.beginPath()
    ctx.arc(x, y, radius, 0, Math.PI * 2)
    ctx.stroke()
}

function drawDash(
    ctx: CanvasRenderingContext2D,
    x: number,
    y: number,
    length: number,
    rotation: number,
    width: number,
    color: string,
): void {
    ctx.save()
    ctx.translate(x, y)
    ctx.rotate(rotation)
    ctx.strokeStyle = color
    ctx.lineWidth = width
    ctx.lineCap = 'round'
    ctx.beginPath()
    ctx.moveTo(-length * 0.5, 0)
    ctx.lineTo(length * 0.5, 0)
    ctx.stroke()
    ctx.restore()
}

function drawCapsule(
    ctx: CanvasRenderingContext2D,
    x: number,
    y: number,
    width: number,
    height: number,
    rotation: number,
    color: string,
): void {
    const radius = Math.min(width, height) * 0.5
    const halfW = width * 0.5
    const halfH = height * 0.5

    ctx.save()
    ctx.translate(x, y)
    ctx.rotate(rotation)
    ctx.fillStyle = color
    ctx.beginPath()
    ctx.moveTo(-halfW + radius, -halfH)
    ctx.lineTo(halfW - radius, -halfH)
    ctx.arc(halfW - radius, 0, radius, -Math.PI * 0.5, Math.PI * 0.5)
    ctx.lineTo(-halfW + radius, halfH)
    ctx.arc(-halfW + radius, 0, radius, Math.PI * 0.5, -Math.PI * 0.5)
    ctx.closePath()
    ctx.fill()
    ctx.restore()
}

function drawMiniSquiggle(
    ctx: CanvasRenderingContext2D,
    x: number,
    y: number,
    size: number,
    rotation: number,
    color: string,
): void {
    const points: SquigglePoint[] = [
        { x: -0.9, y: -0.2 },
        { x: -0.4, y: 0.15 },
        { x: 0.1, y: -0.1 },
        { x: 0.55, y: 0.24 },
        { x: 0.95, y: -0.18 },
    ]

    ctx.save()
    ctx.translate(x, y)
    ctx.rotate(rotation)
    ctx.scale(size, size)
    ctx.strokeStyle = color
    ctx.lineWidth = 0.25
    ctx.lineCap = 'round'
    ctx.lineJoin = 'round'
    ctx.beginPath()
    ctx.moveTo(points[0].x, points[0].y)
    for (let index = 1; index < points.length; index++) {
        ctx.lineTo(points[index].x, points[index].y)
    }
    ctx.stroke()
    ctx.restore()
}

function drawStarburst(ctx: CanvasRenderingContext2D, x: number, y: number, radius: number, color: string): void {
    ctx.strokeStyle = color
    ctx.lineWidth = Math.max(1, radius * 0.14)
    ctx.lineCap = 'round'
    for (let index = 0; index < 8; index++) {
        const angle = (Math.PI * 2 * index) / 8
        ctx.beginPath()
        ctx.moveTo(x + Math.cos(angle) * radius * 0.2, y + Math.sin(angle) * radius * 0.2)
        ctx.lineTo(x + Math.cos(angle) * radius, y + Math.sin(angle) * radius)
        ctx.stroke()
    }
}

function drawBackground(
    ctx: CanvasRenderingContext2D,
    w: number,
    h: number,
    palette: Palette,
    time: number,
    glow: number,
    motion: number,
): void {
    ctx.fillStyle = palette.background
    ctx.fillRect(0, 0, w, h)

    ctx.save()
    ctx.globalCompositeOperation = 'lighter'

    const glowCenterX = w * (0.34 + Math.sin(time * (0.08 + motion * 0.06)) * 0.08)
    const glowCenterY = h * (0.3 + Math.cos(time * (0.07 + motion * 0.05)) * 0.08)
    const glowRadius = Math.max(w, h) * (0.72 + glow * 0.12)

    const halo = ctx.createRadialGradient(glowCenterX, glowCenterY, 0, glowCenterX, glowCenterY, glowRadius)
    halo.addColorStop(0, rgba(mixHex(palette.front, palette.background, 0.38), 0.05 + glow * 0.05))
    halo.addColorStop(0.55, rgba(mixHex(palette.squiggle, palette.background, 0.56), 0.025 + glow * 0.03))
    halo.addColorStop(1, 'rgba(0, 0, 0, 0)')
    ctx.fillStyle = halo
    ctx.fillRect(0, 0, w, h)

    const sweep = ctx.createLinearGradient(0, 0, w, h)
    sweep.addColorStop(0, rgba(mixHex(palette.accent, palette.background, 0.62), 0.02 + glow * 0.02))
    sweep.addColorStop(0.48, 'rgba(0, 0, 0, 0)')
    sweep.addColorStop(1, rgba(mixHex(palette.squiggle, palette.background, 0.66), 0.018 + glow * 0.02))
    ctx.fillStyle = sweep
    ctx.fillRect(0, 0, w, h)

    ctx.restore()
}

function getBasePalette(controls: Record<string, unknown>): Palette {
    const theme = controls.theme as ThemeName
    if (theme === 'Custom') {
        return {
            accent: controls.accentColor as string,
            background: controls.backgroundColor as string,
            front: controls.frontColor as string,
            squiggle: controls.squiggleColor as string,
        }
    }

    return THEME_PALETTES[theme]
}

function getActivePalette(controls: Record<string, unknown>, time: number): Palette {
    const base = getBasePalette(controls)
    const colorMode = controls.colorMode as ColorMode
    if (colorMode !== 'Color Cycle') return base

    const cycleSpeed = controls.cycleSpeed as number
    const shift = time * (1.2 + cycleSpeed * 0.18)
    return {
        accent: shiftHexHue(base.accent, shift - 90),
        background: shiftHexHue(base.background, shift * 0.2 - 24),
        front: shiftHexHue(base.front, shift),
        squiggle: shiftHexHue(base.squiggle, shift + 100),
    }
}

function buildFlecks(count: number): Fleck[] {
    return Array.from({ length: count }, (_, index) => ({
        colorIndex: Math.floor(hash(index * 4.41 + 5.3) * 3),
        drift: 1.2 + hash(index * 5.31 + 2.2) * 4.4,
        phase: hash(index * 7.19 + 8.1) * Math.PI * 2,
        rotation: hash(index * 2.31 + 7.4) * Math.PI * 2,
        size: 0.8 + hash(index * 1.71 + 2.3) * 2.1,
        variant: Math.floor(hash(index * 3.07 + 4.8) * 5),
        x: 0.04 + hash(index * 0.83 + 1.1) * 0.92,
        y: 0.05 + hash(index * 1.21 + 6.2) * 0.9,
    }))
}

function buildOrnaments(count: number): Ornament[] {
    return Array.from({ length: count }, (_, index) => ({
        colorIndex: Math.floor(hash(index * 5.81 + 7.7) * 3),
        drift: 8 + hash(index * 7.43 + 8.9) * 14,
        phase: hash(index * 6.71 + 1.7) * Math.PI * 2,
        rotation: hash(index * 3.71 + 9.5) * Math.PI * 2,
        size: 8 + hash(index * 2.93 + 6.1) * 18,
        variant: Math.floor(hash(index * 4.13 + 2.9) * 5),
        x: 0.1 + hash(index * 0.91 + 2.4) * 0.8,
        y: 0.12 + hash(index * 1.47 + 4.2) * 0.74,
    }))
}

function drawCarpetFlecks(
    ctx: CanvasRenderingContext2D,
    w: number,
    h: number,
    flecks: Fleck[],
    palette: Palette,
    time: number,
    moveScale: number,
): void {
    const colors = [palette.front, palette.squiggle, palette.accent]

    for (const fleck of flecks) {
        const swayX =
            Math.sin(time * (0.12 + fleck.drift * 0.015) + fleck.phase) * fleck.drift * (0.25 + moveScale * 0.45)
        const swayY =
            Math.cos(time * (0.1 + fleck.drift * 0.012) + fleck.phase * 1.2) * fleck.drift * (0.16 + moveScale * 0.28)
        const x = clamp(fleck.x * w + swayX, 2, w - 2)
        const y = clamp(fleck.y * h + swayY, 2, h - 2)
        const color = colors[fleck.colorIndex] ?? palette.front
        const size = fleck.size * (0.88 + 0.16 * Math.sin(time * 0.2 + fleck.phase))
        const alpha = 0.38 + 0.14 * Math.sin(time * 0.24 + fleck.phase * 0.9)
        const ink = rgba(color, alpha)

        if (fleck.variant === 0) {
            ctx.fillStyle = ink
            ctx.fillRect(x, y, size, size)
        } else if (fleck.variant === 1) {
            drawDash(ctx, x, y, size * 2.5, fleck.rotation, Math.max(1, size * 0.45), ink)
        } else if (fleck.variant === 2) {
            ctx.fillStyle = ink
            ctx.beginPath()
            ctx.arc(x, y, size * 0.5, 0, Math.PI * 2)
            ctx.fill()
        } else if (fleck.variant === 3) {
            drawTriangle(ctx, x, y, size * 0.75, fleck.rotation, ink)
        } else {
            drawRing(ctx, x, y, size * 0.7, Math.max(1, size * 0.28), ink)
        }
    }
}

function drawOrnaments(
    ctx: CanvasRenderingContext2D,
    w: number,
    h: number,
    ornaments: Ornament[],
    palette: Palette,
    time: number,
    scene: SceneName,
    moveScale: number,
): void {
    const colors = [palette.front, palette.squiggle, palette.accent]
    const driftMultiplier = scene === 'Pattern 1' ? 0.8 : scene === 'Pattern 2' ? 1 : 0.65

    for (const ornament of ornaments) {
        const orbitX =
            Math.sin(time * (0.11 + ornament.drift * 0.004) + ornament.phase) *
            ornament.drift *
            driftMultiplier *
            (0.18 + moveScale * 0.34)
        const orbitY =
            Math.cos(time * (0.09 + ornament.drift * 0.003) + ornament.phase * 1.2) *
            ornament.drift *
            0.8 *
            driftMultiplier *
            (0.16 + moveScale * 0.28)
        const size = ornament.size * (0.84 + 0.1 * Math.sin(time * 0.22 + ornament.phase))
        const x = clamp(ornament.x * w + orbitX, size, w - size)
        const y = clamp(ornament.y * h + orbitY, size, h - size)
        const color = colors[ornament.colorIndex] ?? palette.front
        const ink = rgba(color, 0.62)

        if (ornament.variant === 0) {
            drawTriangle(ctx, x, y, size * 0.38, ornament.rotation, ink)
        } else if (ornament.variant === 1) {
            drawDiamond(ctx, x, y, size * 0.36, ornament.rotation, ink)
        } else if (ornament.variant === 2) {
            drawRing(ctx, x, y, size * 0.3, Math.max(1.2, size * 0.08), ink)
        } else if (ornament.variant === 3) {
            drawDash(ctx, x, y, size * 0.9, ornament.rotation, Math.max(1.8, size * 0.09), ink)
        } else {
            drawStarburst(ctx, x, y, size * 0.34, ink)
        }
    }
}

function drawPatternOne(
    ctx: CanvasRenderingContext2D,
    w: number,
    h: number,
    palette: Palette,
    time: number,
    moveScale: number,
    glow: number,
): void {
    const sx = w / 320
    const sy = h / 200
    const scale = Math.min(sx, sy)
    const lineWidth = Math.max(8, 16 * scale)
    const bandY = [0.18, 0.46, 0.74]

    const rows = [
        [
            { x: 0, y: 0 },
            { x: 40, y: 2 },
            { x: 30, y: 45 },
            { x: 10, y: 35 },
            { x: 30, y: 110 },
        ],
        [
            { x: 75, y: 5 },
            { x: 60, y: 75 },
            { x: 90, y: 85 },
            { x: 105, y: 40 },
            { x: 140, y: 40 },
            { x: 120, y: 120 },
            { x: 90, y: 120 },
            { x: 120, y: 150 },
        ],
        [
            { x: 145, y: 150 },
            { x: 185, y: 10 },
            { x: 220, y: 35 },
            { x: 200, y: 100 },
            { x: 280, y: 80 },
            { x: 305, y: 15 },
        ],
        [
            { x: 320, y: -55 },
            { x: 305, y: -15 },
            { x: 235, y: 0 },
            { x: 270, y: 20 },
            { x: 250, y: 60 },
        ],
    ]

    for (let bandIndex = 0; bandIndex < bandY.length; bandIndex++) {
        const y = h * bandY[bandIndex] + Math.sin(time * (0.26 + moveScale * 0.4) + bandIndex * 1.1) * h * 0.03
        const xOffset = Math.sin(time * (0.18 + moveScale * 0.24) + bandIndex * 0.7) * 22 * scale

        for (let rowIndex = 0; rowIndex < rows.length; rowIndex++) {
            const points = scalePoints(rows[rowIndex], xOffset, y, sx, sy)
            drawPolyline(ctx, points, rgba(palette.squiggle, 0.1 + glow * 0.06), lineWidth * 1.55)
            drawPolyline(ctx, points, palette.squiggle, lineWidth)
        }
    }

    const upwardLanes = [0.18 * w, 0.5 * w, 0.82 * w]
    for (let laneIndex = 0; laneIndex < upwardLanes.length; laneIndex++) {
        const laneX = upwardLanes[laneIndex]
        const laneSlant = laneIndex % 2 === 0 ? -1 : 1

        for (let item = 0; item < 5; item++) {
            const anchor = h * lerp(0.18, 0.82, item / 4)
            const y = anchor + Math.sin(time * (0.34 + moveScale * 0.46) + laneIndex * 0.7 + item * 0.8) * h * 0.028
            const x = laneX + laneSlant * Math.cos(time * (0.22 + moveScale * 0.3) + item * 0.9) * w * 0.018
            const color = item % 2 === 0 ? palette.front : palette.accent

            if (item % 3 === 0) {
                drawCapsule(ctx, x, y, 18 * sx, 54 * sy, laneSlant * 0.26, color)
            } else if (item % 3 === 1) {
                drawRing(ctx, x, y, 16 * sy, Math.max(2, 5 * scale), color)
            } else {
                drawDiamond(ctx, x, y, 18 * sy, Math.PI * 0.25, color)
            }
        }
    }
}

function drawPatternTwo(
    ctx: CanvasRenderingContext2D,
    w: number,
    h: number,
    palette: Palette,
    time: number,
    moveScale: number,
    density: number,
    glow: number,
): void {
    const densityMix = clamp(density / 100, 0, 1)
    const squiggleCount = Math.floor(5 + densityMix * 5)

    for (let index = 0; index < squiggleCount; index++) {
        const baseX = (index + 0.5) * (w / squiggleCount)
        const y =
            h * lerp(0.18, 0.82, index / Math.max(1, squiggleCount - 1)) +
            Math.sin(time * (0.4 + moveScale * 0.36) + index * 0.9) * h * 0.05
        const size = 8 + (index % 3) * 4
        drawMiniSquiggle(
            ctx,
            baseX + Math.sin(time * (0.46 + moveScale * 0.34) + index) * w * 0.018,
            y,
            size,
            index % 3 === 0 ? -0.2 : index % 3 === 1 ? 0.4 : 0,
            rgba(palette.squiggle, 0.66),
        )
    }

    const conveyors = [
        { angle: Math.PI * 0.22, colorA: palette.front, colorB: palette.accent, shape: 'capsule' },
        { angle: Math.PI * 1.22, colorA: palette.accent, colorB: palette.front, shape: 'triangle' },
    ] as const

    for (const [laneIndex, lane] of conveyors.entries()) {
        ctx.save()
        ctx.translate(w * 0.5, h * 0.5)
        ctx.rotate(lane.angle)
        ctx.translate(-w * 0.5, -h * 0.5)

        for (let item = 0; item < 7; item++) {
            const base = item / 6
            const travel = Math.sin(time * (0.26 + moveScale * 0.24) + item * 0.8 + laneIndex * 0.6) * 0.07
            const progress = clamp(base + travel, 0.08, 0.92)
            const x = lerp(w * 0.12, w * 0.88, progress)
            const y = h * 0.5 + Math.sin(time * 0.34 + item * 0.7 + laneIndex) * h * 0.018
            const isAccent = item % 2 === 0
            const color = isAccent ? lane.colorA : lane.colorB

            if (lane.shape === 'capsule') {
                drawCapsule(ctx, x, y, 30, 98, 0, rgba(color, 0.78))
                drawCapsule(ctx, x, y, 14, 72, 0, rgba(isAccent ? lane.colorB : lane.colorA, 0.7))
            } else {
                drawTriangle(ctx, x, y + 12, 30, Math.PI, rgba(color, 0.76))
                drawTriangle(ctx, x + 4, y + 20, 20, Math.PI * 0.88, rgba(isAccent ? lane.colorB : lane.colorA, 0.68))
            }
        }

        ctx.restore()
    }

    drawRing(
        ctx,
        w * 0.5,
        h * 0.5,
        Math.min(w, h) * 0.16,
        Math.max(4, Math.min(w, h) * 0.022),
        rgba(palette.squiggle, 0.1 + glow * 0.06),
    )
}

function drawRibbon(
    ctx: CanvasRenderingContext2D,
    w: number,
    baseline: number,
    amplitude: number,
    thickness: number,
    color: string,
    time: number,
    phase: number,
): void {
    const points: SquigglePoint[] = []
    const steps = 18

    for (let index = 0; index <= steps; index++) {
        const x = (index / steps) * w
        const wave = Math.sin((x / w) * Math.PI * 3.5 + time * 1.2 + phase) * amplitude
        const ripple = Math.sin((x / w) * Math.PI * 8.5 - time * 0.8 + phase * 0.7) * amplitude * 0.34
        points.push({
            x,
            y: baseline + wave + ripple,
        })
    }

    ctx.strokeStyle = color
    ctx.lineWidth = thickness
    ctx.lineCap = 'round'
    ctx.lineJoin = 'round'
    ctx.beginPath()
    ctx.moveTo(points[0].x, points[0].y)

    for (let index = 1; index < points.length - 1; index++) {
        const midX = (points[index].x + points[index + 1].x) * 0.5
        const midY = (points[index].y + points[index + 1].y) * 0.5
        ctx.quadraticCurveTo(points[index].x, points[index].y, midX, midY)
    }

    const last = points[points.length - 1]
    ctx.lineTo(last.x, last.y)
    ctx.stroke()
}

function drawPatternThree(
    ctx: CanvasRenderingContext2D,
    w: number,
    h: number,
    palette: Palette,
    time: number,
    moveScale: number,
    glow: number,
): void {
    const flow = time * (0.34 + moveScale * 0.52)

    drawRibbon(ctx, w, h * 0.42, h * 0.09, Math.max(26, h * 0.15), rgba(palette.front, 0.1 + glow * 0.05), flow, 0)
    drawRibbon(ctx, w, h * 0.42, h * 0.09, Math.max(17, h * 0.1), palette.front, flow, 0)
    drawRibbon(ctx, w, h * 0.42, h * 0.09, Math.max(7, h * 0.042), palette.accent, flow, 0.7)
    drawRibbon(ctx, w, h * 0.68, h * 0.08, Math.max(22, h * 0.13), rgba(palette.squiggle, 0.1 + glow * 0.05), flow, 1.4)
    drawRibbon(ctx, w, h * 0.68, h * 0.08, Math.max(14, h * 0.085), palette.squiggle, flow, 1.4)
    drawRibbon(ctx, w, h * 0.68, h * 0.08, Math.max(6, h * 0.034), palette.accent, flow, 2.1)

    const dotCount = 8
    for (let index = 0; index < dotCount; index++) {
        const baseProgress = index / Math.max(1, dotCount - 1)
        const x =
            lerp(w * 0.08, w * 0.92, baseProgress) +
            Math.sin(time * (0.28 + moveScale * 0.22) + index * 0.9) * w * 0.016
        const y = (index % 2 === 0 ? h * 0.22 : h * 0.84) + Math.sin(time * 0.48 + index) * (h * 0.03)
        const color = index % 2 === 0 ? palette.accent : palette.front
        if (index % 3 === 0) {
            drawRing(ctx, x, y, Math.max(10, h * 0.04), Math.max(2, h * 0.01), color)
        } else if (index % 3 === 1) {
            drawDiamond(ctx, x, y, Math.max(10, h * 0.042), Math.PI * 0.25, color)
        } else {
            drawTriangle(ctx, x, y, Math.max(10, h * 0.045), Math.PI * 0.12, color)
        }
    }
}

export default canvas.stateful(
    'Roller Rink',
    {
        theme: combo('Palette', THEMES, { default: 'Blacklight', group: 'Scene' }),
        scene: combo('Layout', SCENES, { default: 'Pattern 3', group: 'Scene' }),
        density: num('Decor Density', [0, 100], 42, { group: 'Scene' }),
        colorMode: combo('Palette Motion', COLOR_MODES, { default: 'Static', group: 'Color' }),
        cycleSpeed: num('Color Drift', [0, 100], 22, { group: 'Color' }),
        frontColor: color('Primary Color', '#ff52c8', { group: 'Color' }),
        squiggleColor: color('Line Color', '#25e7ff', { group: 'Color' }),
        accentColor: color('Accent Color', '#ffb347', { group: 'Color' }),
        backgroundColor: color('Backdrop Color', '#05050b', { group: 'Color' }),
        moveSpeed: num('Motion', [0, 100], 24, { group: 'Motion' }),
        glow: num('Glow', [0, 100], 34, { group: 'Motion' }),
    },
    () => {
        let flecks: Fleck[] = []
        let ornaments: Ornament[] = []
        let lastDensity = -1

        function reseed(density: number): void {
            const fleckCount = Math.floor(20 + density * 0.46)
            const ornamentCount = Math.floor(4 + density * 0.08)
            flecks = buildFlecks(fleckCount)
            ornaments = buildOrnaments(ornamentCount)
            lastDensity = density
        }

        return (ctx, time, controls) => {
            const width = ctx.canvas.width
            const height = ctx.canvas.height
            const density = controls.density as number
            const glow = clamp((controls.glow as number) / 100, 0, 1)

            if (flecks.length === 0 || ornaments.length === 0 || density !== lastDensity) {
                reseed(density)
            }

            const moveScale = clamp((controls.moveSpeed as number) / 100, 0, 1)
            const scene = controls.scene as SceneName
            const palette = getActivePalette(controls as Record<string, unknown>, time)

            drawBackground(ctx, width, height, palette, time, glow, moveScale)
            drawCarpetFlecks(ctx, width, height, flecks, palette, time, moveScale)
            drawOrnaments(ctx, width, height, ornaments, palette, time, scene, moveScale)

            if (scene === 'Pattern 1') {
                drawPatternOne(ctx, width, height, palette, time, moveScale, glow)
            } else if (scene === 'Pattern 2') {
                drawPatternTwo(ctx, width, height, palette, time, moveScale, density, glow)
            } else {
                drawPatternThree(ctx, width, height, palette, time, moveScale, glow)
            }
        }
    },
    {
        author: 'Hypercolor',
        description:
            'Step onto blacklight carpet geometry — retro arcade patterns glow under ultraviolet, pulsing and shifting in warm nostalgic haze',
        presets: [
            {
                controls: {
                    accentColor: '#ffb347',
                    backgroundColor: '#05050b',
                    colorMode: 'Static',
                    cycleSpeed: 0,
                    density: 72,
                    frontColor: '#ff52c8',
                    glow: 55,
                    moveSpeed: 18,
                    scene: 'Pattern 1',
                    squiggleColor: '#25e7ff',
                    theme: 'Blacklight',
                },
                description:
                    'Crusty 1987 roller rink carpet under full UV — neon triangles, mystery stains, and pure nostalgic magic',
                name: 'Carpet Burns & Arcade Tokens',
            },
            {
                controls: {
                    accentColor: '#ffb347',
                    backgroundColor: '#0b0811',
                    colorMode: 'Color Cycle',
                    cycleSpeed: 68,
                    density: 95,
                    frontColor: '#ff61bf',
                    glow: 78,
                    moveSpeed: 40,
                    scene: 'Pattern 2',
                    squiggleColor: '#6af2ff',
                    theme: 'Cotton Candy',
                },
                description:
                    'Saturated color cycling floods dense geometric confetti — the visual equivalent of a dolphin sticker sheet',
                name: 'Lisa Frank Trapper Keeper',
            },
            {
                controls: {
                    accentColor: '#ff8c24',
                    backgroundColor: '#11140a',
                    colorMode: 'Static',
                    cycleSpeed: 0,
                    density: 28,
                    frontColor: '#00d3a8',
                    glow: 22,
                    moveSpeed: 8,
                    scene: 'Pattern 3',
                    squiggleColor: '#00b8ff',
                    theme: 'Bus Seat',
                },
                description:
                    'Precisely placed pastels drift with deliberate symmetry — every shape knows exactly where it belongs',
                name: 'Wes Anderson Lobby',
            },
            {
                controls: {
                    accentColor: '#ff6ac1',
                    backgroundColor: '#060b04',
                    colorMode: 'Color Cycle',
                    cycleSpeed: 44,
                    density: 58,
                    frontColor: '#68ff42',
                    glow: 90,
                    moveSpeed: 62,
                    scene: 'Pattern 1',
                    squiggleColor: '#00f0bc',
                    theme: 'Laser Lime',
                },
                description:
                    'Acid green squiggles and hot pink geometry vibrate in the dark — smells like fog machine and victory',
                name: 'Laser Tag Aftermath',
            },
            {
                controls: {
                    accentColor: '#20ecff',
                    backgroundColor: '#0d0406',
                    colorMode: 'Static',
                    cycleSpeed: 0,
                    density: 35,
                    frontColor: '#ff8d1f',
                    glow: 45,
                    moveSpeed: 4,
                    scene: 'Pattern 2',
                    squiggleColor: '#ff3d7e',
                    theme: 'Arcade Heat',
                },
                description:
                    'Warm arcade heat patterns hover in place — the exhausted glow of fast food neon through rain-streaked glass',
                name: 'Taco Bell 2am',
            },
            {
                controls: {
                    accentColor: '#ffb347',
                    backgroundColor: '#0b0811',
                    colorMode: 'Color Cycle',
                    cycleSpeed: 100,
                    density: 100,
                    frontColor: '#ff61bf',
                    glow: 100,
                    moveSpeed: 88,
                    scene: 'Pattern 3',
                    squiggleColor: '#6af2ff',
                    theme: 'Cotton Candy',
                },
                description:
                    'Every surface screams with color-shifting ribbons and maximum confetti — a birthday party inside a kaleidoscope',
                name: 'Dopamine Rush',
            },
            {
                controls: {
                    accentColor: '#ff8c24',
                    backgroundColor: '#11140a',
                    colorMode: 'Static',
                    cycleSpeed: 0,
                    density: 0,
                    frontColor: '#00d3a8',
                    glow: 12,
                    moveSpeed: 2,
                    scene: 'Pattern 1',
                    squiggleColor: '#00b8ff',
                    theme: 'Bus Seat',
                },
                description:
                    'Bare geometric bones float on a near-black canvas — the rink after closing, lights dimmed, last song echoing',
                name: 'Closing Time',
            },
        ],
    },
)
