import { canvas, color, combo, num } from '@hypercolor/sdk'

interface GlassSeed {
    baseX: number
    baseY: number
    driftX: number
    driftY: number
    phaseX: number
    phaseY: number
    phase: number
    colorBias: number
}

interface SeedPosition {
    x: number
    y: number
    phase: number
    colorIndex: number
}

interface Rgb {
    r: number
    g: number
    b: number
}

interface GlassPalette {
    bg: Rgb
    colors: Rgb[]
    edge: Rgb
    glint: Rgb
}

interface Vec2 {
    x: number
    y: number
}

type PaletteName = 'Custom' | 'Prism' | 'Solar' | 'Rose Quartz' | 'Lagoon' | 'Glacier'

const PALETTE_NAMES: PaletteName[] = ['Prism', 'Lagoon', 'Glacier', 'Rose Quartz', 'Solar', 'Custom']
const TAU = Math.PI * 2
const DEFAULT_BACKGROUND = '#050913'
const DEFAULT_COLOR_1 = '#22f0ff'
const DEFAULT_COLOR_2 = '#ff46c8'
const DEFAULT_COLOR_3 = '#3659ff'
const MIN_CELL_COUNT = 7
const MAX_CELL_COUNT = 16

const PALETTES: Record<Exclude<PaletteName, 'Custom'>, GlassPalette> = {
    Glacier: {
        bg: { b: 22, g: 9, r: 4 },
        colors: [hsvToRgb(192, 0.94, 0.94), hsvToRgb(224, 0.88, 0.92), hsvToRgb(278, 0.76, 0.82)],
        edge: hsvToRgb(206, 0.78, 0.9),
        glint: hsvToRgb(248, 0.58, 0.72),
    },
    Lagoon: {
        bg: { b: 20, g: 12, r: 2 },
        colors: [hsvToRgb(156, 0.88, 0.88), hsvToRgb(188, 0.96, 0.96), hsvToRgb(252, 0.86, 0.92)],
        edge: hsvToRgb(182, 0.78, 0.9),
        glint: hsvToRgb(210, 0.56, 0.72),
    },
    Prism: {
        bg: { b: 18, g: 3, r: 6 },
        colors: [hsvToRgb(188, 0.96, 0.98), hsvToRgb(244, 0.9, 0.98), hsvToRgb(320, 0.92, 0.96)],
        edge: hsvToRgb(214, 0.86, 0.92),
        glint: hsvToRgb(292, 0.62, 0.76),
    },
    'Rose Quartz': {
        bg: { b: 18, g: 5, r: 10 },
        colors: [hsvToRgb(334, 0.9, 0.96), hsvToRgb(286, 0.86, 0.92), hsvToRgb(210, 0.9, 0.96)],
        edge: hsvToRgb(318, 0.8, 0.92),
        glint: hsvToRgb(254, 0.52, 0.74),
    },
    Solar: {
        bg: { b: 12, g: 6, r: 14 },
        colors: [hsvToRgb(22, 0.92, 0.98), hsvToRgb(320, 0.92, 0.96), hsvToRgb(248, 0.9, 0.94)],
        edge: hsvToRgb(24, 0.88, 0.92),
        glint: hsvToRgb(334, 0.58, 0.76),
    },
}

function clamp(value: number, min: number, max: number): number {
    return Math.max(min, Math.min(max, value))
}

function hash(value: number): number {
    const x = Math.sin(value * 127.1 + 311.7) * 43758.5453123
    return x - Math.floor(x)
}

