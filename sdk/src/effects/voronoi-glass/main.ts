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

type PaletteName = 'Custom' | 'Prism' | 'Solar' | 'Rose Quartz' | 'Lagoon' | 'Glacier'

const PALETTE_NAMES: PaletteName[] = ['Prism', 'Lagoon', 'Glacier', 'Rose Quartz', 'Solar', 'Custom']
const TAU = Math.PI * 2
const DEFAULT_BACKGROUND = '#050913'
const DEFAULT_COLOR_1 = '#22f0ff'
const DEFAULT_COLOR_2 = '#ff46c8'
const DEFAULT_COLOR_3 = '#3659ff'

const PALETTES: Record<Exclude<PaletteName, 'Custom'>, GlassPalette> = {
    Glacier: {
        bg:     { r: 4, g: 9, b: 22 },
        colors: [
            hsvToRgb(192, 0.94, 0.94),
            hsvToRgb(224, 0.88, 0.92),
            hsvToRgb(278, 0.76, 0.82),
        ],
        edge:  hsvToRgb(206, 0.78, 0.90),
        glint: hsvToRgb(248, 0.58, 0.72),
    },
    Prism: {
        bg:     { r: 6, g: 3, b: 18 },
        colors: [
            hsvToRgb(188, 0.96, 0.98),
            hsvToRgb(244, 0.90, 0.98),
            hsvToRgb(320, 0.92, 0.96),
        ],
        edge:  hsvToRgb(214, 0.86, 0.92),
        glint: hsvToRgb(292, 0.62, 0.76),
    },
    Lagoon: {
        bg:     { r: 2, g: 12, b: 20 },
        colors: [
            hsvToRgb(156, 0.88, 0.88),
            hsvToRgb(188, 0.96, 0.96),
            hsvToRgb(252, 0.86, 0.92),
        ],
        edge:  hsvToRgb(182, 0.78, 0.90),
        glint: hsvToRgb(210, 0.56, 0.72),
    },
    'Rose Quartz': {
        bg:     { r: 10, g: 5, b: 18 },
        colors: [
            hsvToRgb(334, 0.90, 0.96),
            hsvToRgb(286, 0.86, 0.92),
            hsvToRgb(210, 0.90, 0.96),
        ],
        edge:  hsvToRgb(318, 0.80, 0.92),
        glint: hsvToRgb(254, 0.52, 0.74),
    },
    Solar: {
        bg:     { r: 14, g: 6, b: 12 },
        colors: [
            hsvToRgb(22, 0.92, 0.98),
            hsvToRgb(320, 0.92, 0.96),
            hsvToRgb(248, 0.90, 0.94),
        ],
        edge:  hsvToRgb(24, 0.88, 0.92),
        glint: hsvToRgb(334, 0.58, 0.76),
    },
}

function clamp(value: number, min: number, max: number): number {
    return Math.max(min, Math.min(max, value))
}

function smoothstep(edge0: number, edge1: number, value: number): number {
    if (edge0 === edge1) {
        return value < edge0 ? 0 : 1
    }

    const t = clamp((value - edge0) / (edge1 - edge0), 0, 1)
    return t * t * (3 - 2 * t)
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
        r: Math.round((r + m) * 255),
        g: Math.round((g + m) * 255),
        b: Math.round((b + m) * 255),
    }
}

function hexToRgb(hex: string, fallback: Rgb): Rgb {
    const normalized = hex.trim().replace('#', '')
    const full = normalized.length === 3
        ? `${normalized[0]}${normalized[0]}${normalized[1]}${normalized[1]}${normalized[2]}${normalized[2]}`
        : normalized

    if (!/^[0-9a-fA-F]{6}$/.test(full)) {
        return fallback
    }

    const value = Number.parseInt(full, 16)

    return {
        r: (value >> 16) & 255,
        g: (value >> 8) & 255,
        b: value & 255,
    }
}

function mixRgb(a: Rgb, b: Rgb, amount: number): Rgb {
    const t = clamp(amount, 0, 1)

    return {
        r: Math.round(a.r + (b.r - a.r) * t),
        g: Math.round(a.g + (b.g - a.g) * t),
        b: Math.round(a.b + (b.b - a.b) * t),
    }
}

function saturateRgb(color: Rgb, amount: number): Rgb {
    const luminance = color.r * 0.2126 + color.g * 0.7152 + color.b * 0.0722

    return {
        r: clamp(Math.round(luminance + (color.r - luminance) * amount), 0, 255),
        g: clamp(Math.round(luminance + (color.g - luminance) * amount), 0, 255),
        b: clamp(Math.round(luminance + (color.b - luminance) * amount), 0, 255),
    }
}

function scaleRgb(color: Rgb, scale: number): Rgb {
    return {
        r: clamp(Math.round(color.r * scale), 0, 255),
        g: clamp(Math.round(color.g * scale), 0, 255),
        b: clamp(Math.round(color.b * scale), 0, 255),
    }
}

