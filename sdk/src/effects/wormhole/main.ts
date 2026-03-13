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

interface Slice {
    progress: number
    alpha: number
    points: Point[]
    wall: Rgb
    rim: Rgb
    lineWidth: number
}

const THEME_NAMES = ['Abyssal', 'Custom', 'Event Horizon', 'Quantum', 'Solar Flare', 'Spectral', 'Void Gate'] as const
const GEOMETRY_NAMES = ['Hex Gate', 'Organic Fold', 'Prism Rift', 'Pulse Ring'] as const

type ThemeName = typeof THEME_NAMES[number]
type GeometryName = typeof GEOMETRY_NAMES[number]

const DEFAULT_BACKGROUND = '#050913'
const DEFAULT_COLOR_1 = '#20f0ff'
const DEFAULT_COLOR_2 = '#9056ff'
const DEFAULT_COLOR_3 = '#ff5cb7'
const TAU = Math.PI * 2

const THEMES: Record<Exclude<ThemeName, 'Custom'>, ThemePalette> = {
    'Event Horizon': {
        background: '#040814',
        wallA: '#082c74',
        wallB: '#14d8ff',
        accent: '#7d52ff',
        core: '#2af6ff',
    },
    'Void Gate': {
        background: '#0a0416',
        wallA: '#4d127f',
        wallB: '#ff3ca8',
        accent: '#7f5cff',
        core: '#ff7bd0',
    },
    Quantum: {
        background: '#031112',
        wallA: '#00a9a2',
        wallB: '#2eff78',
        accent: '#20f0ff',
        core: '#87ffbe',
    },
    Abyssal: {
        background: '#080607',
        wallA: '#4c0c18',
        wallB: '#ff6200',
        accent: '#b4154e',
        core: '#ff9340',
    },
    'Solar Flare': {
        background: '#140700',
        wallA: '#ff5e00',
        wallB: '#ffb100',
        accent: '#ff4b7a',
        core: '#ffd166',
    },
    Spectral: {
        background: '#060612',
        wallA: '#1f4fff',
        wallB: '#18f0ff',
        accent: '#ff4fb4',
        core: '#8d5cff',
    },
}

function clamp(value: number, min: number, max: number): number {
    return Math.max(min, Math.min(max, value))
}