function hsvToRgb(h: number, s: number, v: number): Rgb {
    const hue = ((h % 360) + 360) % 360
    const sat = clamp(s, 0, 1)
    const val = clamp(v, 0, 1)
    const chroma = val * sat
    const x = chroma * (1 - Math.abs(((hue / 60) % 2) - 1))
    const m = val - chroma

    let r = 0
    let g = 0
    let b = 0

    if (hue < 60) [r, g, b] = [chroma, x, 0]
    else if (hue < 120) [r, g, b] = [x, chroma, 0]
    else if (hue < 180) [r, g, b] = [0, chroma, x]
    else if (hue < 240) [r, g, b] = [0, x, chroma]
    else if (hue < 300) [r, g, b] = [x, 0, chroma]
    else [r, g, b] = [chroma, 0, x]

    return {
        b: Math.round((b + m) * 255),
        g: Math.round((g + m) * 255),
        r: Math.round((r + m) * 255),
    }
}

function hexToRgb(hex: string, fallback: Rgb): Rgb {
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

function saturateRgb(color: Rgb, amount: number): Rgb {
    const luminance = color.r * 0.2126 + color.g * 0.7152 + color.b * 0.0722

    return {
        b: clamp(Math.round(luminance + (color.b - luminance) * amount), 0, 255),
        g: clamp(Math.round(luminance + (color.g - luminance) * amount), 0, 255),
        r: clamp(Math.round(luminance + (color.r - luminance) * amount), 0, 255),
    }
}

function scaleRgb(color: Rgb, scale: number): Rgb {
    return {
        b: clamp(Math.round(color.b * scale), 0, 255),
        g: clamp(Math.round(color.g * scale), 0, 255),
        r: clamp(Math.round(color.r * scale), 0, 255),
    }
}

function toRgba(color: Rgb, alpha: number): string {
    return `rgba(${color.r},${color.g},${color.b},${clamp(alpha, 0, 1).toFixed(3)})`
}

function createSeed(index: number): GlassSeed {
    const i = index + 1

    return {
        baseX: hash(i * 1.37 + 0.31),
        baseY: hash(i * 2.11 + 1.91),
        colorBias: hash(i * 17.07 + 3.47),
        driftX: hash(i * 3.73 + 4.17),
        driftY: hash(i * 5.19 + 2.83),
        phase: hash(i * 13.91 + 6.23) * TAU,
        phaseX: hash(i * 7.43 + 9.21) * TAU,
        phaseY: hash(i * 11.87 + 5.61) * TAU,
    }
}

function resolvePalette(name: PaletteName, color1: string, color2: string, color3: string): GlassPalette {
    if (name !== 'Custom') {
        return PALETTES[name] ?? PALETTES.Prism
    }

    const primary = hexToRgb(color1, PALETTES.Prism.colors[0])
    const secondary = hexToRgb(color2, PALETTES.Prism.colors[1])
    const accent = hexToRgb(color3, PALETTES.Prism.colors[2])
    const edge = saturateRgb(mixRgb(primary, secondary, 0.38), 1.16)
    const glint = scaleRgb(saturateRgb(mixRgb(primary, accent, 0.52), 1.1), 0.82)

    return {
        bg: { b: 19, g: 9, r: 5 },
        colors: [primary, secondary, accent],
        edge,
        glint,
    }
}

function intersectHalfPlane(start: Vec2, end: Vec2, nx: number, ny: number, limit: number): Vec2 {
    const dx = end.x - start.x
    const dy = end.y - start.y
    const denominator = dx * nx + dy * ny

    if (Math.abs(denominator) < 0.00001) {
        return { x: end.x, y: end.y }
    }

    const t = clamp((limit - (start.x * nx + start.y * ny)) / denominator, 0, 1)
    return {
        x: start.x + dx * t,
        y: start.y + dy * t,
    }
}

function clipPolygonHalfPlane(polygon: Vec2[], nx: number, ny: number, limit: number): Vec2[] {
    if (polygon.length === 0) {
        return polygon
    }

    const clipped: Vec2[] = []
    let previous = polygon[polygon.length - 1]
    let previousInside = previous.x * nx + previous.y * ny <= limit + 0.0001

    for (const point of polygon) {
        const inside = point.x * nx + point.y * ny <= limit + 0.0001

        if (inside !== previousInside) {
            clipped.push(intersectHalfPlane(previous, point, nx, ny, limit))
        }

        if (inside) {
            clipped.push(point)
        }

        previous = point
        previousInside = inside
    }

    return clipped
}

function polygonCentroid(polygon: Vec2[]): Vec2 {
    let signedArea = 0
    let centroidX = 0
    let centroidY = 0

    for (let i = 0; i < polygon.length; i++) {
        const current = polygon[i]
        const next = polygon[(i + 1) % polygon.length]
        const cross = current.x * next.y - next.x * current.y
        signedArea += cross
        centroidX += (current.x + next.x) * cross
        centroidY += (current.y + next.y) * cross
    }

    if (Math.abs(signedArea) < 0.0001) {
        const total = polygon.reduce(
            (sum, point) => ({
                x: sum.x + point.x,
                y: sum.y + point.y,
            }),
            { x: 0, y: 0 },
        )
        return {
            x: total.x / Math.max(1, polygon.length),
            y: total.y / Math.max(1, polygon.length),
        }
    }

    return {
        x: centroidX / (3 * signedArea),
        y: centroidY / (3 * signedArea),
    }
}

function averageRadius(polygon: Vec2[], center: Vec2): number {
    const total = polygon.reduce((sum, point) => sum + Math.hypot(point.x - center.x, point.y - center.y), 0)
    return total / Math.max(1, polygon.length)
}

function insetPolygon(polygon: Vec2[], center: Vec2, scale: number): Vec2[] {
    return polygon.map((point) => ({
        x: center.x + (point.x - center.x) * scale,
        y: center.y + (point.y - center.y) * scale,
    }))
}

function drawPolygonPath(ctx: CanvasRenderingContext2D, polygon: Vec2[]): void {
    if (polygon.length < 3) {
        return
    }

    ctx.beginPath()
    ctx.moveTo(polygon[0].x, polygon[0].y)
    for (let i = 1; i < polygon.length; i++) {
        ctx.lineTo(polygon[i].x, polygon[i].y)
    }
    ctx.closePath()
}

function buildVoronoiCell(index: number, positions: SeedPosition[], width: number, height: number): Vec2[] {
    const seed = positions[index]
    let polygon: Vec2[] = [
        { x: 0, y: 0 },
        { x: width, y: 0 },
        { x: width, y: height },
        { x: 0, y: height },
    ]

    for (let i = 0; i < positions.length; i++) {
        if (i === index) {
            continue
        }

        const other = positions[i]
        const nx = other.x - seed.x
        const ny = other.y - seed.y
        const limit = (other.x * other.x + other.y * other.y - seed.x * seed.x - seed.y * seed.y) * 0.5
        polygon = clipPolygonHalfPlane(polygon, nx, ny, limit)

        if (polygon.length < 3) {
            return []
        }
    }

    return polygon
}

export default canvas.stateful(
    'Voronoi Glass',
    {
        palette: combo('Palette', PALETTE_NAMES, { default: 'Prism', group: 'Color' }),
        color1: color('Color 1', DEFAULT_COLOR_1, { group: 'Color' }),
        color2: color('Color 2', DEFAULT_COLOR_2, { group: 'Color' }),
        color3: color('Color 3', DEFAULT_COLOR_3, { group: 'Color' }),
        background: color('Backdrop', DEFAULT_BACKGROUND, { group: 'Color' }),
        speed: num('Speed', [1, 10], 4, { group: 'Motion' }),
        drift: num('Drift', [0, 100], 44, { group: 'Motion' }),
        density: num('Density', [10, 100], 42, { group: 'Geometry' }),
        refraction: num('Refraction', [0, 100], 58, { group: 'Geometry' }),
        contrast: num('Contrast', [0, 100], 56, { group: 'Geometry' }),
        edgeGlow: num('Edge Glow', [0, 100], 66, { group: 'Geometry' }),
        glaze: num('Glaze', [0, 100], 18, { group: 'Geometry' }),
    },
    () => {
        const seeds: GlassSeed[] = []
        let seedCount = 0

        function ensureSeedCount(count: number): void {
            const target = clamp(Math.round(count), MIN_CELL_COUNT, MAX_CELL_COUNT)
            if (target === seedCount && seeds.length === target) return

            if (target > seeds.length) {
                for (let i = seeds.length; i < target; i++) {
                    seeds.push(createSeed(i))
                }
            } else {
                seeds.length = target
            }

            seedCount = target
        }

        return (ctx, time, c) => {
            const speedMix = clamp(((c.speed as number) - 1) / 9, 0, 1)
            const densityMix = clamp(((c.density as number) - 10) / 90, 0, 1)
            const driftMix = clamp((c.drift as number) / 100, 0, 1)
            const refractionMix = clamp((c.refraction as number) / 100, 0, 1)
            const contrastMix = clamp((c.contrast as number) / 100, 0, 1)
            const edgeGlowMix = clamp((c.edgeGlow as number) / 100, 0, 1)
            const glazeMix = clamp((c.glaze as number) / 100, 0, 1)
            const palette = resolvePalette(
                c.palette as PaletteName,
                c.color1 as string,
                c.color2 as string,
                c.color3 as string,
            )
            const background = mixRgb(palette.bg, hexToRgb(c.background as string, palette.bg), 0.78)

            const w = ctx.canvas.width
            const h = ctx.canvas.height
            const minDim = Math.min(w, h)
            const centerX = w * 0.5
            const centerY = h * 0.5

            if (w === 0 || h === 0) return

            ensureSeedCount(MIN_CELL_COUNT + Math.round(densityMix * (MAX_CELL_COUNT - MIN_CELL_COUNT)))
            if (seeds.length === 0) return

            const positions: SeedPosition[] = new Array(seeds.length)
            const driftRate = 0.06 + speedMix * 0.18 + driftMix * 0.18
            const swirlRate = 0.1 + speedMix * 0.16 + refractionMix * 0.12

            for (let i = 0; i < seeds.length; i++) {
                const seed = seeds[i]
                const orbitX =
                    Math.sin(time * (driftRate * (0.75 + seed.driftX * 0.75)) + seed.phaseX) *
                    w *
                    ((0.012 + seed.driftX * 0.028) * (0.35 + driftMix * 0.85))
                const orbitY =
                    Math.cos(time * (driftRate * (0.65 + seed.driftY * 0.85)) + seed.phaseY) *
                    h *
                    ((0.015 + seed.driftY * 0.032) * (0.35 + driftMix * 0.9))
                const baseX = (0.1 + seed.baseX * 0.8) * w + orbitX
                const baseY = (0.1 + seed.baseY * 0.8) * h + orbitY
                const localX = baseX - centerX
                const localY = baseY - centerY
                const radiusMix = Math.hypot(localX / Math.max(1, w), localY / Math.max(1, h))
                const swirlAngle =
                    Math.sin(time * (swirlRate * (0.85 + seed.colorBias * 0.9)) + seed.phase) *
                    (0.18 + refractionMix * 0.2) *
                    (0.35 + radiusMix * 1.8)
                const swirlCos = Math.cos(swirlAngle)
                const swirlSin = Math.sin(swirlAngle)

                positions[i] = {
                    colorIndex: Math.floor(seed.colorBias * palette.colors.length) % palette.colors.length,
                    phase: seed.phase,
                    x: clamp(centerX + localX * swirlCos - localY * swirlSin, w * 0.06, w * 0.94),
                    y: clamp(centerY + localX * swirlSin + localY * swirlCos, h * 0.06, h * 0.94),
                }
            }

            ctx.fillStyle = `rgb(${background.r},${background.g},${background.b})`
            ctx.fillRect(0, 0, w, h)

            const ambientGlow = ctx.createRadialGradient(
                w * (0.28 + Math.sin(time * (0.1 + speedMix * 0.05)) * 0.15),
                h * (0.24 + Math.cos(time * (0.08 + speedMix * 0.04)) * 0.16),
                0,
                centerX,
                centerY,
                minDim * (0.9 + refractionMix * 0.2),
            )
            ambientGlow.addColorStop(0, toRgba(palette.glint, 0.03 + glazeMix * 0.02))
            ambientGlow.addColorStop(0.38, toRgba(palette.colors[0], 0.018 + contrastMix * 0.02))
            ambientGlow.addColorStop(1, 'rgba(0,0,0,0)')
            ctx.fillStyle = ambientGlow
            ctx.fillRect(0, 0, w, h)

            const ambientWash = ctx.createLinearGradient(
                w * (0.08 + Math.sin(time * 0.07) * 0.1),
                0,
                w * (0.92 + Math.cos(time * 0.06) * 0.08),
                h,
            )
            ambientWash.addColorStop(0, toRgba(palette.colors[1], 0.02 + glazeMix * 0.01))
            ambientWash.addColorStop(0.5, 'rgba(0,0,0,0)')
            ambientWash.addColorStop(1, toRgba(palette.edge, 0.018 + refractionMix * 0.018))
            ctx.fillStyle = ambientWash
            ctx.fillRect(0, 0, w, h)

            const cells: Array<{ centroid: Vec2; polygon: Vec2[]; radius: number; seed: SeedPosition }> = []
            for (let i = 0; i < positions.length; i++) {
                const polygon = buildVoronoiCell(i, positions, w, h)
                if (polygon.length < 3) {
                    continue
                }

                const centroid = polygonCentroid(polygon)
                cells.push({
                    centroid,
                    polygon,
                    radius: averageRadius(polygon, centroid),
                    seed: positions[i],
                })
            }

            cells.sort((left, right) => right.radius - left.radius)

            ctx.save()
            ctx.lineCap = 'round'
            ctx.lineJoin = 'round'

            for (const cell of cells) {
                const accentColor = palette.colors[(cell.seed.colorIndex + 1) % palette.colors.length]
                const baseColor = palette.colors[cell.seed.colorIndex]
                const shimmer = 0.5 + 0.5 * Math.sin(time * (0.24 + speedMix * 0.18) + cell.seed.phase)
                const refractionDrift = time * (0.18 + speedMix * 0.22) + cell.seed.phase

                const fillGradient = ctx.createLinearGradient(
                    cell.seed.x - Math.cos(refractionDrift) * cell.radius * 0.8,
                    cell.seed.y - Math.sin(refractionDrift) * cell.radius * 0.8,
                    cell.centroid.x + Math.cos(refractionDrift + Math.PI * 0.5) * cell.radius,
                    cell.centroid.y + Math.sin(refractionDrift + Math.PI * 0.5) * cell.radius,
                )
                const deepTone = mixRgb(background, baseColor, 0.26 + contrastMix * 0.24)
                const refracted = mixRgb(baseColor, accentColor, 0.16 + refractionMix * 0.36)
                const highlight = scaleRgb(
                    saturateRgb(mixRgb(palette.glint, baseColor, 0.36 + shimmer * 0.18), 1.12),
                    0.72 + glazeMix * 0.08 + shimmer * 0.12,
                )

                fillGradient.addColorStop(0, toRgba(deepTone, 0.96))
                fillGradient.addColorStop(0.64, toRgba(refracted, 0.9))
                fillGradient.addColorStop(1, toRgba(highlight, 0.84))

                drawPolygonPath(ctx, cell.polygon)
                ctx.fillStyle = fillGradient
                ctx.fill()

                ctx.save()
                drawPolygonPath(ctx, cell.polygon)
                ctx.clip()

                const beamAngle = cell.seed.phase + time * (0.11 + speedMix * 0.09)
                const beamDx = Math.cos(beamAngle)
                const beamDy = Math.sin(beamAngle)
                const beam = ctx.createLinearGradient(
                    cell.centroid.x - beamDx * cell.radius * 1.35 - beamDy * cell.radius * 0.45,
                    cell.centroid.y - beamDy * cell.radius * 1.35 + beamDx * cell.radius * 0.45,
                    cell.centroid.x + beamDx * cell.radius * 1.35 + beamDy * cell.radius * 0.45,
                    cell.centroid.y + beamDy * cell.radius * 1.35 - beamDx * cell.radius * 0.45,
                )
                beam.addColorStop(0, 'rgba(0,0,0,0)')
                beam.addColorStop(0.48, toRgba(palette.glint, 0.025 + glazeMix * 0.08 + refractionMix * 0.05))
                beam.addColorStop(0.54, toRgba(highlight, 0.07 + edgeGlowMix * 0.08))
                beam.addColorStop(1, 'rgba(0,0,0,0)')
                ctx.fillStyle = beam
                ctx.fillRect(
                    cell.centroid.x - cell.radius * 1.8,
                    cell.centroid.y - cell.radius * 1.8,
                    cell.radius * 3.6,
                    cell.radius * 3.6,
                )
                ctx.restore()

                const shardCenter = {
                    x:
                        cell.centroid.x +
                        Math.cos(time * (0.72 + speedMix * 0.35) + cell.seed.phase) * cell.radius * 0.16,
                    y:
                        cell.centroid.y +
                        Math.sin(time * (0.58 + speedMix * 0.28) + cell.seed.phase * 1.3) * cell.radius * 0.16,
                }
                const shardPolygon = insetPolygon(cell.polygon, cell.centroid, 0.42 + shimmer * 0.16 + glazeMix * 0.05)
                for (let i = 0; i < shardPolygon.length; i++) {
                    const current = shardPolygon[i]
                    const next = shardPolygon[(i + 1) % shardPolygon.length]
                    const shardPulse =
                        0.5 + 0.5 * Math.sin(time * (0.85 + speedMix * 0.45) + cell.seed.phase * 1.1 + i * 1.4)

                    if (shardPulse <= 0.2) {
                        continue
                    }

                    ctx.beginPath()
                    ctx.moveTo(shardCenter.x, shardCenter.y)
                    ctx.lineTo(current.x, current.y)
                    ctx.lineTo(next.x, next.y)
                    ctx.closePath()
                    ctx.fillStyle = toRgba(
                        mixRgb(highlight, accentColor, (i % 3) / 2),
                        0.018 + shardPulse * 0.05 + glazeMix * 0.02,
                    )
                    ctx.fill()
                }

                const leadColor = saturateRgb(mixRgb(palette.edge, accentColor, 0.1 + shimmer * 0.08), 1.08)
                drawPolygonPath(ctx, cell.polygon)
                ctx.strokeStyle = toRgba(leadColor, 0.22 + edgeGlowMix * 0.3 + contrastMix * 0.06)
                ctx.lineWidth = 1.2 + edgeGlowMix * 2.2
                ctx.stroke()

                ctx.setLineDash([cell.radius * 0.22, cell.radius * 0.12])
                ctx.lineDashOffset = -(time * (16 + speedMix * 16) + cell.seed.phase * cell.radius * 0.35)
                drawPolygonPath(ctx, cell.polygon)
                ctx.strokeStyle = toRgba(palette.glint, 0.04 + edgeGlowMix * 0.08 + glazeMix * 0.02)
                ctx.lineWidth = 0.8 + edgeGlowMix * 1.1
                ctx.stroke()
                ctx.setLineDash([])

                const innerPolygon = insetPolygon(cell.polygon, cell.centroid, 0.78 - glazeMix * 0.12)
                drawPolygonPath(ctx, innerPolygon)
                ctx.strokeStyle = toRgba(palette.glint, 0.03 + glazeMix * 0.06 + refractionMix * 0.04)
                ctx.lineWidth = 0.8 + refractionMix * 1.3
                ctx.stroke()
            }

            ctx.restore()

            ctx.save()
            ctx.globalCompositeOperation = 'lighter'

            for (const cell of cells) {
                const sparkle = 0.5 + 0.5 * Math.sin(time * (0.55 + speedMix * 0.35) + cell.seed.phase * 1.7)
                const glowX = cell.centroid.x + Math.cos(cell.seed.phase + time * 0.4) * cell.radius * 0.18
                const glowY = cell.centroid.y + Math.sin(cell.seed.phase * 1.2 + time * 0.35) * cell.radius * 0.18
                const glow = ctx.createRadialGradient(
                    glowX,
                    glowY,
                    0,
                    glowX,
                    glowY,
                    cell.radius * (0.26 + edgeGlowMix * 0.2),
                )
                glow.addColorStop(0, toRgba(palette.glint, 0.02 + sparkle * 0.03 + glazeMix * 0.02))
                glow.addColorStop(0.55, toRgba(palette.edge, 0.014 + edgeGlowMix * 0.016))
                glow.addColorStop(1, 'rgba(0,0,0,0)')
                ctx.fillStyle = glow
                ctx.fillRect(glowX - cell.radius * 0.7, glowY - cell.radius * 0.7, cell.radius * 1.4, cell.radius * 1.4)
            }

            ctx.save()
            ctx.translate(centerX, centerY)
            ctx.rotate(-0.58 + Math.sin(time * (0.05 + speedMix * 0.03)) * 0.1)
            for (let band = 0; band < 3; band++) {
                const bandWidth = minDim * (0.09 + band * 0.018 + refractionMix * 0.03)
                const travel = ((time * (28 + speedMix * 24) + band * minDim * 0.55) % (minDim * 2.8)) - minDim * 1.4
                const ribbonColor = band % 2 === 0 ? palette.glint : palette.edge
                const ribbon = ctx.createLinearGradient(0, travel - bandWidth, 0, travel + bandWidth)
                ribbon.addColorStop(0, 'rgba(0,0,0,0)')
                ribbon.addColorStop(0.5, toRgba(ribbonColor, 0.016 + edgeGlowMix * 0.018 + glazeMix * 0.01))
                ribbon.addColorStop(1, 'rgba(0,0,0,0)')
                ctx.fillStyle = ribbon
                ctx.fillRect(-w, travel - bandWidth, w * 2, bandWidth * 2)
            }
            ctx.restore()

            const glazeOverlay = ctx.createLinearGradient(0, 0, w, h)
            const shimmerStop = clamp(0.38 + Math.sin(time * (0.05 + speedMix * 0.04)) * 0.18, 0.16, 0.84)
            glazeOverlay.addColorStop(0, toRgba(palette.glint, 0.005 + glazeMix * 0.008))
            glazeOverlay.addColorStop(
                shimmerStop,
                toRgba(palette.edge, 0.012 + glazeMix * 0.012 + refractionMix * 0.01),
            )
            glazeOverlay.addColorStop(1, 'rgba(0,0,0,0)')
            ctx.fillStyle = glazeOverlay
            ctx.fillRect(0, 0, w, h)

            const cathedralGlow = ctx.createRadialGradient(
                w * (0.34 + Math.sin(time * (0.08 + speedMix * 0.06)) * 0.1),
                h * (0.32 + Math.cos(time * (0.07 + speedMix * 0.05)) * 0.12),
                0,
                w * 0.5,
                h * 0.48,
                minDim * (0.72 + refractionMix * 0.18),
            )
            cathedralGlow.addColorStop(0, toRgba(palette.glint, 0.012 + edgeGlowMix * 0.01 + glazeMix * 0.012))
            cathedralGlow.addColorStop(0.56, toRgba(palette.edge, 0.01 + edgeGlowMix * 0.016))
            cathedralGlow.addColorStop(1, 'rgba(0,0,0,0)')
            ctx.fillStyle = cathedralGlow
            ctx.fillRect(0, 0, w, h)

            ctx.restore()
        }
    },
    {
        description:
            'Peer through luminous stained glass — Voronoi cells shift and reform, each facet glowing with cathedral light',
        presets: [
            {
                controls: {
                    background: '#0a0518',
                    color1: '#22f0ff',
                    color2: '#ff46c8',
                    color3: '#3659ff',
                    contrast: 70,
                    density: 32,
                    drift: 28,
                    edgeGlow: 85,
                    glaze: 12,
                    palette: 'Rose Quartz',
                    refraction: 72,
                    speed: 2,
                },
                description:
                    'Vast rose window flooding a stone nave with fractured color — slow-drifting cells in deep jewel tones, edges burning like leaded glass catching the last light',
                name: 'Cathedral at Twilight',
            },
            {
                controls: {
                    background: '#020814',
                    color1: '#22f0ff',
                    color2: '#ff46c8',
                    color3: '#3659ff',
                    contrast: 82,
                    density: 78,
                    drift: 16,
                    edgeGlow: 42,
                    glaze: 6,
                    palette: 'Glacier',
                    refraction: 88,
                    speed: 3,
                },
                description:
                    'Looking up through a crack in ancient ice — dense tessellations of blue-white shards refracting polar light, cold and sharp as a diamond saw',
                name: 'Glacier Crevasse',
            },
            {
                controls: {
                    background: '#0e060c',
                    color1: '#22f0ff',
                    color2: '#ff46c8',
                    color3: '#3659ff',
                    contrast: 48,
                    density: 55,
                    drift: 90,
                    edgeGlow: 72,
                    glaze: 35,
                    palette: 'Solar',
                    refraction: 65,
                    speed: 6,
                },
                description:
                    'A furnace-hot window melting at the seams — cells drift and warp like cooling magma, amber edges bleeding into deep solar orange',
                name: 'Molten Stained Glass',
            },
            {
                controls: {
                    background: '#020c14',
                    color1: '#22f0ff',
                    color2: '#ff46c8',
                    color3: '#3659ff',
                    contrast: 38,
                    density: 48,
                    drift: 60,
                    edgeGlow: 55,
                    glaze: 58,
                    palette: 'Lagoon',
                    refraction: 45,
                    speed: 2,
                },
                description:
                    'Bioluminescent jellyfish pulsing through deep ocean glass — slow, hypnotic cells in teal and violet, glazed with an unearthly aquatic shimmer',
                name: 'Abyssal Lagoon',
            },
            {
                controls: {
                    background: '#060312',
                    color1: '#22f0ff',
                    color2: '#ff46c8',
                    color3: '#3659ff',
                    contrast: 90,
                    density: 95,
                    drift: 72,
                    edgeGlow: 95,
                    glaze: 22,
                    palette: 'Prism',
                    refraction: 100,
                    speed: 8,
                },
                description:
                    'White light shatters through a room of cut diamonds — prismatic edges fire in every direction at once',
                name: 'Diamond Refraction Chamber',
            },
            {
                controls: {
                    background: '#0c0808',
                    color1: '#ff6600',
                    color2: '#ffcc00',
                    color3: '#ff0044',
                    contrast: 62,
                    density: 28,
                    drift: 95,
                    edgeGlow: 80,
                    glaze: 45,
                    palette: 'Solar',
                    refraction: 30,
                    speed: 5,
                },
                description:
                    'Stare into the corona during totality — vast amber cells drift and collide, each seam blazing with trapped solar wind',
                name: 'Eclipse Corona',
            },
            {
                controls: {
                    background: '#020204',
                    color1: '#22f0ff',
                    color2: '#ff46c8',
                    color3: '#3659ff',
                    contrast: 18,
                    density: 15,
                    drift: 10,
                    edgeGlow: 12,
                    glaze: 90,
                    palette: 'Glacier',
                    refraction: 20,
                    speed: 1,
                },
                description:
                    'Sea glass worn smooth by a thousand tides — faint cells suspended in glacial stillness, light barely remembering where it entered',
                name: 'Frozen Reliquary',
            },
        ],
    },
)