function toRgba(color: Rgb, alpha: number): string {
    return `rgba(${color.r},${color.g},${color.b},${clamp(alpha, 0, 1).toFixed(3)})`
}

function createSeed(index: number): GlassSeed {
    const i = index + 1

    return {
        baseX:     hash(i * 1.37 + 0.31),
        baseY:     hash(i * 2.11 + 1.91),
        driftX:    hash(i * 3.73 + 4.17),
        driftY:    hash(i * 5.19 + 2.83),
        phaseX:    hash(i * 7.43 + 9.21) * TAU,
        phaseY:    hash(i * 11.87 + 5.61) * TAU,
        phase:     hash(i * 13.91 + 6.23) * TAU,
        colorBias: hash(i * 17.07 + 3.47),
    }
}

function resolvePalette(
    name: PaletteName,
    color1: string,
    color2: string,
    color3: string,
): GlassPalette {
    if (name !== 'Custom') {
        return PALETTES[name] ?? PALETTES.Prism
    }

    const primary = hexToRgb(color1, PALETTES.Prism.colors[0])
    const secondary = hexToRgb(color2, PALETTES.Prism.colors[1])
    const accent = hexToRgb(color3, PALETTES.Prism.colors[2])
    const edge = saturateRgb(mixRgb(primary, secondary, 0.38), 1.16)
    const glint = scaleRgb(saturateRgb(mixRgb(primary, accent, 0.52), 1.10), 0.82)

    return {
        bg: { r: 5, g: 9, b: 19 },
        colors: [primary, secondary, accent],
        edge,
        glint,
    }
}