function smoothstep(edge0: number, edge1: number, value: number): number {
    const t = clamp((value - edge0) / (edge1 - edge0), 0, 1)
    return t * t * (3 - 2 * t)
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

function hexToRgb(hex: string, fallback: Rgb = { r: 0, g: 0, b: 0 }): Rgb {
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

function scaleRgb(color: Rgb, factor: number): Rgb {
    return {
        r: clamp(Math.round(color.r * factor), 0, 255),
        g: clamp(Math.round(color.g * factor), 0, 255),
        b: clamp(Math.round(color.b * factor), 0, 255),
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
            background: hexToRgb(preset.background),
            wallA: hexToRgb(preset.wallA),
            wallB: hexToRgb(preset.wallB),
            accent: hexToRgb(preset.accent),
            core: hexToRgb(preset.core),
        }
    }

    const wallA = hexToRgb(color1, hexToRgb(DEFAULT_COLOR_1))
    const wallB = hexToRgb(color2, hexToRgb(DEFAULT_COLOR_2))
    const accent = saturateRgb(hexToRgb(color3, hexToRgb(DEFAULT_COLOR_3)), 1.18)
    const backdrop = hexToRgb(background, hexToRgb(DEFAULT_BACKGROUND))

    return {
        background: backdrop,
        wallA,
        wallB,
        accent,
        core: saturateRgb(mixRgb(accent, wallB, 0.35), 1.12),
    }
}

function samplePalette(t: number, palette: ResolvedPalette): Rgb {
    const phase = wrap(t, 1)
    if (phase < 0.34) {
        return mixRgb(palette.wallA, palette.accent, phase / 0.34)
    }
    if (phase < 0.68) {
        return mixRgb(palette.accent, palette.wallB, (phase - 0.34) / 0.34)
    }
    return mixRgb(palette.wallB, palette.wallA, (phase - 0.68) / 0.32)
}

function geometryPointCount(geometry: GeometryName): number {
    if (geometry === 'Hex Gate') return 6
    if (geometry === 'Pulse Ring') return 16
    if (geometry === 'Prism Rift') return 8
    return 10
}

function buildRing(
    geometry: GeometryName,
    centerX: number,
    centerY: number,
    radiusX: number,
    radiusY: number,
    rotation: number,
    sliceIndex: number,
    time: number,
    twistMix: number,
    pulseMix: number,
): Point[] {
    const count = geometryPointCount(geometry)
    const points: Point[] = []

    for (let index = 0; index < count; index++) {
        const baseAngle = (index / count) * TAU + rotation
        let localRadiusX = radiusX
        let localRadiusY = radiusY

        if (geometry === 'Prism Rift') {
            const fold = index % 2 === 0 ? 1 : 0.72 + pulseMix * 0.08
            localRadiusX *= fold
            localRadiusY *= fold
        } else if (geometry === 'Organic Fold') {
            const noise = Math.sin(baseAngle * 3 + time * (0.8 + twistMix) + sliceIndex * 0.37)
            const jitter = (hash(sliceIndex * 17 + index * 5) - 0.5) * 0.18
            const fold = 1 + noise * (0.08 + pulseMix * 0.05) + jitter
            localRadiusX *= fold
            localRadiusY *= fold * (0.94 + hash(sliceIndex * 11 + index) * 0.12)
        } else if (geometry === 'Pulse Ring') {
            const wave = Math.sin(baseAngle * 2 + time * 1.7 + sliceIndex * 0.29)
            const fold = 1 + wave * (0.04 + pulseMix * 0.04)
            localRadiusX *= fold
            localRadiusY *= fold
        }

        points.push({
            x: centerX + Math.cos(baseAngle) * localRadiusX,
            y: centerY + Math.sin(baseAngle) * localRadiusY,
        })
    }

    return points
}

function drawClosedPath(ctx: CanvasRenderingContext2D, points: Point[]): void {
    if (points.length === 0) return
    ctx.beginPath()
    ctx.moveTo(points[0].x, points[0].y)
    for (let index = 1; index < points.length; index++) {
        ctx.lineTo(points[index].x, points[index].y)
    }
    ctx.closePath()
}

export default canvas.stateful('Wormhole', {
    theme:      combo('Theme', THEME_NAMES, { default: 'Event Horizon', group: 'Scene' }),
    geometry:   combo('Geometry', GEOMETRY_NAMES, { default: 'Hex Gate', group: 'Scene' }),
    speed:      num('Speed', [1, 10], 5, { group: 'Motion' }),
    depth:      num('Depth', [0, 100], 66, { group: 'Motion' }),
    twist:      num('Twist', [0, 100], 58, { group: 'Motion' }),
    drift:      num('Drift', [0, 100], 42, { group: 'Motion' }),
    pulse:      num('Pulse', [0, 100], 34, { group: 'Atmosphere' }),
    thickness:  num('Wall Thickness', [0, 100], 56, { group: 'Atmosphere' }),
    contrast:   num('Contrast', [0, 100], 60, { group: 'Atmosphere' }),
    color1:     color('Color 1', DEFAULT_COLOR_1, { group: 'Color' }),
    color2:     color('Color 2', DEFAULT_COLOR_2, { group: 'Color' }),
    color3:     color('Color 3', DEFAULT_COLOR_3, { group: 'Color' }),
    background: color('Backdrop', DEFAULT_BACKGROUND, { group: 'Color' }),
}, () => {
    return (ctx, time, controls) => {
        const width = ctx.canvas.width
        const height = ctx.canvas.height
        const minDim = Math.min(width, height)

        if (width === 0 || height === 0) return

        const speedMix = clamp((((controls.speed as number) ?? 5) - 1) / 9, 0, 1)
        const depthMix = clamp(((controls.depth as number) ?? 66) / 100, 0, 1)
        const twistMix = clamp(((controls.twist as number) ?? 58) / 100, 0, 1)
        const driftMix = clamp(((controls.drift as number) ?? 42) / 100, 0, 1)
        const pulseMix = clamp(((controls.pulse as number) ?? 34) / 100, 0, 1)
        const thicknessMix = clamp(((controls.thickness as number) ?? 56) / 100, 0, 1)
        const contrastMix = clamp(((controls.contrast as number) ?? 60) / 100, 0, 1)
        const theme = (controls.theme as ThemeName) ?? 'Event Horizon'
        const geometry = (controls.geometry as GeometryName) ?? 'Hex Gate'

        const palette = resolvePalette(
            theme,
            controls.color1 as string,
            controls.color2 as string,
            controls.color3 as string,
            controls.background as string,
        )

        const centerX = width * 0.5
        const centerY = height * 0.5
        const driftTime = time * (0.16 + speedMix * 0.12)
        const vanishingX = centerX + Math.sin(driftTime * 0.67) * width * (0.04 + driftMix * 0.12)
            + Math.sin(driftTime * 0.31 + 1.8) * width * driftMix * 0.04
        const vanishingY = centerY + Math.cos(driftTime * 0.53 + 0.6) * height * (0.05 + driftMix * 0.14)
            + Math.cos(driftTime * 0.23 + 3.1) * height * driftMix * 0.03

        ctx.fillStyle = rgb(palette.background)
        ctx.fillRect(0, 0, width, height)

        const aura = ctx.createRadialGradient(vanishingX, vanishingY, 0, vanishingX, vanishingY, minDim * 0.78)
        aura.addColorStop(0, rgba(mixRgb(palette.core, palette.background, 0.42), 0.18 + contrastMix * 0.08))
        aura.addColorStop(0.48, rgba(mixRgb(palette.accent, palette.background, 0.70), 0.06 + pulseMix * 0.04))
        aura.addColorStop(1, rgba(palette.background, 0))
        ctx.fillStyle = aura
        ctx.fillRect(0, 0, width, height)

        const sliceCount = Math.round(14 + depthMix * 18)
        const travel = 0.04 + speedMix * 0.055
        const fadeIn = 0.10
        const fadeOut = 0.10
        const slices: Slice[] = []

        for (let index = 0; index < sliceCount; index++) {
            const progress = wrap(index / sliceCount + time * travel, 1)

            // Fade envelope: smooth in/out at wrap boundaries to prevent pop
            const alpha = smoothstep(0, fadeIn, progress) * smoothstep(0, fadeOut, 1 - progress)
            if (alpha < 0.005) continue

            const depthCurve = progress * progress * (3 - 2 * progress)  // smoothstep curve instead of pow
            const centerBlend = 1 - progress
            const pulseWave = 1 + Math.sin(time * (1.4 + pulseMix * 1.9) + index * 0.47) * (0.03 + pulseMix * 0.06)
            const baseRadius = minDim * (0.05 + depthCurve * (0.18 + depthMix * 0.42)) * pulseWave
            const radiusX = baseRadius * (geometry === 'Prism Rift' ? 1.08 : 1)
            const radiusY = baseRadius * (geometry === 'Pulse Ring' ? 0.72 : geometry === 'Organic Fold' ? 0.82 : 0.88)
            const ringCenterX = lerp(centerX, vanishingX, centerBlend)
            const ringCenterY = lerp(centerY, vanishingY, centerBlend)
            const rotation = time * (0.3 + speedMix * 0.22) + index * 0.18 + depthCurve * (0.8 + twistMix * 4.2)
            const points = buildRing(
                geometry,
                ringCenterX,
                ringCenterY,
                radiusX,
                radiusY,
                rotation,
                index,
                time,
                twistMix,
                pulseMix,
            )

            const colorPhase = wrap(progress * 0.82 + time * (0.03 + speedMix * 0.02), 1)
            const edgeBase = samplePalette(colorPhase, palette)
            const wall = mixRgb(
                palette.background,
                saturateRgb(edgeBase, 1.08),
                0.16 + contrastMix * 0.18 + progress * 0.10,
            )
            const rim = mixRgb(edgeBase, palette.core, 0.18 + pulseMix * 0.14)
            const lineWidth = Math.max(
                1.8,
                minDim * (0.004 + thicknessMix * 0.014) * (0.34 + depthCurve),
            )

            slices.push({
                progress,
                alpha,
                points,
                wall,
                rim,
                lineWidth,
            })
        }

        slices.sort((left, right) => left.progress - right.progress)

        // Max gap between adjacent sorted slices before we skip the wall panel
        const wrapGapThreshold = 2.5 / sliceCount

        for (let index = 1; index < slices.length; index++) {
            const previous = slices[index - 1]
            const current = slices[index]

            // Skip wall panels that span the wrap boundary
            if (current.progress - previous.progress > wrapGapThreshold) continue

            const panelAlpha = Math.min(previous.alpha, current.alpha)
            if (panelAlpha < 0.01) continue

            const pointCount = Math.min(previous.points.length, current.points.length)

            for (let pointIndex = 0; pointIndex < pointCount; pointIndex++) {
                const nextIndex = (pointIndex + 1) % pointCount
                const pulseBand = 0.5 + 0.5 * Math.sin(time * 1.3 + index * 0.41 + pointIndex * 0.77)
                const wallColor = mixRgb(
                    previous.wall,
                    current.wall,
                    0.42 + pulseBand * 0.12,
                )

                ctx.fillStyle = rgba(scaleRgb(wallColor, 0.86 + contrastMix * 0.18), panelAlpha)
                ctx.beginPath()
                ctx.moveTo(previous.points[pointIndex].x, previous.points[pointIndex].y)
                ctx.lineTo(previous.points[nextIndex].x, previous.points[nextIndex].y)
                ctx.lineTo(current.points[nextIndex].x, current.points[nextIndex].y)
                ctx.lineTo(current.points[pointIndex].x, current.points[pointIndex].y)
                ctx.closePath()
                ctx.fill()
            }
        }

        for (const slice of slices) {
            if (slice.alpha < 0.01) continue

            drawClosedPath(ctx, slice.points)
            ctx.lineWidth = slice.lineWidth
            ctx.strokeStyle = rgba(slice.rim, slice.alpha)
            ctx.stroke()

            drawClosedPath(ctx, slice.points)
            ctx.lineWidth = Math.max(1, slice.lineWidth * 0.38)
            ctx.strokeStyle = rgba(mixRgb(slice.rim, palette.core, 0.35), (0.38 + pulseMix * 0.18) * slice.alpha)
            ctx.stroke()
        }

        const coreRadius = minDim * (0.032 + pulseMix * 0.018) * (1 + Math.sin(time * 2.1) * 0.12)
        const core = buildRing(
            geometry,
            vanishingX,
            vanishingY,
            coreRadius * 1.12,
            coreRadius * (geometry === 'Pulse Ring' ? 0.78 : 0.92),
            time * (0.6 + twistMix * 1.2),
            99,
            time,
            twistMix,
            pulseMix,
        )

        drawClosedPath(ctx, core)
        ctx.fillStyle = rgb(mixRgb(palette.background, palette.core, 0.38 + contrastMix * 0.12))
        ctx.fill()

        drawClosedPath(ctx, core)
        ctx.lineWidth = Math.max(2, minDim * (0.010 + thicknessMix * 0.012))
        ctx.strokeStyle = rgb(saturateRgb(palette.core, 1.12))
        ctx.stroke()
    }
}, {
    description: 'Geometric tunnel with solid walls, drifting vanishing point, and LED-friendly depth control',
    presets: [
        {
            name: 'Event Horizon Collapse',
            description: 'The point of no return — hexagonal walls compress as spacetime folds inward at terminal velocity',
            controls: {
                theme: 'Event Horizon',
                geometry: 'Hex Gate',
                speed: 9,
                depth: 95,
                twist: 85,
                drift: 20,
                pulse: 70,
                thickness: 80,
                contrast: 90,
                color1: '#20f0ff',
                color2: '#9056ff',
                color3: '#ff5cb7',
                background: '#050913',
            },
        },
        {
            name: 'Organic Spore Channel',
            description: 'Biological passage through a living organism — pulsing organic folds in quantum greens and teals',
            controls: {
                theme: 'Quantum',
                geometry: 'Organic Fold',
                speed: 3,
                depth: 55,
                twist: 40,
                drift: 75,
                pulse: 85,
                thickness: 40,
                contrast: 45,
                color1: '#20f0ff',
                color2: '#9056ff',
                color3: '#ff5cb7',
                background: '#050913',
            },
        },
        {
            name: 'Abyssal Maw',
            description: 'Swallowed by a creature of deep space — fiery rings contract and expand like the breathing of a void leviathan',
            controls: {
                theme: 'Abyssal',
                geometry: 'Pulse Ring',
                speed: 5,
                depth: 80,
                twist: 30,
                drift: 60,
                pulse: 95,
                thickness: 70,
                contrast: 75,
                color1: '#20f0ff',
                color2: '#9056ff',
                color3: '#ff5cb7',
                background: '#050913',
            },
        },
        {
            name: 'Prism Gate Ascension',
            description: 'Ascending through crystalline dimensions — spectral light refracts through razor-edged prismatic geometry',
            controls: {
                theme: 'Spectral',
                geometry: 'Prism Rift',
                speed: 6,
                depth: 70,
                twist: 100,
                drift: 35,
                pulse: 50,
                thickness: 55,
                contrast: 85,
                color1: '#20f0ff',
                color2: '#9056ff',
                color3: '#ff5cb7',
                background: '#050913',
            },
        },
        {
            name: 'Void Gate Meditation',
            description: 'Hovering at the threshold of nothingness — slow violet geometry drifts through absolute stillness',
            controls: {
                theme: 'Void Gate',
                geometry: 'Hex Gate',
                speed: 1,
                depth: 30,
                twist: 15,
                drift: 90,
                pulse: 20,
                thickness: 30,
                contrast: 35,
                color1: '#20f0ff',
                color2: '#9056ff',
                color3: '#ff5cb7',
                background: '#050913',
            },
        },
    ],
})
