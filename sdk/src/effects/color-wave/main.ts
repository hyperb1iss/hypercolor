import { canvas, color, combo, num, scaleContext } from '@hypercolor/sdk'

import type { Rgb } from '../_builtin/common'
import { BUILTIN_DESIGN_BASIS, hexToRgb, hslCss, mixRgb, rgbToCss, scaleRgb, withLift } from '../_builtin/common'

interface PaletteTriad {
    primary: string
    secondary: string
    accent: string
}

const CUSTOM_PALETTE = 'Custom'

// Curated palettes cover the full emotional range: electric club energy,
// cinematic warmth, clinical hard-edged tech, calm ambient. Each triad is
// tuned so any pair reads as deliberate on LED hardware.
const PALETTES: Record<string, PaletteTriad> = {
    SilkCircuit: { accent: '#ff6ac1', primary: '#e135ff', secondary: '#80ffea' },
    Cyberpunk: { accent: '#ffb300', primary: '#ff2975', secondary: '#00f0ff' },
    Aurora: { accent: '#9d00ff', primary: '#00ff9f', secondary: '#00b8ff' },
    Sunset: { accent: '#ffd23f', primary: '#ff3d7f', secondary: '#ff8c42' },
    Vaporwave: { accent: '#b967ff', primary: '#ff71ce', secondary: '#01cdfe' },
    'Ocean Deep': { accent: '#caf0f8', primary: '#0353a4', secondary: '#00b4d8' },
    Lava: { accent: '#ffba08', primary: '#d00000', secondary: '#ff6d00' },
    Pastel: { accent: '#cdb4db', primary: '#ffafcc', secondary: '#bde0fe' },
    Monochrome: { accent: '#e5e7eb', primary: '#ffffff', secondary: '#6b7280' },
    Forest: { accent: '#ffd166', primary: '#2d6a4f', secondary: '#95d5b2' },
    CMYK: { accent: '#ffff00', primary: '#00ffff', secondary: '#ff00ff' },
    'Blood Moon': { accent: '#ffb199', primary: '#8b0000', secondary: '#ff3344' },
    'Neon 80s': { accent: '#ffe66d', primary: '#ff006e', secondary: '#00f5ff' },
}

const PALETTE_NAMES: readonly string[] = [CUSTOM_PALETTE, ...Object.keys(PALETTES)]

function fract(value: number): number {
    return value - Math.floor(value)
}

function rgbaString(rgb: Rgb, alpha: number): string {
    const clamped = Math.min(1, Math.max(0, alpha))
    return `rgba(${Math.round(rgb.r)}, ${Math.round(rgb.g)}, ${Math.round(rgb.b)}, ${clamped})`
}

function resolvePalette(
    name: string,
    customPrimary: Rgb,
    customSecondary: Rgb,
    customAccent: Rgb,
): { primary: Rgb; secondary: Rgb; accent: Rgb } {
    if (name === CUSTOM_PALETTE) {
        return { accent: customAccent, primary: customPrimary, secondary: customSecondary }
    }
    const triad = PALETTES[name]
    if (!triad) {
        return { accent: customAccent, primary: customPrimary, secondary: customSecondary }
    }
    return {
        accent: hexToRgb(triad.accent),
        primary: hexToRgb(triad.primary),
        secondary: hexToRgb(triad.secondary),
    }
}

function directionAngle(direction: string): number {
    if (direction === 'Vertical') return Math.PI * 0.5
    if (direction === 'Diagonal') return Math.PI * 0.32
    return 0
}