export default canvas.stateful('Voronoi Glass', {
    speed:      [1, 10, 4],
    density:    [10, 100, 42],
    drift:      num('Drift', [0, 100], 44),
    refraction: [0, 100, 58],
    contrast:   num('Contrast', [0, 100], 56),
    edgeGlow:   [0, 100, 66],
    glaze:      num('Glaze', [0, 100], 18),
    palette:    combo('Palette', PALETTE_NAMES, { default: 'Prism' }),
    color1:     color('Color 1', DEFAULT_COLOR_1),
    color2:     color('Color 2', DEFAULT_COLOR_2),
    color3:     color('Color 3', DEFAULT_COLOR_3),
    background: color('Backdrop', DEFAULT_BACKGROUND),
}, () => {
    const seeds: GlassSeed[] = []
    let seedCount = 0
    let frame: ImageData | null = null
    let frameKey = ''

    function ensureSeedCount(count: number): void {
        const target = clamp(Math.round(count), 6, 20)
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

    function ensureFrame(ctx: CanvasRenderingContext2D): ImageData {
        const { width, height } = ctx.canvas
        const key = `${width}:${height}`

        if (!frame || frameKey !== key) {
            frame = ctx.createImageData(width, height)
            frameKey = key
        }

        return frame
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
        const background = mixRgb(
            palette.bg,
            hexToRgb(c.background as string, palette.bg),
            0.82,
        )

        const w = ctx.canvas.width
        const h = ctx.canvas.height
        const minDim = Math.min(w, h)

        if (w === 0 || h === 0) return

        ensureSeedCount(6 + Math.round(densityMix * 12))
        if (seeds.length === 0) return

        const image = ensureFrame(ctx)
        const data = image.data
        const positions: SeedPosition[] = new Array(seeds.length)

        const driftRate = 0.08 + speedMix * 0.22 + driftMix * 0.16
        const refractionRate = 0.16 + speedMix * 0.2
        const glazeRate = 0.06 + speedMix * 0.08 + glazeMix * 0.08

        for (let i = 0; i < seeds.length; i++) {
            const seed = seeds[i]
            const orbitX = Math.sin(
                time * (driftRate * (0.75 + seed.driftX * 0.75)) + seed.phaseX,
            ) * ((0.03 + seed.driftX * 0.06) * (0.45 + driftMix * 0.95))
            const orbitY = Math.cos(
                time * (driftRate * (0.65 + seed.driftY * 0.85)) + seed.phaseY,
            ) * ((0.03 + seed.driftY * 0.05) * (0.45 + driftMix * 0.95))

            positions[i] = {
                x: clamp(0.08 + seed.baseX * 0.84 + orbitX, 0.04, 0.96) * w,
                y: clamp(0.08 + seed.baseY * 0.84 + orbitY, 0.04, 0.96) * h,
                phase: seed.phase,
                colorIndex: Math.floor(seed.colorBias * palette.colors.length) % palette.colors.length,
            }
        }

        const cellRadius = Math.sqrt((w * h) / positions.length) * (0.7 + (1 - densityMix) * 0.18)
        const edgeWidth = cellRadius * (0.10 + edgeGlowMix * 0.10 + contrastMix * 0.05)

        let offset = 0

        for (let y = 0; y < h; y++) {
            const cy = (y - h * 0.5) / Math.max(1, h * 0.5)

            for (let x = 0; x < w; x++) {
                const cx = (x - w * 0.5) / Math.max(1, w * 0.5)

                let nearest2 = Number.POSITIVE_INFINITY
                let second2 = Number.POSITIVE_INFINITY
                let nearestIndex = 0
                let secondIndex = 0
                let nearestDx = 0
                let nearestDy = 0

                for (let i = 0; i < positions.length; i++) {
                    const seed = positions[i]
                    const dx = x - seed.x
                    const dy = y - seed.y
                    const distance2 = dx * dx + dy * dy

                    if (distance2 < nearest2) {
                        second2 = nearest2
                        secondIndex = nearestIndex
                        nearest2 = distance2
                        nearestIndex = i
                        nearestDx = dx
                        nearestDy = dy
                    } else if (distance2 < second2) {
                        second2 = distance2
                        secondIndex = i
                    }
                }

                const lead = positions[nearestIndex]
                const neighbor = positions[secondIndex]
                const baseColor = palette.colors[lead.colorIndex]
                const neighborColor = palette.colors[neighbor.colorIndex]

                const nearestDistance = Math.sqrt(nearest2)
                const secondDistance = Math.sqrt(second2)
                const edgeFactor = 1 - smoothstep(0, edgeWidth, secondDistance - nearestDistance)
                const centerFactor = 1 - smoothstep(cellRadius * 0.18, cellRadius * 0.94, nearestDistance)

                const facet = 0.5 + 0.5 * Math.sin(
                    nearestDx * (0.052 + refractionMix * 0.04)
                    + nearestDy * (0.041 + refractionMix * 0.035)
                    + lead.phase
                    + time * refractionRate,
                )

                const glaze = 0.5 + 0.5 * Math.sin(
                    x * 0.018
                    + y * 0.021
                    + lead.phase * 1.4
                    + time * glazeRate,
                )

                const tintMix = clamp(
                    0.04 + refractionMix * 0.10 + edgeFactor * 0.14 + glaze * glazeMix * 0.04,
                    0.03,
                    0.26,
                )

                const interior = mixRgb(
                    baseColor,
                    palette.glint,
                    0.04 + facet * (0.06 + glazeMix * 0.04) + centerFactor * (0.04 + contrastMix * 0.06),
                )
                const refracted = mixRgb(interior, neighborColor, tintMix)

                const light = clamp(
                    0.12 + centerFactor * (0.22 + contrastMix * 0.12) + facet * 0.06 + glaze * glazeMix * 0.03,
                    0.10,
                    0.66 + contrastMix * 0.06,
                )

                let pixel = mixRgb(background, refracted, light)

                const edgeColor = saturateRgb(mixRgb(palette.edge, neighborColor, 0.06 + glaze * 0.05), 1.08)
                pixel = mixRgb(
                    pixel,
                    edgeColor,
                    edgeFactor * (0.10 + edgeGlowMix * 0.26 + contrastMix * 0.06),
                )

                const vignette = clamp(1 - (cx * cx + cy * cy) * (0.12 + contrastMix * 0.08), 0.72, 1)
                pixel = scaleRgb(pixel, vignette)

                data[offset] = pixel.r
                data[offset + 1] = pixel.g
                data[offset + 2] = pixel.b
                data[offset + 3] = 255
                offset += 4
            }
        }

        ctx.putImageData(image, 0, 0)

        ctx.save()
        ctx.globalCompositeOperation = 'lighter'

        const glowX = w * (0.35 + Math.sin(time * (0.09 + speedMix * 0.08)) * 0.12)
        const glowY = h * (0.32 + Math.cos(time * (0.07 + speedMix * 0.06)) * 0.14)
        const glow = ctx.createRadialGradient(
            glowX,
            glowY,
            0,
            glowX,
            glowY,
            minDim * (0.62 + refractionMix * 0.18),
        )
        glow.addColorStop(0, toRgba(palette.glint, 0.012 + glazeMix * 0.014 + edgeGlowMix * 0.010))
        glow.addColorStop(0.55, toRgba(palette.edge, 0.008 + edgeGlowMix * 0.016))
        glow.addColorStop(1, 'rgba(0,0,0,0)')
        ctx.fillStyle = glow
        ctx.fillRect(0, 0, w, h)

        const glazeOverlay = ctx.createLinearGradient(0, 0, w, h)
        const shimmerStop = clamp(
            0.42 + Math.sin(time * (0.06 + speedMix * 0.05)) * 0.18,
            0.18,
            0.82,
        )
        glazeOverlay.addColorStop(0, toRgba(palette.glint, 0.004 + glazeMix * 0.006))
        glazeOverlay.addColorStop(
            shimmerStop,
            toRgba(palette.edge, 0.010 + glazeMix * 0.010 + refractionMix * 0.010),
        )
        glazeOverlay.addColorStop(1, 'rgba(0,0,0,0)')
        ctx.fillStyle = glazeOverlay
        ctx.fillRect(0, 0, w, h)

        ctx.restore()
    }
}, {
    description: 'LED-friendly stained glass cells with deeper controls, lower white skew, and restrained glaze overlays',
})
