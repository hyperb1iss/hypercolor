import { canvas, color, combo, num } from '@hypercolor/sdk'

interface Point {
    x: number
    y: number
}

interface Rgb {
    r: number
    g: number
    b: number
}

interface ThemePalette {
    background: string
    wallA: string
    wallB: string
    accent: string
    core: string
}

interface ResolvedPalette {
    background: Rgb
    wallA: Rgb
    wallB: Rgb
    accent: Rgb
    core: Rgb
}

interface RibbonSeed {
    amplitude: number
    colorBias: number
    lane: number
    phase: number
    speed: number
    width: number
}

interface SparkSeed {
    colorBias: number
    phase: number
    ribbon: number
    size: number
    speed: number
}

interface RenderedRibbon {
    colorA: Rgb
    colorB: Rgb
    core: Rgb
    fieldIndex: number
    fringeA: Rgb
    fringeB: Rgb
    lane: number
    leftNode: Point
    points: Point[]
    rightNode: Point
    speed: number
    span: number
    strength: number
    width: number
    phase: number
}

interface BridgeField {
    axisDir: Point
    axisNormal: Point
    leftNode: Point
    midpoint: Point
    nodeRadius: number
    rightNode: Point
    span: number
    strength: number
}

const THEME_NAMES = ['Abyssal', 'Custom', 'Event Horizon', 'Quantum', 'Solar Flare', 'Spectral', 'Void Gate'] as const
const GEOMETRY_NAMES = ['Braided Flux', 'Prism Bridge', 'Tidal Lattice', 'Halo Exchange'] as const

type ThemeName = (typeof THEME_NAMES)[number]
type GeometryName = (typeof GEOMETRY_NAMES)[number]

const DEFAULT_BACKGROUND = '#050913'
const DEFAULT_COLOR_1 = '#20f0ff'
const DEFAULT_COLOR_2 = '#9056ff'
const DEFAULT_COLOR_3 = '#ff5cb7'
const TAU = Math.PI * 2