function waveColor(
    mode: string,
    phase: number,
    primary: Rgb,
    secondary: Rgb,
    accent: Rgb,
    brightness: number,
    saturation: number,
): Rgb {
    if (mode === 'Spectrum') {
        // hslCss returns a css string; we want an Rgb value for consistent paint code.
        const cssString = hslCss(phase * 360, 62 + saturation * 30, 46 + brightness * 24, 1)
        const match = cssString.match(/rgba?\(([\d.]+),\s*([\d.]+),\s*([\d.]+)/i)
        if (match) {
            return { b: Number.parseFloat(match[3]), g: Number.parseFloat(match[2]), r: Number.parseFloat(match[1]) }
        }
        return primary
    }

    if (mode === 'Triad Cycle') {
        const t = fract(phase)
        if (t < 1 / 3) return scaleRgb(mixRgb(primary, secondary, t * 3), brightness)
        if (t < 2 / 3) return scaleRgb(mixRgb(secondary, accent, (t - 1 / 3) * 3), brightness)
        return scaleRgb(mixRgb(accent, primary, (t - 2 / 3) * 3), brightness)
    }

    return scaleRgb(mixRgb(primary, secondary, 0.5 + Math.sin(phase * Math.PI * 2) * 0.5), brightness)
}

// Paint a band whose cross-section profile morphs from soft glow (edge=0) to
// hard-edged rectangle (edge=1). At high edge the gradient collapses to a
// solid fill so the band reads as a real line, not a smear.
function paintBand(
    ctx: CanvasRenderingContext2D,
    position: number,
    halfWidth: number,
    span: number,
    rgb: Rgb,
    alpha: number,
    edge: number,
): void {
    // Above this threshold the taper is so narrow it just looks hazy.
    // Skip the gradient entirely and draw a solid bar.
    if (edge >= 0.85) {
        ctx.fillStyle = rgbaString(rgb, alpha)
        ctx.fillRect(position - halfWidth, -span, halfWidth * 2, span * 2)
        return
    }

    const soft = 1 - edge
    const plateau = edge * 0.92
    const shoulder = (1 - plateau) / 2
    const outerAlpha = alpha * soft * 0.18

    const gradient = ctx.createLinearGradient(position - halfWidth, 0, position + halfWidth, 0)
    gradient.addColorStop(0, 'rgba(0, 0, 0, 0)')
    if (soft > 0.05) {
        gradient.addColorStop(Math.max(0, shoulder * 0.35), rgbaString(rgb, outerAlpha))
    }
    gradient.addColorStop(Math.max(0, shoulder), rgbaString(rgb, alpha))
    gradient.addColorStop(Math.min(1, 1 - shoulder), rgbaString(rgb, alpha))
    if (soft > 0.05) {
        gradient.addColorStop(Math.min(1, 1 - shoulder * 0.35), rgbaString(rgb, outerAlpha))
    }
    gradient.addColorStop(1, 'rgba(0, 0, 0, 0)')
    ctx.fillStyle = gradient
    ctx.fillRect(position - halfWidth, -span, halfWidth * 2, span * 2)
}

export default canvas(
    'Color Wave',
    {
        mode: combo('Mode', ['Sweep', 'Counterflow', 'Tunnel'], { default: 'Sweep', group: 'Scene' }),
        palette: combo('Palette', PALETTE_NAMES, {
            default: 'SilkCircuit',
            group: 'Colors',
            tooltip: 'Curated triad. Overrides color pickers unless set to Custom.',
        }),
        colorMode: combo('Color Mode', ['Custom Duo', 'Triad Cycle', 'Spectrum'], {
            default: 'Triad Cycle',
            group: 'Colors',
        }),
        primaryColor: color('Primary Color', '#80ffea', { group: 'Colors' }),
        secondaryColor: color('Secondary Color', '#e135ff', { group: 'Colors' }),
        accentColor: color('Accent Color', '#ff9f45', { group: 'Colors' }),
        direction: combo('Direction', ['Horizontal', 'Vertical', 'Diagonal'], {
            default: 'Horizontal',
            group: 'Motion',
        }),
        speed: num('Speed', [0, 100], 54, { group: 'Motion' }),
        density: num('Density', [0, 100], 42, { group: 'Motion' }),
        width: num('Band Width', [4, 100], 34, { group: 'Motion' }),
        trail: num('Trail', [0, 100], 46, { group: 'Motion' }),
        warp: num('Warp', [0, 100], 20, { group: 'Motion' }),
        edge: num('Edge', [0, 100], 18, {
            group: 'Motion',
            tooltip: 'Crispness. 0 is bloomed glow, 100 is hard-edged lines.',
        }),
        saturation: num('Saturation', [0, 100], 72, { group: 'Output' }),
        brightness: num('Brightness', [0, 100], 88, { group: 'Output' }),
    },
    (ctx, time, controls) => {
        const s = scaleContext(ctx.canvas, BUILTIN_DESIGN_BASIS)
        const width = s.width
        const height = s.height
        const mode = controls.mode as string
        const direction = controls.direction as string
        const colorMode = controls.colorMode as string
        const paletteName = controls.palette as string
        const speed = (controls.speed as number) / 100
        const density = (controls.density as number) / 100
        const bandWidth = s.ds(12 + (controls.width as number) * 1.4)
        const trail = (controls.trail as number) / 100
        const warp = (controls.warp as number) / 100
        const edge = Math.min(1, Math.max(0, (controls.edge as number) / 100))
        const brightness = (controls.brightness as number) / 100
        const saturation = (controls.saturation as number) / 100
        const customPrimary = hexToRgb(controls.primaryColor as string)
        const customSecondary = hexToRgb(controls.secondaryColor as string)
        const customAccent = hexToRgb(controls.accentColor as string)
        const { primary, secondary, accent } = resolvePalette(paletteName, customPrimary, customSecondary, customAccent)

        // Background: crisp presets want flat near-black so bands pop with
        // full contrast; soft presets keep the gradient underbed so bloom
        // has something warm to land on.
        const bgBright = brightness * 0.12 * Math.max(0, 1 - edge * 1.1)
        const base = scaleRgb(mixRgb(primary, secondary, 0.18), bgBright)
        const accentMix = 0.25 * Math.max(0, 1 - edge * 1.3)
        const endScale = Math.max(0, 0.9 - edge * 0.9)
        const fill = ctx.createLinearGradient(0, 0, width, height)
        fill.addColorStop(0, rgbToCss(base))
        fill.addColorStop(1, rgbToCss(scaleRgb(mixRgb(base, accent, accentMix), endScale)))
        ctx.fillStyle = fill
        ctx.fillRect(0, 0, width, height)

        ctx.save()
        // Additive compositing creates bloom; at medium-to-high edge we switch to
        // source-over so overlapping bands don't blow out into white smear.
        ctx.globalCompositeOperation = edge > 0.4 ? 'source-over' : 'lighter'

        if (mode === 'Tunnel') {
            const ringCount = Math.round(5 + density * 8)
            const centerX = s.dx(160 + Math.sin(time * (0.24 + warp * 0.7)) * 20)
            const centerY = s.dy(100 + Math.cos(time * (0.19 + warp * 0.6)) * 14)
            const maxRadius = s.ds(160)

            for (let i = 0; i < ringCount; i++) {
                const phase = fract(time * (0.08 + speed * 0.42) + i / ringCount)
                const radius = s.ds(10) + phase * maxRadius
                const softRingAlpha = (1 - phase) * (0.16 + trail * 0.32)
                const crispRingAlpha = (1 - phase * 0.85) * (0.55 + trail * 0.45)
                const alpha = softRingAlpha * (1 - edge) + crispRingAlpha * edge
                const ringColor = waveColor(
                    colorMode,
                    phase + i * 0.08,
                    primary,
                    secondary,
                    accent,
                    brightness,
                    saturation,
                )

                ctx.save()
                ctx.lineWidth = bandWidth * (0.28 + (1 - phase) * 0.18) * (0.7 + edge * 0.8)
                // Shadow blur is the classic bloom vector; taper it out with edge.
                ctx.shadowBlur = bandWidth * (0.55 + trail * 0.45) * (1 - edge)
                ctx.shadowColor = rgbaString(ringColor, 1)
                ctx.strokeStyle = rgbaString(ringColor, 1)
                ctx.globalAlpha = alpha
                ctx.beginPath()
                ctx.arc(centerX, centerY, radius, 0, Math.PI * 2)
                ctx.stroke()
                ctx.restore()
            }
        } else {
            const angle = directionAngle(direction)
            const span = Math.hypot(width, height) * 1.28
            const bandCount = Math.round(4 + density * 8)
            const travel = span * (2.1 + trail * 0.9)

            ctx.translate(width / 2, height / 2)
            ctx.rotate(angle + Math.sin(time * 0.22) * warp * 0.12)

            for (let i = 0; i < bandCount; i++) {
                const basePhase = fract(time * (0.06 + speed * 0.36) + i / bandCount)
                const signedPhase = mode === 'Counterflow' && i % 2 === 1 ? 1 - basePhase : basePhase
                const drift = Math.sin(time * 0.8 + i * 1.4) * warp * span * 0.08
                const position = signedPhase * travel - travel * 0.5 + drift
                const bandColor = waveColor(
                    colorMode,
                    basePhase + i * 0.1,
                    primary,
                    secondary,
                    accent,
                    brightness,
                    saturation,
                )

                // Alpha interpolates from layered glow (edge=0) to nearly
                // solid (edge=1) so crisp bands read as solid color, not haze.
                const softAlpha = 0.18 + trail * 0.42
                const crispAlpha = 0.88 + trail * 0.12
                const mainAlpha = softAlpha * (1 - edge) + crispAlpha * edge
                paintBand(ctx, position, bandWidth, span, bandColor, mainAlpha, edge)

                // Halo ghost band only contributes at low edge; above that it
                // just muddies the crisp bands we worked hard to keep solid.
                if (edge < 0.4) {
                    const haloRgb = withLift(bandColor, 0.18)
                    const haloOffset = bandWidth * (0.55 + trail * 0.35)
                    paintBand(
                        ctx,
                        position - haloOffset,
                        bandWidth * 0.75,
                        span,
                        haloRgb,
                        (0.07 + trail * 0.12) * (1 - edge * 1.6),
                        edge,
                    )
                }
            }
        }

        ctx.restore()

        // Vignette scales with edge: crisp presets want flat borders, not bloom falloff.
        const vignetteAmount = (0.2 + (1 - brightness) * 0.3) * (1 - edge * 0.75)
        if (vignetteAmount > 0.02) {
            const vignette = ctx.createRadialGradient(width / 2, height / 2, s.ds(30), width / 2, height / 2, s.ds(200))
            vignette.addColorStop(0, 'rgba(0, 0, 0, 0)')
            vignette.addColorStop(1, `rgba(0, 0, 0, ${vignetteAmount})`)
            ctx.fillStyle = vignette
            ctx.fillRect(0, 0, width, height)
        }
    },
    {
        author: 'Hypercolor',
        builtinId: 'color_wave',
        category: 'ambient',
        description:
            'Traveling light bands across 13 curated palettes, from bloomed glow to razor-crisp lines via the Edge control.',
        designBasis: BUILTIN_DESIGN_BASIS,
        presets: [
            {
                controls: {
                    accentColor: '#ff6ac1',
                    brightness: 92,
                    colorMode: 'Triad Cycle',
                    density: 44,
                    direction: 'Horizontal',
                    edge: 14,
                    mode: 'Sweep',
                    palette: 'SilkCircuit',
                    primaryColor: '#e135ff',
                    saturation: 78,
                    secondaryColor: '#80ffea',
                    speed: 50,
                    trail: 54,
                    warp: 18,
                    width: 34,
                },
                description: 'The house palette in its native habitat. Bloomed sweep with warm accents layered in.',
                name: 'Silk Sweep',
            },
            {
                controls: {
                    accentColor: '#ffb300',
                    brightness: 94,
                    colorMode: 'Triad Cycle',
                    density: 62,
                    direction: 'Horizontal',
                    edge: 88,
                    mode: 'Sweep',
                    palette: 'Cyberpunk',
                    primaryColor: '#ff2975',
                    saturation: 90,
                    secondaryColor: '#00f0ff',
                    speed: 78,
                    trail: 18,
                    warp: 6,
                    width: 14,
                },
                description: 'Hot pink and cyan shards cutting across a black field. Sharp enough to read as data.',
                name: 'Neon Shards',
            },
            {
                controls: {
                    accentColor: '#e5e7eb',
                    brightness: 88,
                    colorMode: 'Custom Duo',
                    density: 78,
                    direction: 'Vertical',
                    edge: 95,
                    mode: 'Counterflow',
                    palette: 'Monochrome',
                    primaryColor: '#ffffff',
                    saturation: 20,
                    secondaryColor: '#6b7280',
                    speed: 60,
                    trail: 12,
                    warp: 4,
                    width: 8,
                },
                description: 'Thin white bars interleaved with steel. Hard-edged barcode scrolling in opposite lanes.',
                name: 'Grid Static',
            },
            {
                controls: {
                    accentColor: '#9d00ff',
                    brightness: 80,
                    colorMode: 'Triad Cycle',
                    density: 34,
                    direction: 'Diagonal',
                    edge: 8,
                    mode: 'Counterflow',
                    palette: 'Aurora',
                    primaryColor: '#00ff9f',
                    saturation: 68,
                    secondaryColor: '#00b8ff',
                    speed: 32,
                    trail: 72,
                    warp: 38,
                    width: 56,
                },
                description: 'Slow green-to-violet curtains drifting past each other. Pure ambient.',
                name: 'Aurora Drift',
            },
            {
                controls: {
                    accentColor: '#ffba08',
                    brightness: 86,
                    colorMode: 'Triad Cycle',
                    density: 30,
                    direction: 'Horizontal',
                    edge: 22,
                    mode: 'Sweep',
                    palette: 'Lava',
                    primaryColor: '#d00000',
                    saturation: 82,
                    secondaryColor: '#ff6d00',
                    speed: 22,
                    trail: 82,
                    warp: 12,
                    width: 68,
                },
                description: 'Slow molten rolls. Deep red pooling into orange, gold only at the peaks.',
                name: 'Lava Pour',
            },
            {
                controls: {
                    accentColor: '#ffe66d',
                    brightness: 90,
                    colorMode: 'Triad Cycle',
                    density: 50,
                    direction: 'Horizontal',
                    edge: 72,
                    mode: 'Counterflow',
                    palette: 'Neon 80s',
                    primaryColor: '#ff006e',
                    saturation: 86,
                    secondaryColor: '#00f5ff',
                    speed: 46,
                    trail: 34,
                    warp: 14,
                    width: 22,
                },
                description: 'Arcade marquee energy. Crisp magenta and cyan lines stacked against each other.',
                name: 'Arcade Marquee',
            },
            {
                controls: {
                    accentColor: '#caf0f8',
                    brightness: 84,
                    colorMode: 'Triad Cycle',
                    density: 54,
                    direction: 'Horizontal',
                    edge: 16,
                    mode: 'Tunnel',
                    palette: 'Ocean Deep',
                    primaryColor: '#0353a4',
                    saturation: 66,
                    secondaryColor: '#00b4d8',
                    speed: 36,
                    trail: 68,
                    warp: 28,
                    width: 36,
                },
                description: 'Concentric blue rings pulsing outward through water. The club portal, chilled.',
                name: 'Abyssal Portal',
            },
            {
                controls: {
                    accentColor: '#ffff00',
                    brightness: 95,
                    colorMode: 'Triad Cycle',
                    density: 72,
                    direction: 'Diagonal',
                    edge: 85,
                    mode: 'Sweep',
                    palette: 'CMYK',
                    primaryColor: '#00ffff',
                    saturation: 100,
                    secondaryColor: '#ff00ff',
                    speed: 70,
                    trail: 10,
                    warp: 0,
                    width: 10,
                },
                description: 'Printer-calibration energy. Pure cyan, magenta, yellow bars marching diagonally.',
                name: 'Calibration',
            },
            {
                controls: {
                    accentColor: '#cdb4db',
                    brightness: 78,
                    colorMode: 'Triad Cycle',
                    density: 36,
                    direction: 'Vertical',
                    edge: 28,
                    mode: 'Sweep',
                    palette: 'Pastel',
                    primaryColor: '#ffafcc',
                    saturation: 54,
                    secondaryColor: '#bde0fe',
                    speed: 28,
                    trail: 60,
                    warp: 20,
                    width: 48,
                },
                description: 'Cotton-candy columns drifting up. Soft, low-saturation, reads as calm not washed out.',
                name: 'Cotton Drift',
            },
            {
                controls: {
                    accentColor: '#b967ff',
                    brightness: 88,
                    colorMode: 'Triad Cycle',
                    density: 48,
                    direction: 'Horizontal',
                    edge: 48,
                    mode: 'Counterflow',
                    palette: 'Vaporwave',
                    primaryColor: '#ff71ce',
                    saturation: 80,
                    secondaryColor: '#01cdfe',
                    speed: 40,
                    trail: 48,
                    warp: 22,
                    width: 28,
                },
                description: 'Pink and cyan trading lanes with just enough edge to keep the vapor from fogging over.',
                name: 'Mirror Drift',
            },
            {
                controls: {
                    accentColor: '#ffd23f',
                    brightness: 86,
                    colorMode: 'Spectrum',
                    density: 58,
                    direction: 'Diagonal',
                    edge: 40,
                    mode: 'Sweep',
                    palette: 'Sunset',
                    primaryColor: '#ff3d7f',
                    saturation: 84,
                    secondaryColor: '#ff8c42',
                    speed: 60,
                    trail: 38,
                    warp: 26,
                    width: 22,
                },
                description: 'Full-spectrum diagonal traffic with balanced edge. Rainbow without the bloom mush.',
                name: 'Prism Drift',
            },
            {
                controls: {
                    accentColor: '#ffb199',
                    brightness: 82,
                    colorMode: 'Triad Cycle',
                    density: 42,
                    direction: 'Horizontal',
                    edge: 68,
                    mode: 'Tunnel',
                    palette: 'Blood Moon',
                    primaryColor: '#8b0000',
                    saturation: 74,
                    secondaryColor: '#ff3344',
                    speed: 34,
                    trail: 46,
                    warp: 26,
                    width: 18,
                },
                description: 'Tight crimson rings pulsing out with minimal glow. A ritual, not a rave.',
                name: 'Blood Ritual',
            },
        ],
    },
)
