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
    Blacklight: {
        background: '#05050b',
        front: '#ff52c8',
        squiggle: '#25e7ff',
        accent: '#f3f14e',
    },
    'Bus Seat': {
        background: '#ccc000',
        front: '#00cc93',
        squiggle: '#00addb',
        accent: '#111111',
    },
    'Laser Lime': {
        background: '#060b04',
        front: '#a7ff2e',
        squiggle: '#00f0bc',
        accent: '#ff67db',
    },
    'Cotton Candy': {
        background: '#0b0811',
        front: '#ff61bf',
        squiggle: '#6af2ff',
        accent: '#ffe65e',
    },
    'Arcade Heat': {
        background: '#0d0406',
        front: '#ff8d1f',
        squiggle: '#ff3d7e',
        accent: '#20ecff',
    },
}

function clamp(value: number, min: number, max: number): number {
    return Math.max(min, Math.min(max, value))
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
    const expanded = normalized.length === 3
        ? normalized.split('').map((char) => `${char}${char}`).join('')
        : normalized

    if (!/^[0-9a-fA-F]{6}$/.test(expanded)) {
        return { r: 255, g: 255, b: 255 }
    }

    const value = Number.parseInt(expanded, 16)
    return {
        r: (value >> 16) & 255,
        g: (value >> 8) & 255,
        b: value & 255,
    }
}

function rgbToHex(rgb: RGB): string {
    const channel = (value: number) => clamp(Math.round(value), 0, 255).toString(16).padStart(2, '0')
    return `#${channel(rgb.r)}${channel(rgb.g)}${channel(rgb.b)}`
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
        s: s * 100,
        l: l * 100,
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
        r: (r + match) * 255,
        g: (g + match) * 255,
        b: (b + match) * 255,
    }
}