const THEMES: Record<Exclude<ThemeName, 'Custom'>, ThemePalette> = {
    Abyssal: {
        accent: '#3ef3ff',
        background: '#080607',
        core: '#ffd166',
        wallA: '#5d1028',
        wallB: '#ff6200',
    },
    'Event Horizon': {
        accent: '#ff4fd8',
        background: '#030714',
        core: '#ffd166',
        wallA: '#2840ff',
        wallB: '#20f0ff',
    },
    Quantum: {
        accent: '#ffde59',
        background: '#031112',
        core: '#9056ff',
        wallA: '#00d7ff',
        wallB: '#7bff58',
    },
    'Solar Flare': {
        accent: '#ff4b7a',
        background: '#140700',
        core: '#20f0ff',
        wallA: '#ff5e00',
        wallB: '#ffd166',
    },
    Spectral: {
        accent: '#ff4fb4',
        background: '#060612',
        core: '#ffd166',
        wallA: '#20f0ff',
        wallB: '#8d5cff',
    },
    'Void Gate': {
        accent: '#ff3ca8',
        background: '#0a0416',
        core: '#ffd6ff',
        wallA: '#6121ff',
        wallB: '#18f0ff',
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
    const seeded = Math.sin(value * 127.1 + 311.7) * 43758.5453123
    return seeded - Math.floor(seeded)
}

function hexToRgb(hex: string, fallback: Rgb = { b: 0, g: 0, r: 0 }): Rgb {
    const normalized = hex.trim().replace('#', '')
    const full =
        normalized.length === 3
            ? `${normalized[0]}${normalized[0]}${normalized[1]}${normalized[1]}${normalized[2]}${normalized[2]}`
            : normalized

    if (!/^[0-9a-fA-F]{6}$/.test(full)) {
        return fallback
    }

    const value = Number.parseInt(full, 16)

    return {
        b: value & 255,
        g: (value >> 8) & 255,
        r: (value >> 16) & 255,
    }
}

function mixRgb(a: Rgb, b: Rgb, amount: number): Rgb {
    const t = clamp(amount, 0, 1)
    return {
        b: Math.round(a.b + (b.b - a.b) * t),
        g: Math.round(a.g + (b.g - a.g) * t),
        r: Math.round(a.r + (b.r - a.r) * t),
    }
}

function scaleRgb(color: Rgb, factor: number): Rgb {
    return {
        b: clamp(Math.round(color.b * factor), 0, 255),
        g: clamp(Math.round(color.g * factor), 0, 255),
        r: clamp(Math.round(color.r * factor), 0, 255),
    }
}

function saturateRgb(color: Rgb, amount: number): Rgb {
    const luminance = color.r * 0.2126 + color.g * 0.7152 + color.b * 0.0722
    return {
        b: clamp(Math.round(luminance + (color.b - luminance) * amount), 0, 255),
        g: clamp(Math.round(luminance + (color.g - luminance) * amount), 0, 255),
        r: clamp(Math.round(luminance + (color.r - luminance) * amount), 0, 255),
    }
}

function richenRgb(color: Rgb, saturation = 1.18, maxWhiteness = 0.24, peak = 236): Rgb {
    const saturated = saturateRgb(color, saturation)
    const brightest = Math.max(saturated.r, saturated.g, saturated.b)

    if (brightest <= 0) {
        return saturated
    }

    let dimmestKey: keyof Rgb = 'r'
    if (saturated.g < saturated[dimmestKey]) dimmestKey = 'g'
    if (saturated.b < saturated[dimmestKey]) dimmestKey = 'b'

    const sculpted = { ...saturated }
    sculpted[dimmestKey] = Math.min(saturated[dimmestKey], Math.round(brightest * clamp(maxWhiteness, 0, 1)))

    if (brightest <= peak) {
        return sculpted
    }

    return scaleRgb(sculpted, peak / brightest)
}

function rgb(color: Rgb): string {
    return `rgb(${color.r}, ${color.g}, ${color.b})`
}

function rgba(color: Rgb, alpha: number): string {
    return `rgba(${color.r}, ${color.g}, ${color.b}, ${clamp(alpha, 0, 1).toFixed(3)})`
}

function resolvePalette(
    theme: ThemeName,
    color1: string,
    color2: string,
    color3: string,
    background: string,
): ResolvedPalette {
    if (theme !== 'Custom') {
        const preset = THEMES[theme] ?? THEMES['Event Horizon']
        return {
            accent: hexToRgb(preset.accent),
            background: hexToRgb(preset.background),
            core: hexToRgb(preset.core),
            wallA: hexToRgb(preset.wallA),
            wallB: hexToRgb(preset.wallB),
        }
    }

    const wallA = hexToRgb(color1, hexToRgb(DEFAULT_COLOR_1))
    const wallB = hexToRgb(color2, hexToRgb(DEFAULT_COLOR_2))
    const accent = saturateRgb(hexToRgb(color3, hexToRgb(DEFAULT_COLOR_3)), 1.18)
    const backdrop = hexToRgb(background, hexToRgb(DEFAULT_BACKGROUND))

    return {
        accent,
        background: backdrop,
        core: saturateRgb(mixRgb(accent, wallB, 0.35), 1.12),
        wallA,
        wallB,
    }
}

function sampleSpectralPalette(t: number, palette: ResolvedPalette): Rgb {
    const phase = wrap(t, 1)
    const prismA = saturateRgb(mixRgb(palette.wallA, palette.core, 0.34), 1.18)
    const prismB = saturateRgb(mixRgb(palette.accent, palette.wallB, 0.42), 1.24)
    const prismC = saturateRgb(mixRgb(palette.core, palette.accent, 0.58), 1.16)
    const prismD = saturateRgb(mixRgb(palette.wallB, palette.core, 0.28), 1.12)

    if (phase < 0.2) {
        return mixRgb(prismA, palette.accent, phase / 0.2)
    }
    if (phase < 0.4) {
        return mixRgb(palette.accent, prismB, (phase - 0.2) / 0.2)
    }
    if (phase < 0.6) {
        return mixRgb(prismB, palette.core, (phase - 0.4) / 0.2)
    }
    if (phase < 0.8) {
        return mixRgb(palette.core, prismC, (phase - 0.6) / 0.2)
    }
    return mixRgb(prismC, prismD, (phase - 0.8) / 0.2)
}

function addPoint(a: Point, b: Point): Point {
    return { x: a.x + b.x, y: a.y + b.y }
}

function subPoint(a: Point, b: Point): Point {
    return { x: a.x - b.x, y: a.y - b.y }
}

function scalePoint(point: Point, amount: number): Point {
    return { x: point.x * amount, y: point.y * amount }
}

function lerpPoint(a: Point, b: Point, amount: number): Point {
    return {
        x: lerp(a.x, b.x, amount),
        y: lerp(a.y, b.y, amount),
    }
}

function normalizePoint(point: Point, fallback: Point): Point {
    const length = Math.hypot(point.x, point.y)
    if (length <= 0.0001) {
        return fallback
    }
    return {
        x: point.x / length,
        y: point.y / length,
    }
}

function perpendicular(point: Point): Point {
    return { x: -point.y, y: point.x }
}

function cubicBezierPoint(p0: Point, p1: Point, p2: Point, p3: Point, t: number): Point {
    const inverse = 1 - t
    const inverse2 = inverse * inverse
    const inverse3 = inverse2 * inverse
    const t2 = t * t
    const t3 = t2 * t

    return {
        x: p0.x * inverse3 + 3 * p1.x * inverse2 * t + 3 * p2.x * inverse * t2 + p3.x * t3,
        y: p0.y * inverse3 + 3 * p1.y * inverse2 * t + 3 * p2.y * inverse * t2 + p3.y * t3,
    }
}

function drawPolyline(ctx: CanvasRenderingContext2D, points: Point[]): void {
    if (points.length === 0) return
    ctx.beginPath()
    ctx.moveTo(points[0].x, points[0].y)
    for (let index = 1; index < points.length; index++) {
        ctx.lineTo(points[index].x, points[index].y)
    }
}

function offsetPoints(points: Point[], offset: Point): Point[] {
    return points.map((point) => addPoint(point, offset))
}

function samplePolyline(points: Point[], t: number): Point {
    if (points.length === 0) return { x: 0, y: 0 }
    if (points.length === 1) return points[0]

    const scaled = clamp(t, 0, 1) * (points.length - 1)
    const index = Math.floor(scaled)
    const nextIndex = Math.min(points.length - 1, index + 1)
    const amount = scaled - index
    return lerpPoint(points[index], points[nextIndex], amount)
}

function sampleSegment(points: Point[], start: number, end: number, samples: number): Point[] {
    const segment: Point[] = []
    const safeSamples = Math.max(2, samples)
    for (let index = 0; index < safeSamples; index++) {
        const t = lerp(start, end, index / (safeSamples - 1))
        segment.push(samplePolyline(points, t))
    }
    return segment
}

function ribbonWave(
    geometry: GeometryName,
    t: number,
    phase: number,
    time: number,
    twistMix: number,
    pulseMix: number,
): number {
    if (geometry === 'Prism Bridge') {
        const triangle = Math.abs(wrap(t * 2 + phase * 0.1 + time * 0.06, 1) - 0.5) * 2 - 0.5
        return triangle * 1.1 + Math.sin(t * TAU * 3 + phase + time * (0.4 + pulseMix * 0.4)) * 0.25
    }

    if (geometry === 'Tidal Lattice') {
        return (
            Math.sin(t * TAU * 2 + phase + time * (0.42 + twistMix * 0.5)) * 0.65 +
            Math.cos(t * TAU * 4 - phase * 0.6 - time * (0.35 + pulseMix * 0.45)) * 0.35
        )
    }

    if (geometry === 'Halo Exchange') {
        return (
            Math.sin(t * TAU + phase + time * (0.32 + twistMix * 0.38)) *
            Math.cos(t * TAU * 2.2 - time * (0.24 + pulseMix * 0.36) + phase * 0.3)
        )
    }

    return Math.sin(t * TAU * 1.5 + phase + time * (0.46 + twistMix * 0.7))
}

function buildRibbonPoints(
    leftNode: Point,
    rightNode: Point,
    seed: RibbonSeed,
    geometry: GeometryName,
    time: number,
    twistMix: number,
    pulseMix: number,
    thicknessMix: number,
): Point[] {
    const axis = subPoint(rightNode, leftNode)
    const span = Math.hypot(axis.x, axis.y)
    const tangent = normalizePoint(axis, { x: 1, y: 0 })
    const normal = perpendicular(tangent)
    const midpoint = lerpPoint(leftNode, rightNode, 0.5)
    const laneOffset = seed.lane * span * (0.05 + thicknessMix * 0.05)
    const bow = span * (0.08 + seed.amplitude * 0.1 + pulseMix * 0.04)
    const direction = seed.lane >= 0 ? 1 : -1

    const p0 = addPoint(leftNode, scalePoint(normal, laneOffset))
    const p3 = addPoint(rightNode, scalePoint(normal, -laneOffset))
    const p1 = addPoint(
        addPoint(leftNode, scalePoint(tangent, span * 0.24)),
        scalePoint(normal, bow * direction + laneOffset * 0.5),
    )
    const p2 = addPoint(
        addPoint(rightNode, scalePoint(tangent, -span * 0.24)),
        scalePoint(normal, -bow * direction - laneOffset * 0.5),
    )

    const points: Point[] = []
    const sampleCount = 44

    for (let index = 0; index < sampleCount; index++) {
        const t = index / (sampleCount - 1)
        const envelope = Math.sin(t * Math.PI) ** 0.92
        const weave =
            ribbonWave(geometry, t, seed.phase, time * seed.speed, twistMix, pulseMix) *
            span *
            (0.025 + twistMix * 0.05 + seed.amplitude * 0.025) *
            envelope
        const ripple =
            Math.sin(t * TAU * (2.5 + seed.width * 2.2) - time * (0.36 + pulseMix * 0.55) + seed.phase * 0.7) *
            span *
            0.008 *
            (0.4 + twistMix)
        const centerPull =
            Math.cos((t - 0.5) * Math.PI) *
            span *
            0.018 *
            (0.5 + pulseMix * 0.6) *
            (geometry === 'Halo Exchange' ? 1.2 : 1)

        const base = cubicBezierPoint(p0, p1, p2, p3, t)
        const towardCenter = subPoint(midpoint, base)
        const centerDir = normalizePoint(towardCenter, normal)

        points.push(
            addPoint(
                addPoint(base, scalePoint(normal, weave)),
                addPoint(scalePoint(tangent, ripple), scalePoint(centerDir, centerPull * envelope)),
            ),
        )
    }

    return points
}

function drawNodeHalo(
    ctx: CanvasRenderingContext2D,
    node: Point,
    radius: number,
    palette: ResolvedPalette,
    time: number,
    phase: number,
    geometry: GeometryName,
    pulseMix: number,
    contrastMix: number,
    strength: number,
): void {
    const glowA = richenRgb(sampleSpectralPalette(phase * 0.09 + time * 0.05, palette), 1.16, 0.22, 228)
    const glowB = richenRgb(sampleSpectralPalette(phase * 0.09 + 0.38 - time * 0.04, palette), 1.16, 0.22, 220)
    const glow = ctx.createRadialGradient(node.x, node.y, 0, node.x, node.y, radius * 1.45)
    glow.addColorStop(0, rgba(glowA, (0.055 + contrastMix * 0.025) * strength))
    glow.addColorStop(0.44, rgba(glowB, (0.02 + pulseMix * 0.016) * strength))
    glow.addColorStop(1, 'rgba(0,0,0,0)')
    ctx.fillStyle = glow
    ctx.fillRect(node.x - radius * 1.6, node.y - radius * 1.6, radius * 3.2, radius * 3.2)

    ctx.save()
    ctx.globalCompositeOperation = 'source-over'
    ctx.lineCap = 'round'

    const haloCount = geometry === 'Halo Exchange' ? 4 : 3
    for (let ring = 0; ring < haloCount; ring++) {
        const orbit = ring / Math.max(1, haloCount - 1)
        const rotation = time * (0.18 + orbit * 0.12) + phase + ring * 0.65
        const radiusX = radius * (1.05 + orbit * 0.48)
        const radiusY =
            radius *
            (geometry === 'Prism Bridge'
                ? 0.42 + orbit * 0.12
                : geometry === 'Tidal Lattice'
                  ? 0.58 + orbit * 0.16
                  : 0.5 + orbit * 0.14)
        const color = richenRgb(
            sampleSpectralPalette(phase * 0.11 + ring * 0.18 + time * 0.025, palette),
            1.2,
            0.2,
            236,
        )
        const fringeA = richenRgb(
            sampleSpectralPalette(phase * 0.11 + 0.17 + ring * 0.14 - time * 0.035, palette),
            1.24,
            0.18,
            236,
        )
        const fringeB = richenRgb(
            sampleSpectralPalette(phase * 0.11 + 0.53 + ring * 0.2 + time * 0.03, palette),
            1.24,
            0.18,
            236,
        )
        const alpha = (0.05 + (1 - orbit) * 0.04 + pulseMix * 0.02) * strength
        const chromaSpread = radius * (0.02 + pulseMix * 0.018 + orbit * 0.01)
        const chromaOffset = {
            x: Math.cos(rotation + Math.PI * 0.5) * chromaSpread,
            y: Math.sin(rotation + Math.PI * 0.5) * chromaSpread,
        }

        ctx.setLineDash([radius * 0.22, radius * 0.16 + ring * 2])
        ctx.lineDashOffset = -time * (22 + ring * 6)
        ctx.beginPath()
        ctx.ellipse(node.x - chromaOffset.x, node.y - chromaOffset.y, radiusX, radiusY, rotation, 0, TAU)
        ctx.lineWidth = Math.max(1, radius * 0.06 * (1 - orbit * 0.25) * (0.7 + strength * 0.3))
        ctx.strokeStyle = rgba(fringeA, alpha * 0.4)
        ctx.stroke()

        ctx.beginPath()
        ctx.ellipse(node.x + chromaOffset.x, node.y + chromaOffset.y, radiusX, radiusY, rotation, 0, TAU)
        ctx.lineWidth = Math.max(1, radius * 0.06 * (1 - orbit * 0.25) * (0.7 + strength * 0.3))
        ctx.strokeStyle = rgba(fringeB, alpha * 0.4)
        ctx.stroke()

        ctx.beginPath()
        ctx.ellipse(node.x, node.y, radiusX, radiusY, rotation, 0, TAU)
        ctx.lineWidth = Math.max(1, radius * 0.075 * (1 - orbit * 0.25) * (0.7 + strength * 0.3))
        ctx.strokeStyle = rgba(color, alpha)
        ctx.stroke()
    }

    ctx.setLineDash([])
    ctx.restore()
}

function drawLensDiamond(
    ctx: CanvasRenderingContext2D,
    leftNode: Point,
    rightNode: Point,
    palette: ResolvedPalette,
    time: number,
    pulseMix: number,
    contrastMix: number,
    strength: number,
): void {
    const axis = subPoint(rightNode, leftNode)
    const span = Math.hypot(axis.x, axis.y)
    const tangent = normalizePoint(axis, { x: 1, y: 0 })
    const normal = perpendicular(tangent)
    const center = lerpPoint(leftNode, rightNode, 0.5)
    const halfLength = span * (0.15 + pulseMix * 0.025)
    const halfWidth = span * (0.05 + contrastMix * 0.025)

    const points = [
        addPoint(center, scalePoint(tangent, halfLength)),
        addPoint(center, scalePoint(normal, halfWidth)),
        addPoint(center, scalePoint(tangent, -halfLength)),
        addPoint(center, scalePoint(normal, -halfWidth)),
    ]

    const lensGradient = ctx.createLinearGradient(points[2].x, points[2].y, points[0].x, points[0].y)
    lensGradient.addColorStop(
        0,
        rgba(
            richenRgb(sampleSpectralPalette(time * 0.035 + 0.06, palette), 1.18, 0.22, 228),
            (0.06 + contrastMix * 0.03) * strength,
        ),
    )
    lensGradient.addColorStop(
        0.35,
        rgba(
            richenRgb(sampleSpectralPalette(time * 0.03 + 0.26, palette), 1.2, 0.2, 228),
            (0.075 + pulseMix * 0.025) * strength,
        ),
    )
    lensGradient.addColorStop(
        0.65,
        rgba(
            richenRgb(sampleSpectralPalette(time * 0.04 + 0.52, palette), 1.2, 0.2, 228),
            (0.09 + pulseMix * 0.03) * strength,
        ),
    )
    lensGradient.addColorStop(
        1,
        rgba(
            richenRgb(sampleSpectralPalette(time * 0.025 + 0.78, palette), 1.18, 0.22, 228),
            (0.06 + contrastMix * 0.03) * strength,
        ),
    )

    ctx.beginPath()
    ctx.moveTo(points[0].x, points[0].y)
    ctx.lineTo(points[1].x, points[1].y)
    ctx.lineTo(points[2].x, points[2].y)
    ctx.lineTo(points[3].x, points[3].y)
    ctx.closePath()
    ctx.fillStyle = lensGradient
    ctx.fill()

    ctx.save()
    ctx.clip()
    ctx.globalCompositeOperation = 'source-over'
    for (let stripe = 0; stripe < 3; stripe++) {
        const stripeShift = (wrap(time * (0.12 + stripe * 0.03), 1) - 0.5) * halfLength * 3
        const stripeGradient = ctx.createLinearGradient(
            center.x - tangent.x * halfLength * 2 - normal.x * halfWidth + tangent.x * stripeShift,
            center.y - tangent.y * halfLength * 2 - normal.y * halfWidth + tangent.y * stripeShift,
            center.x + tangent.x * halfLength * 2 + normal.x * halfWidth + tangent.x * stripeShift,
            center.y + tangent.y * halfLength * 2 + normal.y * halfWidth + tangent.y * stripeShift,
        )
        stripeGradient.addColorStop(0, 'rgba(0,0,0,0)')
        stripeGradient.addColorStop(
            0.5,
            rgba(
                richenRgb(sampleSpectralPalette(stripe * 0.21 + time * 0.06, palette), 1.24, 0.18, 236),
                (0.035 + pulseMix * 0.018) * strength,
            ),
        )
        stripeGradient.addColorStop(1, 'rgba(0,0,0,0)')
        ctx.fillStyle = stripeGradient
        ctx.fillRect(center.x - halfLength * 2.4, center.y - halfWidth * 2.4, halfLength * 4.8, halfWidth * 4.8)
    }
    ctx.restore()

    const chromaSpread = span * 0.008 * strength

    ctx.beginPath()
    ctx.moveTo(points[0].x - normal.x * chromaSpread, points[0].y - normal.y * chromaSpread)
    ctx.lineTo(points[1].x - normal.x * chromaSpread, points[1].y - normal.y * chromaSpread)
    ctx.lineTo(points[2].x - normal.x * chromaSpread, points[2].y - normal.y * chromaSpread)
    ctx.lineTo(points[3].x - normal.x * chromaSpread, points[3].y - normal.y * chromaSpread)
    ctx.closePath()
    ctx.lineWidth = Math.max(1, span * 0.006)
    ctx.strokeStyle = rgba(
        richenRgb(sampleSpectralPalette(time * 0.04 + 0.18, palette), 1.2, 0.18, 236),
        (0.07 + contrastMix * 0.035) * strength,
    )
    ctx.stroke()

    ctx.beginPath()
    ctx.moveTo(points[0].x + normal.x * chromaSpread, points[0].y + normal.y * chromaSpread)
    ctx.lineTo(points[1].x + normal.x * chromaSpread, points[1].y + normal.y * chromaSpread)
    ctx.lineTo(points[2].x + normal.x * chromaSpread, points[2].y + normal.y * chromaSpread)
    ctx.lineTo(points[3].x + normal.x * chromaSpread, points[3].y + normal.y * chromaSpread)
    ctx.closePath()
    ctx.lineWidth = Math.max(1, span * 0.006)
    ctx.strokeStyle = rgba(
        richenRgb(sampleSpectralPalette(0.58 - time * 0.035, palette), 1.2, 0.18, 236),
        (0.07 + contrastMix * 0.035) * strength,
    )
    ctx.stroke()

    ctx.beginPath()
    ctx.moveTo(points[0].x, points[0].y)
    ctx.lineTo(points[1].x, points[1].y)
    ctx.lineTo(points[2].x, points[2].y)
    ctx.lineTo(points[3].x, points[3].y)
    ctx.closePath()
    ctx.lineWidth = Math.max(2, span * 0.01)
    ctx.strokeStyle = rgba(
        richenRgb(mixRgb(palette.accent, palette.core, 0.25), 1.14, 0.18, 232),
        (0.14 + contrastMix * 0.08) * strength,
    )
    ctx.stroke()
}

export default canvas.stateful(
    'Einstein Bridge',
    {
        background: color('Backdrop', DEFAULT_BACKGROUND, { group: 'Color' }),
        color1: color('Color 1', DEFAULT_COLOR_1, { group: 'Color' }),
        color2: color('Color 2', DEFAULT_COLOR_2, { group: 'Color' }),
        color3: color('Color 3', DEFAULT_COLOR_3, { group: 'Color' }),
        contrast: num('Contrast', [0, 100], 60, { group: 'Atmosphere' }),
        depth: num('Depth', [0, 100], 66, { group: 'Motion' }),
        drift: num('Drift', [0, 100], 42, { group: 'Motion' }),
        geometry: combo('Geometry', GEOMETRY_NAMES, { default: 'Braided Flux', group: 'Scene' }),
        pulse: num('Pulse', [0, 100], 34, { group: 'Atmosphere' }),
        speed: num('Speed', [1, 10], 5, { group: 'Motion' }),
        theme: combo('Theme', THEME_NAMES, { default: 'Event Horizon', group: 'Scene' }),
        thickness: num('Wall Thickness', [0, 100], 56, { group: 'Atmosphere' }),
        twist: num('Twist', [0, 100], 58, { group: 'Motion' }),
    },
    () => {
        const ribbons: RibbonSeed[] = []
        const sparks: SparkSeed[] = []
        let ribbonCount = 0
        let sparkCount = 0

        function ensureRibbons(count: number): void {
            const target = clamp(Math.round(count), 6, 14)
            if (target === ribbonCount && ribbons.length === target) return

            if (target > ribbons.length) {
                for (let index = ribbons.length; index < target; index++) {
                    const seed = index + 1
                    ribbons.push({
                        amplitude: hash(seed * 2.31 + 0.2),
                        colorBias: hash(seed * 7.13 + 3.11),
                        lane: hash(seed * 3.71 + 1.9) * 2 - 1,
                        phase: hash(seed * 5.19 + 4.07) * TAU,
                        speed: 0.65 + hash(seed * 9.91 + 2.1) * 1.25,
                        width: hash(seed * 11.37 + 6.4),
                    })
                }
            } else {
                ribbons.length = target
            }

            ribbonCount = target
        }

        function ensureSparks(count: number): void {
            const target = clamp(Math.round(count), 10, 26)
            if (target === sparkCount && sparks.length === target) return

            if (target > sparks.length) {
                for (let index = sparks.length; index < target; index++) {
                    const seed = index + 1
                    sparks.push({
                        colorBias: hash(seed * 8.93 + 1.17),
                        phase: hash(seed * 2.81 + 7.42),
                        ribbon: Math.floor(hash(seed * 3.29 + 5.61) * 32),
                        size: hash(seed * 6.73 + 2.31),
                        speed: 0.7 + hash(seed * 9.47 + 4.23) * 1.4,
                    })
                }
            } else {
                sparks.length = target
            }

            sparkCount = target
        }

        return (ctx, time, controls) => {
            const width = ctx.canvas.width
            const height = ctx.canvas.height
            const minDim = Math.min(width, height)
            const maxDim = Math.max(width, height)

            if (width === 0 || height === 0) return

            const speedMix = clamp((((controls.speed as number) ?? 5) - 1) / 9, 0, 1)
            const depthMix = clamp(((controls.depth as number) ?? 66) / 100, 0, 1)
            const twistMix = clamp(((controls.twist as number) ?? 58) / 100, 0, 1)
            const driftMix = clamp(((controls.drift as number) ?? 42) / 100, 0, 1)
            const pulseMix = clamp(((controls.pulse as number) ?? 34) / 100, 0, 1)
            const thicknessMix = clamp(((controls.thickness as number) ?? 56) / 100, 0, 1)
            const contrastMix = clamp(((controls.contrast as number) ?? 60) / 100, 0, 1)
            const theme = (controls.theme as ThemeName) ?? 'Event Horizon'
            const geometry = (controls.geometry as GeometryName) ?? 'Braided Flux'

            const palette = resolvePalette(
                theme,
                controls.color1 as string,
                controls.color2 as string,
                controls.color3 as string,
                controls.background as string,
            )

            const center = { x: width * 0.5, y: height * 0.5 }
            const axisTime = time * (0.16 + speedMix * 0.1)
            const axisRotation = Math.sin(axisTime * 0.42) * (0.12 + driftMix * 0.18)
            const fieldCount = geometry === 'Tidal Lattice' ? 4 : 3
            const bridgeFields: BridgeField[] = []

            for (let fieldIndex = 0; fieldIndex < fieldCount; fieldIndex++) {
                const fieldMix = fieldIndex / (fieldCount - 1)
                const centerBias = fieldMix - 0.5
                const rotationSpread =
                    geometry === 'Prism Bridge'
                        ? 1.18
                        : geometry === 'Halo Exchange'
                          ? 1.42
                          : geometry === 'Tidal Lattice'
                            ? 0.96
                            : 0.84
                const fieldRotation =
                    axisRotation +
                    centerBias * rotationSpread +
                    Math.sin(axisTime * 0.51 + fieldIndex * 1.7) * (0.1 + driftMix * 0.08)
                const axisDir = normalizePoint(
                    {
                        x: Math.cos(fieldRotation),
                        y: Math.sin(fieldRotation) * (geometry === 'Prism Bridge' ? 0.92 : 0.82),
                    },
                    { x: 1, y: 0 },
                )
                const axisNormal = perpendicular(axisDir)
                const span = maxDim * (0.55 + depthMix * 0.26 + Math.abs(centerBias) * 0.08)
                const sheetOffset =
                    centerBias * minDim * (0.72 + driftMix * 0.18) +
                    Math.sin(axisTime * 0.73 + fieldIndex * 1.3) * minDim * (0.05 + driftMix * 0.04)
                const travelOffset = Math.cos(axisTime * 0.37 + fieldIndex * 1.8) * minDim * (0.06 + driftMix * 0.03)
                const fieldCenter = addPoint(
                    center,
                    addPoint(scalePoint(axisNormal, sheetOffset), scalePoint(axisDir, travelOffset)),
                )
                const nodeOffsetY = minDim * (0.08 + driftMix * 0.12 + Math.abs(centerBias) * 0.02)
                const strength = 1 - Math.abs(centerBias) * 0.58
                const nodeRadius = minDim * (0.06 + depthMix * 0.035) * (0.82 + strength * 0.38)
                const leftNode = addPoint(
                    addPoint(fieldCenter, scalePoint(axisDir, -span)),
                    addPoint(
                        scalePoint(axisNormal, Math.sin(axisTime * 0.83 + 0.9 + fieldIndex * 0.9) * nodeOffsetY),
                        scalePoint(
                            axisDir,
                            Math.cos(axisTime * 0.31 + 1.7 + fieldIndex * 0.4) * minDim * driftMix * 0.03,
                        ),
                    ),
                )
                const rightNode = addPoint(
                    addPoint(fieldCenter, scalePoint(axisDir, span)),
                    addPoint(
                        scalePoint(axisNormal, Math.sin(axisTime * 0.83 + 3.2 + fieldIndex * 0.9) * -nodeOffsetY),
                        scalePoint(
                            axisDir,
                            Math.cos(axisTime * 0.31 + 3.4 + fieldIndex * 0.4) * minDim * driftMix * 0.03,
                        ),
                    ),
                )

                bridgeFields.push({
                    axisDir,
                    axisNormal,
                    leftNode,
                    midpoint: lerpPoint(leftNode, rightNode, 0.5),
                    nodeRadius,
                    rightNode,
                    span: Math.hypot(rightNode.x - leftNode.x, rightNode.y - leftNode.y),
                    strength,
                })
            }

            ctx.fillStyle = rgb(palette.background)
            ctx.fillRect(0, 0, width, height)

            const backdrop = ctx.createLinearGradient(0, 0, width, height)
            backdrop.addColorStop(
                0,
                rgba(
                    mixRgb(sampleSpectralPalette(time * 0.01 + 0.04, palette), palette.background, 0.56),
                    0.24 + contrastMix * 0.05,
                ),
            )
            backdrop.addColorStop(
                0.26,
                rgba(
                    mixRgb(sampleSpectralPalette(time * 0.015 + 0.22, palette), palette.background, 0.68),
                    0.1 + pulseMix * 0.04,
                ),
            )
            backdrop.addColorStop(
                0.5,
                rgba(
                    mixRgb(sampleSpectralPalette(time * 0.012 + 0.46, palette), palette.background, 0.78),
                    0.05 + pulseMix * 0.03,
                ),
            )
            backdrop.addColorStop(
                0.74,
                rgba(
                    mixRgb(sampleSpectralPalette(0.68 - time * 0.013, palette), palette.background, 0.66),
                    0.1 + contrastMix * 0.03,
                ),
            )
            backdrop.addColorStop(
                1,
                rgba(
                    mixRgb(sampleSpectralPalette(0.9 - time * 0.01, palette), palette.background, 0.54),
                    0.22 + contrastMix * 0.05,
                ),
            )
            ctx.fillStyle = backdrop
            ctx.fillRect(0, 0, width, height)

            ctx.save()
            ctx.globalCompositeOperation = 'source-over'
            for (const [fieldIndex, field] of bridgeFields.entries()) {
                for (let storm = 0; storm < 2; storm++) {
                    const orbit = time * (0.06 + storm * 0.03 + speedMix * 0.05) + storm * 1.7 + fieldIndex * 1.2
                    const stormCenter = addPoint(
                        field.midpoint,
                        addPoint(
                            scalePoint(field.axisDir, Math.cos(orbit) * minDim * (0.12 + storm * 0.07)),
                            scalePoint(field.axisNormal, Math.sin(orbit * 1.2) * minDim * (0.2 + storm * 0.08)),
                        ),
                    )
                    const stormColor = sampleSpectralPalette(fieldIndex * 0.18 + storm * 0.22 + time * 0.035, palette)
                    const glow = ctx.createRadialGradient(
                        stormCenter.x,
                        stormCenter.y,
                        0,
                        stormCenter.x,
                        stormCenter.y,
                        minDim * (0.22 + storm * 0.08),
                    )
                    glow.addColorStop(
                        0,
                        rgba(stormColor, (0.028 + contrastMix * 0.014 + pulseMix * 0.012) * field.strength),
                    )
                    glow.addColorStop(
                        0.48,
                        rgba(
                            sampleSpectralPalette(fieldIndex * 0.21 + storm * 0.17 + 0.36 - time * 0.028, palette),
                            (0.012 + pulseMix * 0.01) * field.strength,
                        ),
                    )
                    glow.addColorStop(1, 'rgba(0,0,0,0)')
                    ctx.fillStyle = glow
                    ctx.fillRect(0, 0, width, height)
                }
            }
            ctx.restore()

            for (const [fieldIndex, field] of bridgeFields.entries()) {
                drawNodeHalo(
                    ctx,
                    field.leftNode,
                    field.nodeRadius,
                    palette,
                    time,
                    fieldIndex * 0.75,
                    geometry,
                    pulseMix,
                    contrastMix,
                    field.strength,
                )
                drawNodeHalo(
                    ctx,
                    field.rightNode,
                    field.nodeRadius,
                    palette,
                    time,
                    Math.PI + fieldIndex * 0.75,
                    geometry,
                    pulseMix,
                    contrastMix,
                    field.strength,
                )
            }

            ensureRibbons(5 + Math.round(depthMix * 3))
            ensureSparks(8 + Math.round(depthMix * 5))

            const renderedRibbons: RenderedRibbon[] = []

            ctx.save()
            ctx.globalCompositeOperation = 'source-over'

            for (const [fieldIndex, field] of bridgeFields.entries()) {
                const fieldRibbons: RenderedRibbon[] = ribbons.map((seed, index) => {
                    const fieldSeed: RibbonSeed = {
                        amplitude: clamp(
                            seed.amplitude * 0.68 + hash((fieldIndex + 1) * 4.1 + index * 0.77) * 0.48,
                            0,
                            1,
                        ),
                        colorBias: wrap(seed.colorBias + fieldIndex * 0.17 + index * 0.013, 1),
                        lane: clamp(seed.lane * (0.8 + field.strength * 0.25) + (fieldIndex - 1) * 0.06, -1, 1),
                        phase: seed.phase + fieldIndex * 1.3,
                        speed: seed.speed * (0.92 + field.strength * 0.18),
                        width: clamp(seed.width * 0.7 + hash((fieldIndex + 2) * 5.4 + index * 1.2) * 0.42, 0, 1),
                    }
                    const points = buildRibbonPoints(
                        field.leftNode,
                        field.rightNode,
                        fieldSeed,
                        geometry,
                        time,
                        twistMix,
                        pulseMix,
                        thicknessMix,
                    )
                    const colorA = richenRgb(
                        sampleSpectralPalette(fieldSeed.colorBias + time * (0.04 + speedMix * 0.03), palette),
                        1.2,
                        0.18,
                        236,
                    )
                    const colorB = richenRgb(
                        sampleSpectralPalette(fieldSeed.colorBias + 0.31 + time * (0.036 + speedMix * 0.03), palette),
                        1.2,
                        0.18,
                        236,
                    )
                    const fringeA = richenRgb(
                        sampleSpectralPalette(fieldSeed.colorBias + 0.14 - time * (0.024 + speedMix * 0.015), palette),
                        1.28,
                        0.14,
                        240,
                    )
                    const fringeB = richenRgb(
                        sampleSpectralPalette(fieldSeed.colorBias + 0.64 + time * (0.02 + speedMix * 0.014), palette),
                        1.28,
                        0.14,
                        240,
                    )
                    const core = richenRgb(
                        mixRgb(
                            sampleSpectralPalette(fieldSeed.colorBias + 0.49 + fieldIndex * 0.07, palette),
                            fieldSeed.lane > 0 ? palette.core : palette.accent,
                            0.42,
                        ),
                        1.16,
                        0.12,
                        228,
                    )

                    return {
                        colorA,
                        colorB,
                        core,
                        fieldIndex,
                        fringeA,
                        fringeB,
                        lane: fieldSeed.lane,
                        leftNode: field.leftNode,
                        phase: fieldSeed.phase,
                        points,
                        rightNode: field.rightNode,
                        speed: fieldSeed.speed,
                        span: field.span,
                        strength: field.strength,
                        width:
                            minDim *
                            (0.007 + thicknessMix * 0.016) *
                            (0.72 + fieldSeed.width * 0.88) *
                            (0.82 + field.strength * 0.35),
                    }
                })

                fieldRibbons.sort((left, right) => Math.abs(right.lane) - Math.abs(left.lane))
                renderedRibbons.push(...fieldRibbons)

                for (const ribbon of fieldRibbons) {
                    const ribbonAxis = normalizePoint(subPoint(ribbon.rightNode, ribbon.leftNode), { x: 1, y: 0 })
                    const ribbonNormal = perpendicular(ribbonAxis)
                    const fringeOffsetA = addPoint(
                        scalePoint(ribbonNormal, ribbon.width * (0.1 + contrastMix * 0.05) * ribbon.strength),
                        scalePoint(ribbonAxis, ribbon.width * 0.05 * Math.sin(time * 0.8 + ribbon.phase)),
                    )
                    const fringeOffsetB = addPoint(
                        scalePoint(ribbonNormal, -ribbon.width * (0.1 + contrastMix * 0.05) * ribbon.strength),
                        scalePoint(ribbonAxis, -ribbon.width * 0.05 * Math.cos(time * 0.74 + ribbon.phase)),
                    )
                    const fringePointsA = offsetPoints(ribbon.points, fringeOffsetA)
                    const fringePointsB = offsetPoints(ribbon.points, fringeOffsetB)
                    const gradient = ctx.createLinearGradient(
                        ribbon.leftNode.x,
                        ribbon.leftNode.y,
                        ribbon.rightNode.x,
                        ribbon.rightNode.y,
                    )
                    gradient.addColorStop(0, rgba(ribbon.fringeA, (0.1 + contrastMix * 0.03) * ribbon.strength))
                    gradient.addColorStop(0.2, rgba(ribbon.colorA, (0.14 + contrastMix * 0.05) * ribbon.strength))
                    gradient.addColorStop(0.5, rgba(ribbon.core, (0.2 + pulseMix * 0.05) * ribbon.strength))
                    gradient.addColorStop(0.8, rgba(ribbon.colorB, (0.14 + contrastMix * 0.05) * ribbon.strength))
                    gradient.addColorStop(1, rgba(ribbon.fringeB, (0.1 + contrastMix * 0.03) * ribbon.strength))

                    drawPolyline(ctx, fringePointsA)
                    ctx.lineWidth = ribbon.width * 0.38
                    ctx.strokeStyle = rgba(ribbon.fringeA, (0.09 + contrastMix * 0.03) * ribbon.strength)
                    ctx.stroke()

                    drawPolyline(ctx, fringePointsB)
                    ctx.lineWidth = ribbon.width * 0.38
                    ctx.strokeStyle = rgba(ribbon.fringeB, (0.09 + contrastMix * 0.03) * ribbon.strength)
                    ctx.stroke()

                    drawPolyline(ctx, ribbon.points)
                    ctx.lineWidth = ribbon.width * 1.15
                    ctx.strokeStyle = rgba(
                        richenRgb(mixRgb(ribbon.fringeA, ribbon.fringeB, 0.5), 1.08, 0.18, 224),
                        (0.012 + contrastMix * 0.01) * ribbon.strength,
                    )
                    ctx.stroke()

                    drawPolyline(ctx, ribbon.points)
                    ctx.lineWidth = ribbon.width
                    ctx.strokeStyle = gradient
                    ctx.stroke()

                    ctx.save()
                    ctx.setLineDash([ribbon.span * 0.04, ribbon.span * 0.055])
                    ctx.lineDashOffset = -time * (70 + speedMix * 90) * ribbon.speed
                    drawPolyline(ctx, ribbon.points)
                    ctx.lineWidth = Math.max(1, ribbon.width * 0.28)
                    ctx.strokeStyle = rgba(
                        richenRgb(mixRgb(ribbon.core, palette.core, 0.4), 1.08, 0.14, 228),
                        (0.12 + pulseMix * 0.04) * ribbon.strength,
                    )
                    ctx.stroke()
                    ctx.restore()

                    const pulseProgress = wrap(time * (0.07 + speedMix * 0.16) * ribbon.speed + ribbon.phase / TAU, 1)
                    const segmentRanges =
                        pulseProgress < 0.07
                            ? [
                                  [pulseProgress + 0.93, 1],
                                  [0, pulseProgress + 0.05],
                              ]
                            : pulseProgress > 0.93
                              ? [
                                    [pulseProgress - 0.07, 1],
                                    [0, wrap(pulseProgress + 0.05, 1)],
                                ]
                              : [[pulseProgress - 0.07, pulseProgress + 0.05]]

                    for (const [start, end] of segmentRanges) {
                        const segment = sampleSegment(ribbon.points, start, end, 10)
                        drawPolyline(ctx, segment)
                        ctx.lineWidth = ribbon.width * 0.42
                        ctx.strokeStyle = rgba(
                            richenRgb(mixRgb(ribbon.core, ribbon.fringeA, 0.38), 1.1, 0.14, 232),
                            (0.18 + pulseMix * 0.05) * ribbon.strength,
                        )
                        ctx.stroke()
                    }
                }

                const orderedRibbons = [...fieldRibbons].sort((left, right) => left.lane - right.lane)
                const meshCount = 5 + Math.round(depthMix * 5)
                for (let index = 0; index < meshCount; index++) {
                    const baseT = 0.08 + (index / Math.max(1, meshCount - 1)) * 0.84
                    const drift = Math.sin(time * (0.18 + speedMix * 0.12) + index * 1.1 + fieldIndex * 0.7) * 0.045
                    const crossPoints = orderedRibbons.map((ribbon) =>
                        samplePolyline(ribbon.points, clamp(baseT + drift * ribbon.lane * 0.45, 0, 1)),
                    )

                    drawPolyline(ctx, crossPoints)
                    ctx.lineWidth = Math.max(
                        1,
                        minDim * (0.0018 + thicknessMix * 0.0035) * (0.8 + field.strength * 0.4),
                    )
                    ctx.strokeStyle = rgba(
                        richenRgb(
                            sampleSpectralPalette(baseT + time * 0.03 + index * 0.11 + fieldIndex * 0.09, palette),
                            1.16,
                            0.18,
                            232,
                        ),
                        (0.025 + contrastMix * 0.02 + pulseMix * 0.012) * field.strength,
                    )
                    ctx.stroke()
                }

                drawLensDiamond(
                    ctx,
                    field.leftNode,
                    field.rightNode,
                    palette,
                    time,
                    pulseMix,
                    contrastMix,
                    field.strength * 0.75,
                )
            }

            ctx.restore()

            ctx.save()
            ctx.globalCompositeOperation = 'lighter'
            for (const spark of sparks) {
                const ribbon = renderedRibbons[spark.ribbon % Math.max(1, renderedRibbons.length)]
                const progress = wrap(time * (0.08 + speedMix * 0.14) * spark.speed + spark.phase, 1)
                const point = samplePolyline(ribbon.points, progress)
                const tail = sampleSegment(ribbon.points, clamp(progress - 0.05 - spark.size * 0.02, 0, 1), progress, 8)
                const sparkColor = richenRgb(
                    mixRgb(
                        sampleSpectralPalette(spark.colorBias + time * 0.05 + ribbon.fieldIndex * 0.07, palette),
                        ribbon.core,
                        0.35,
                    ),
                    1.22,
                    0.1,
                    244,
                )
                const sparkFringeA = richenRgb(
                    sampleSpectralPalette(spark.colorBias + 0.16 - time * 0.035, palette),
                    1.28,
                    0.08,
                    246,
                )
                const sparkFringeB = richenRgb(
                    sampleSpectralPalette(spark.colorBias + 0.61 + time * 0.03, palette),
                    1.28,
                    0.08,
                    246,
                )
                const sparkAxis = normalizePoint(subPoint(ribbon.rightNode, ribbon.leftNode), { x: 1, y: 0 })
                const sparkNormal = perpendicular(sparkAxis)
                const sparkSpread = ribbon.width * (0.18 + spark.size * 0.12)
                const fringeTailA = offsetPoints(tail, scalePoint(sparkNormal, sparkSpread * 0.45))
                const fringeTailB = offsetPoints(tail, scalePoint(sparkNormal, -sparkSpread * 0.45))

                drawPolyline(ctx, fringeTailA)
                ctx.lineWidth = Math.max(1, ribbon.width * (0.05 + spark.size * 0.05))
                ctx.strokeStyle = rgba(sparkFringeA, (0.16 + contrastMix * 0.03 + pulseMix * 0.03) * ribbon.strength)
                ctx.stroke()

                drawPolyline(ctx, fringeTailB)
                ctx.lineWidth = Math.max(1, ribbon.width * (0.05 + spark.size * 0.05))
                ctx.strokeStyle = rgba(sparkFringeB, (0.16 + contrastMix * 0.03 + pulseMix * 0.03) * ribbon.strength)
                ctx.stroke()

                drawPolyline(ctx, tail)
                ctx.lineWidth = Math.max(1, ribbon.width * (0.08 + spark.size * 0.08))
                ctx.strokeStyle = rgba(sparkColor, (0.18 + pulseMix * 0.05) * ribbon.strength)
                ctx.stroke()

                ctx.fillStyle = rgba(
                    richenRgb(mixRgb(sparkColor, ribbon.core, 0.18), 1.08, 0.06, 248),
                    0.34 * ribbon.strength,
                )
                ctx.beginPath()
                ctx.arc(point.x, point.y, Math.max(0.75, ribbon.width * (0.08 + spark.size * 0.05)), 0, TAU)
                ctx.fill()
            }
            ctx.restore()
        }
    },
    {
        description:
            'A luminous Einstein bridge stretches between two gravitational anchors, with braided spacetime ribbons ferrying color and light across the gap',
        presets: [
            {
                controls: {
                    background: '#050913',
                    color1: '#20f0ff',
                    color2: '#9056ff',
                    color3: '#ff5cb7',
                    contrast: 88,
                    depth: 86,
                    drift: 36,
                    geometry: 'Braided Flux',
                    pulse: 68,
                    speed: 7,
                    theme: 'Event Horizon',
                    thickness: 72,
                    twist: 88,
                },
                description:
                    'Two bright anchors trade ribbons of cyan, violet, and rose, woven tight enough to feel like engineered spacetime',
                name: 'Causal Braid',
            },
            {
                controls: {
                    background: '#020814',
                    color1: '#20f0ff',
                    color2: '#9056ff',
                    color3: '#ff5cb7',
                    contrast: 82,
                    depth: 74,
                    drift: 18,
                    geometry: 'Prism Bridge',
                    pulse: 44,
                    speed: 5,
                    theme: 'Spectral',
                    thickness: 58,
                    twist: 76,
                },
                description:
                    'A crystalline bridge of spectral facets refracts its own currents, like a portal built by impossible optics',
                name: 'Prism Treaty',
            },
            {
                controls: {
                    background: '#04110d',
                    color1: '#20f0ff',
                    color2: '#41ff7d',
                    color3: '#88ffd3',
                    contrast: 58,
                    depth: 62,
                    drift: 64,
                    geometry: 'Tidal Lattice',
                    pulse: 82,
                    speed: 4,
                    theme: 'Quantum',
                    thickness: 48,
                    twist: 54,
                },
                description:
                    'Soft green currents pulse through a breathing lattice, more tidal than violent, like spacetime acting as fabric instead of vacuum',
                name: 'Living Continuum',
            },
            {
                controls: {
                    background: '#09040f',
                    color1: '#ff5c8a',
                    color2: '#ff8a00',
                    color3: '#ffd166',
                    contrast: 84,
                    depth: 92,
                    drift: 42,
                    geometry: 'Halo Exchange',
                    pulse: 96,
                    speed: 8,
                    theme: 'Solar Flare',
                    thickness: 76,
                    twist: 42,
                },
                description:
                    'Amber and gold halos whip around twin stars as if a bridge has been forged out of solar weather',
                name: 'Coronal Relay',
            },
            {
                controls: {
                    background: '#080607',
                    color1: '#ff6200',
                    color2: '#b4154e',
                    color3: '#ff9340',
                    contrast: 78,
                    depth: 88,
                    drift: 26,
                    geometry: 'Braided Flux',
                    pulse: 74,
                    speed: 6,
                    theme: 'Abyssal',
                    thickness: 82,
                    twist: 72,
                },
                description:
                    'A heavier, predatory bridge burns with ember-red traffic, like two hungry wells laced together by molten gravity',
                name: 'Abyssal Exchange',
            },
            {
                controls: {
                    background: '#0a0416',
                    color1: '#7f5cff',
                    color2: '#ff3ca8',
                    color3: '#ff7bd0',
                    contrast: 46,
                    depth: 40,
                    drift: 72,
                    geometry: 'Halo Exchange',
                    pulse: 26,
                    speed: 2,
                    theme: 'Void Gate',
                    thickness: 36,
                    twist: 28,
                },
                description:
                    'The bridge relaxes into a slow violet conversation, with distant halos whispering between two patient anchors',
                name: 'Quiet Transfer',
            },
        ],
    },
)
