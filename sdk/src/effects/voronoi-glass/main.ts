import { canvas, combo } from '@hypercolor/sdk'

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

const PALETTE_NAMES = ['Glacier', 'Prism', 'Lagoon', 'Rose Quartz', 'Solar']
const TAU = Math.PI * 2
const DEFAULT_BACKGROUND = '#050913'

const PALETTES: Record<string, GlassPalette> = {
    Glacier: {
        bg:     { r: 4, g: 9, b: 22 },
        colors: [
            hsvToRgb(205, 0.82, 0.82),
            hsvToRgb(188, 0.76, 0.86),
            hsvToRgb(244, 0.70, 0.76),
        ],
        edge:  hsvToRgb(192, 0.38, 0.98),
        glint: hsvToRgb(220, 0.16, 1),
    },
    Prism: {
        bg:     { r: 7, g: 4, b: 18 },
        colors: [
            hsvToRgb(188, 0.84, 0.90),
            hsvToRgb(284, 0.82, 0.88),
            hsvToRgb(320, 0.76, 0.86),
        ],
        edge:  hsvToRgb(196, 0.34, 1),
        glint: hsvToRgb(285, 0.18, 1),
    },
    Lagoon: {
        bg:     { r: 2, g: 12, b: 20 },
        colors: [
            hsvToRgb(150, 0.80, 0.82),
            hsvToRgb(184, 0.84, 0.88),
            hsvToRgb(214, 0.76, 0.82),
        ],
        edge:  hsvToRgb(182, 0.32, 0.98),
        glint: hsvToRgb(200, 0.12, 1),
    },
    'Rose Quartz': {
        bg:     { r: 10, g: 5, b: 18 },
        colors: [
            hsvToRgb(330, 0.76, 0.88),
            hsvToRgb(286, 0.78, 0.84),
            hsvToRgb(214, 0.74, 0.80),
        ],
        edge:  hsvToRgb(320, 0.32, 0.98),
        glint: hsvToRgb(222, 0.16, 1),
    },
    Solar: {
        bg:     { r: 14, g: 6, b: 12 },
        colors: [
            hsvToRgb(24, 0.88, 0.92),
            hsvToRgb(330, 0.78, 0.88),
            hsvToRgb(292, 0.74, 0.82),
        ],
        edge:  hsvToRgb(28, 0.30, 1),
        glint: hsvToRgb(340, 0.18, 1),
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

function scaleRgb(color: Rgb, scale: number): Rgb {
    return {
        r: clamp(Math.round(color.r * scale), 0, 255),
        g: clamp(Math.round(color.g * scale), 0, 255),
        b: clamp(Math.round(color.b * scale), 0, 255),
    }
}

function addRgb(base: Rgb, glow: Rgb): Rgb {
    return {
        r: clamp(base.r + glow.r, 0, 255),
        g: clamp(base.g + glow.g, 0, 255),
        b: clamp(base.b + glow.b, 0, 255),
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

function getPalette(name: string): GlassPalette {
    return PALETTES[name] ?? PALETTES.Glacier
}

export default canvas.stateful('Voronoi Glass', {
    speed:      [1, 10, 4],
    density:    [10, 100, 42],
    refraction: [0, 100, 58],
    edgeGlow:   [0, 100, 66],
    palette:    combo('Palette', PALETTE_NAMES, { default: 'Glacier' }),
    background: DEFAULT_BACKGROUND,
}, () => {
    let seeds: GlassSeed[] = []
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
        const refractionMix = clamp((c.refraction as number) / 100, 0, 1)
        const edgeGlowMix = clamp((c.edgeGlow as number) / 100, 0, 1)
        const palette = getPalette(c.palette as string)
        const background = mixRgb(
            palette.bg,
            hexToRgb(c.background as string, palette.bg),
            0.3,
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

        const driftRate = 0.12 + speedMix * 0.3
        const refractionRate = 0.16 + speedMix * 0.2
        const glazeRate = 0.08 + speedMix * 0.12

        for (let i = 0; i < seeds.length; i++) {
            const seed = seeds[i]
            const orbitX = Math.sin(
                time * (driftRate * (0.75 + seed.driftX * 0.75)) + seed.phaseX,
            ) * (0.05 + seed.driftX * 0.08)
            const orbitY = Math.cos(
                time * (driftRate * (0.65 + seed.driftY * 0.85)) + seed.phaseY,
            ) * (0.05 + seed.driftY * 0.07)

            positions[i] = {
                x: clamp(0.08 + seed.baseX * 0.84 + orbitX, 0.04, 0.96) * w,
                y: clamp(0.08 + seed.baseY * 0.84 + orbitY, 0.04, 0.96) * h,
                phase: seed.phase,
                colorIndex: Math.floor(seed.colorBias * palette.colors.length) % palette.colors.length,
            }
        }

        const cellRadius = Math.sqrt((w * h) / positions.length) * (0.7 + (1 - densityMix) * 0.18)
        const edgeWidth = cellRadius * (0.12 + edgeGlowMix * 0.12)

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
                    0.1 + refractionMix * 0.2 + edgeFactor * 0.18 + glaze * 0.1,
                    0.08,
                    0.54,
                )

                const interior = mixRgb(
                    baseColor,
                    palette.glint,
                    0.12 + facet * 0.18 + centerFactor * 0.1,
                )
                const refracted = mixRgb(interior, neighborColor, tintMix)

                const light = clamp(
                    0.24 + centerFactor * 0.3 + facet * 0.12 + glaze * 0.08,
                    0.16,
                    0.84,
                )

                let pixel = mixRgb(background, refracted, light)

                const edgeColor = mixRgb(palette.edge, neighborColor, 0.12 + glaze * 0.12)
                pixel = addRgb(
                    pixel,
                    scaleRgb(edgeColor, edgeFactor * (0.08 + edgeGlowMix * 0.24)),
                )

                const vignette = clamp(1 - (cx * cx + cy * cy) * 0.16, 0.76, 1)
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
        ctx.globalCompositeOperation = 'screen'

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
        glow.addColorStop(0, toRgba(palette.glint, 0.1 + edgeGlowMix * 0.04))
        glow.addColorStop(0.55, toRgba(palette.edge, 0.05 + edgeGlowMix * 0.05))
        glow.addColorStop(1, 'rgba(0,0,0,0)')
        ctx.fillStyle = glow
        ctx.fillRect(0, 0, w, h)

        const glazeOverlay = ctx.createLinearGradient(0, 0, w, h)
        const shimmerStop = clamp(
            0.42 + Math.sin(time * (0.06 + speedMix * 0.05)) * 0.18,
            0.18,
            0.82,
        )
        glazeOverlay.addColorStop(0, toRgba(palette.glint, 0.02))
        glazeOverlay.addColorStop(shimmerStop, toRgba(palette.edge, 0.05 + refractionMix * 0.04))
        glazeOverlay.addColorStop(1, 'rgba(0,0,0,0)')
        ctx.fillStyle = glazeOverlay
        ctx.fillRect(0, 0, w, h)

        ctx.restore()
    }
}, {
    description: 'Slow-drifting Voronoi glass with broad LED-safe cells and soft prism edges',
})