function shiftHexHue(hex: string, degrees: number): string {
    const hsl = rgbToHsl(hexToRgb(hex))
    return rgbToHex({
        ...hslToRgb({
            h: hsl.h + degrees,
            s: hsl.s,
            l: hsl.l,
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

function drawPolyline(
    ctx: CanvasRenderingContext2D,
    points: SquigglePoint[],
    color: string,
    width: number,
): void {
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

function drawStarburst(
    ctx: CanvasRenderingContext2D,
    x: number,
    y: number,
    radius: number,
    color: string,
): void {
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

function drawBackground(ctx: CanvasRenderingContext2D, w: number, h: number, color: string): void {
    ctx.fillStyle = color
    ctx.fillRect(0, 0, w, h)
}

function getBasePalette(controls: Record<string, unknown>): Palette {
    const theme = controls.theme as ThemeName
    if (theme === 'Custom') {
        return {
            background: controls.backgroundColor as string,
            front: controls.frontColor as string,
            squiggle: controls.squiggleColor as string,
            accent: controls.accentColor as string,
        }
    }

    return THEME_PALETTES[theme]
}

function getActivePalette(controls: Record<string, unknown>, time: number): Palette {
    const base = getBasePalette(controls)
    const colorMode = controls.colorMode as ColorMode
    if (colorMode !== 'Color Cycle') return base

    const cycleSpeed = controls.cycleSpeed as number
    const shift = time * (6 + cycleSpeed * 1.2)
    return {
        background: shiftHexHue(base.background, shift * 0.2 - 24),
        front: shiftHexHue(base.front, shift),
        squiggle: shiftHexHue(base.squiggle, shift + 100),
        accent: shiftHexHue(base.accent, shift - 90),
    }
}

function buildFlecks(count: number): Fleck[] {
    return Array.from({ length: count }, (_, index) => ({
        x: hash(index * 0.83 + 1.1),
        y: hash(index * 1.21 + 6.2),
        size: 0.6 + hash(index * 1.71 + 2.3) * 3.6,
        rotation: hash(index * 2.31 + 7.4) * Math.PI * 2,
        variant: Math.floor(hash(index * 3.07 + 4.8) * 5),
        colorIndex: Math.floor(hash(index * 4.41 + 5.3) * 3),
        drift: 0.8 + hash(index * 5.31 + 2.2) * 1.6,
        phase: hash(index * 7.19 + 8.1) * Math.PI * 2,
    }))
}

function buildOrnaments(count: number): Ornament[] {
    return Array.from({ length: count }, (_, index) => ({
        x: hash(index * 0.91 + 2.4),
        y: hash(index * 1.47 + 4.2),
        size: 6 + hash(index * 2.93 + 6.1) * 26,
        rotation: hash(index * 3.71 + 9.5) * Math.PI * 2,
        variant: Math.floor(hash(index * 4.13 + 2.9) * 5),
        colorIndex: Math.floor(hash(index * 5.81 + 7.7) * 3),
        phase: hash(index * 6.71 + 1.7) * Math.PI * 2,
        drift: 4 + hash(index * 7.43 + 8.9) * 16,
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
        const x = wrap(fleck.x * w + Math.sin(time * 0.22 + fleck.phase) * fleck.drift * moveScale * 2.2, w)
        const y = wrap(fleck.y * h + Math.cos(time * 0.19 + fleck.phase) * fleck.drift * moveScale * 1.8, h)
        const color = colors[fleck.colorIndex] ?? palette.front

        if (fleck.variant === 0) {
            ctx.fillStyle = color
            ctx.fillRect(x, y, fleck.size, fleck.size)
        } else if (fleck.variant === 1) {
            drawDash(ctx, x, y, fleck.size * 2.8, fleck.rotation, Math.max(1, fleck.size * 0.55), color)
        } else if (fleck.variant === 2) {
            ctx.fillStyle = color
            ctx.beginPath()
            ctx.arc(x, y, fleck.size * 0.6, 0, Math.PI * 2)
            ctx.fill()
        } else if (fleck.variant === 3) {
            drawTriangle(ctx, x, y, fleck.size * 0.85, fleck.rotation, color)
        } else {
            drawRing(ctx, x, y, fleck.size * 0.7, Math.max(1, fleck.size * 0.28), color)
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
    const driftMultiplier = scene === 'Pattern 1' ? 1.4 : scene === 'Pattern 2' ? 1.1 : 0.75

    for (const ornament of ornaments) {
        const x = wrap(
            ornament.x * w + Math.sin(time * 0.16 + ornament.phase) * ornament.drift * driftMultiplier * moveScale,
            w,
        )
        const y = wrap(
            ornament.y * h + Math.cos(time * 0.13 + ornament.phase) * ornament.drift * 0.7 * driftMultiplier * moveScale,
            h,
        )
        const color = colors[ornament.colorIndex] ?? palette.front
        const size = ornament.size * (0.75 + 0.25 * Math.sin(time * 0.25 + ornament.phase))

        if (ornament.variant === 0) {
            drawTriangle(ctx, x, y, size * 0.45, ornament.rotation, color)
        } else if (ornament.variant === 1) {
            drawDiamond(ctx, x, y, size * 0.42, ornament.rotation, color)
        } else if (ornament.variant === 2) {
            drawRing(ctx, x, y, size * 0.35, Math.max(1.4, size * 0.09), color)
        } else if (ornament.variant === 3) {
            drawDash(ctx, x, y, size, ornament.rotation, Math.max(2, size * 0.1), color)
        } else {
            drawStarburst(ctx, x, y, size * 0.42, color)
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
): void {
    const sx = w / 320
    const sy = h / 200
    const lineWidth = Math.max(8, 18 * Math.min(sx, sy))
    const bandPeriod = 220 * sy
    const bandY = [
        wrap(h + 36 - time * moveScale * 30, bandPeriod) - 150 * sy,
        wrap(h - 38 - time * moveScale * 30, bandPeriod) - 150 * sy,
        wrap(h - 114 - time * moveScale * 30, bandPeriod) - 150 * sy,
    ]

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

    for (const y of bandY) {
        for (const row of rows) {
            drawPolyline(ctx, scalePoints(row, 0, y, sx, sy), palette.squiggle, lineWidth)
        }
    }

    const upwardLanes = [0.44 * w, -0.05 * w, 0.94 * w]
    for (let laneIndex = 0; laneIndex < upwardLanes.length; laneIndex++) {
        const laneX = upwardLanes[laneIndex]
        const lanePhase = laneIndex * 0.18

        ctx.save()
        ctx.translate(laneX, h * 0.6)
        ctx.rotate(0.3)
        ctx.translate(-laneX, -h * 0.6)

        for (let item = 0; item < 7; item++) {
            const progress = wrap(time * moveScale * 0.13 + lanePhase + item * 0.21, 1)
            const y = h * 1.2 - progress * (h + 200 * sy)
            const color = item % 2 === 0 ? palette.front : palette.accent
            if (item % 2 === 0) {
                ctx.fillStyle = color
                ctx.beginPath()
                ctx.arc(laneX + (item % 3 === 0 ? 10 * sx : -10 * sx), y, 26 * sy, 0, Math.PI * 2)
                ctx.fill()
            } else {
                ctx.fillStyle = color
                ctx.fillRect(laneX - 12 * sx, y - 50 * sy, 24 * sx, 100 * sy)
            }
        }

        ctx.restore()
    }

    const downwardLanes = [0.44 * w, 0.94 * w]
    for (let laneIndex = 0; laneIndex < downwardLanes.length; laneIndex++) {
        const laneX = downwardLanes[laneIndex]
        const lanePhase = laneIndex * 0.26

        ctx.save()
        ctx.translate(laneX, h * 0.5)
        ctx.rotate(0.3)
        ctx.translate(-laneX, -h * 0.5)

        for (let item = 0; item < 6; item++) {
            const progress = wrap(time * moveScale * 0.11 + lanePhase + item * 0.22, 1)
            const y = -80 * sy + progress * (h + 180 * sy)
            const color = item % 2 === 0 ? palette.front : palette.accent
            if (item % 2 === 0) {
                drawTriangle(ctx, laneX + (item % 3 === 0 ? 20 * sx : -20 * sx), y, 34 * sy, Math.PI, color)
            } else {
                drawDiamond(ctx, laneX, y, 34 * sy, Math.PI * 0.25, color)
            }
        }

        ctx.restore()
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
): void {
    const squiggleCount = Math.floor(10 + density * 0.12)
    const laneTravel = time * moveScale * 55

    for (let index = 0; index < squiggleCount; index++) {
        const baseX = (index + 0.5) * (w / squiggleCount)
        const direction = index % 2 === 0 ? 1 : -1
        const y = wrap(direction > 0 ? laneTravel + index * 14 : -laneTravel + index * 14, h + 80) - 40
        const size = 8 + (index % 3) * 5
        drawMiniSquiggle(
            ctx,
            baseX + Math.sin(time * 0.7 + index) * 4,
            y,
            size,
            index % 3 === 0 ? -0.2 : index % 3 === 1 ? 0.4 : 0,
            palette.squiggle,
        )
    }

    const conveyors = [
        { angle: Math.PI * 0.25, direction: 1, colorA: palette.front, colorB: palette.accent, shape: 'rect' },
        { angle: Math.PI * 1.25, direction: 1, colorA: palette.accent, colorB: palette.front, shape: 'triangle' },
    ] as const

    for (const [laneIndex, lane] of conveyors.entries()) {
        ctx.save()
        ctx.translate(w * 0.5, h * 0.5)
        ctx.rotate(lane.angle)
        ctx.translate(-w * 0.5, -h * 0.5)

        for (let item = 0; item < 10; item++) {
            const progress = wrap(time * moveScale * 0.11 + item * 0.13 + laneIndex * 0.09, 1)
            const x = -0.35 * w + progress * (w * 1.7)
            const y = h * 0.5 + Math.sin(time * 0.5 + item) * 4
            const isAccent = item % 2 === 0
            const color = isAccent ? lane.colorA : lane.colorB

            if (lane.shape === 'rect') {
                ctx.fillStyle = color
                ctx.fillRect(x - 18, y - 60, 36, 120)
                ctx.fillStyle = isAccent ? lane.colorB : lane.colorA
                ctx.fillRect(x - 13, y - 55, 26, 110)
            } else {
                drawTriangle(ctx, x, y + 18, 38, Math.PI, color)
                drawTriangle(ctx, x + 6, y + 28, 28, Math.PI * 0.88, isAccent ? lane.colorB : lane.colorA)
            }
        }

        ctx.restore()
    }
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
): void {
    drawRibbon(ctx, w, h * 0.42, h * 0.1, Math.max(18, h * 0.11), palette.front, time * moveScale, 0)
    drawRibbon(ctx, w, h * 0.42, h * 0.1, Math.max(8, h * 0.045), palette.accent, time * moveScale, 0.7)
    drawRibbon(ctx, w, h * 0.68, h * 0.09, Math.max(16, h * 0.095), palette.squiggle, time * moveScale, 1.4)
    drawRibbon(ctx, w, h * 0.68, h * 0.09, Math.max(7, h * 0.038), palette.accent, time * moveScale, 2.1)

    const dotCount = 8
    for (let index = 0; index < dotCount; index++) {
        const progress = wrap(time * moveScale * 0.09 + index * 0.16, 1)
        const x = progress * w
        const y = (index % 2 === 0 ? h * 0.2 : h * 0.85) + Math.sin(time * 0.9 + index) * (h * 0.04)
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

export default canvas.stateful('Roller Rink', {
    theme:           combo('Theme', THEMES, { default: 'Blacklight' }),
    scene:           combo('Scene', SCENES, { default: 'Pattern 1' }),
    colorMode:       combo('Color Mode', COLOR_MODES, { default: 'Static' }),
    moveSpeed:       num('Animation Speed', [0, 100], 33),
    cycleSpeed:      num('Color Cycle Speed', [0, 100], 48),
    density:         num('Density', [0, 100], 72),
    frontColor:      color('Front Color', '#ff52c8'),
    squiggleColor:   color('Squiggle Color', '#25e7ff'),
    accentColor:     color('Accent Color', '#f3f14e'),
    backgroundColor: color('Background Color', '#05050b'),
}, () => {
    let flecks: Fleck[] = []
    let ornaments: Ornament[] = []
    let lastDensity = -1

    function reseed(density: number): void {
        const fleckCount = Math.floor(140 + density * 3.2)
        const ornamentCount = Math.floor(18 + density * 0.34)
        flecks = buildFlecks(fleckCount)
        ornaments = buildOrnaments(ornamentCount)
        lastDensity = density
    }

    return (ctx, time, controls) => {
        const width = ctx.canvas.width
        const height = ctx.canvas.height
        const density = controls.density as number

        if (flecks.length === 0 || ornaments.length === 0 || density !== lastDensity) {
            reseed(density)
        }

        const moveScale = clamp((controls.moveSpeed as number) / 33, 0, 3.1)
        const scene = controls.scene as SceneName
        const palette = getActivePalette(controls as Record<string, unknown>, time)

        drawBackground(ctx, width, height, palette.background)
        drawCarpetFlecks(ctx, width, height, flecks, palette, time, moveScale)
        drawOrnaments(ctx, width, height, ornaments, palette, time, scene, moveScale)

        if (scene === 'Pattern 1') {
            drawPatternOne(ctx, width, height, palette, time, moveScale)
        } else if (scene === 'Pattern 2') {
            drawPatternTwo(ctx, width, height, palette, time, moveScale, density)
        } else {
            drawPatternThree(ctx, width, height, palette, time, moveScale)
        }

        if (palette.background.toLowerCase() === '#ccc000') {
            ctx.fillStyle = rgba('#000000', 0.08)
            for (let index = 0; index < 24; index++) {
                const x = hash(index * 1.11 + 0.5) * width
                const y = hash(index * 1.97 + 2.4) * height
                ctx.beginPath()
                ctx.arc(x, y, 3 + hash(index * 3.73 + 4.1) * 7, 0, Math.PI * 2)
                ctx.fill()
            }
        }
    }
}, {
    description: 'Sharp blacklight carpet geometry inspired by the original 90s bus-and-rink patterns, now with themed palettes',
    author: 'Hypercolor',
})
