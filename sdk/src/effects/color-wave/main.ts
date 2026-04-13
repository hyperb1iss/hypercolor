import { canvas, color, combo, num, scaleContext } from '@hypercolor/sdk'

import {
    BUILTIN_DESIGN_BASIS,
    hexToRgb,
    hslCss,
    mixRgb,
    rgbToCss,
    scaleRgb,
    withLift,
} from '../_builtin/common'

function fract(value: number): number {
    return value - Math.floor(value)
}

function bandGradient(
    ctx: CanvasRenderingContext2D,
    position: number,
    width: number,
    span: number,
    color: string,
    alpha: number,
): void {
    const gradient = ctx.createLinearGradient(position - width, 0, position + width, 0)
    gradient.addColorStop(0, 'rgba(255, 255, 255, 0)')
    gradient.addColorStop(0.18, color.replace(/[\d.]+\)$/, `${alpha * 0.2})`))
    gradient.addColorStop(0.5, color.replace(/[\d.]+\)$/, `${alpha})`))
    gradient.addColorStop(0.82, color.replace(/[\d.]+\)$/, `${alpha * 0.22})`))
    gradient.addColorStop(1, 'rgba(255, 255, 255, 0)')
    ctx.fillStyle = gradient
    ctx.fillRect(position - width, -span, width * 2, span * 2)
}

function directionAngle(direction: string): number {
    if (direction === 'Vertical') return Math.PI * 0.5
    if (direction === 'Diagonal') return Math.PI * 0.32
    return 0
}

function waveColor(
    mode: string,
    phase: number,
    primary: ReturnType<typeof hexToRgb>,
    secondary: ReturnType<typeof hexToRgb>,
    accent: ReturnType<typeof hexToRgb>,
    brightness: number,
    saturation: number,
): string {
    if (mode === 'Spectrum') {
        return hslCss(phase * 360, 62 + saturation * 30, 46 + brightness * 24, 1)
    }

    if (mode === 'Triad Cycle') {
        const t = fract(phase)
        if (t < 1 / 3) return rgbToCss(scaleRgb(mixRgb(primary, secondary, t * 3), brightness))
        if (t < 2 / 3) return rgbToCss(scaleRgb(mixRgb(secondary, accent, (t - 1 / 3) * 3), brightness))
        return rgbToCss(scaleRgb(mixRgb(accent, primary, (t - 2 / 3) * 3), brightness))
    }

    return rgbToCss(scaleRgb(mixRgb(primary, secondary, 0.5 + Math.sin(phase * Math.PI * 2) * 0.5), brightness))
}

export default canvas(
    'Color Wave',
    {
        mode: combo('Mode', ['Sweep', 'Counterflow', 'Tunnel'], { default: 'Sweep', group: 'Scene' }),
        direction: combo('Direction', ['Horizontal', 'Vertical', 'Diagonal'], {
            default: 'Horizontal',
            group: 'Motion',
        }),
        colorMode: combo('Color Mode', ['Custom Duo', 'Triad Cycle', 'Spectrum'], {
            default: 'Triad Cycle',
            group: 'Colors',
        }),
        primaryColor: color('Primary Color', '#80ffea', { group: 'Colors' }),
        secondaryColor: color('Secondary Color', '#e135ff', { group: 'Colors' }),
        accentColor: color('Accent Color', '#ff9f45', { group: 'Colors' }),
        speed: num('Speed', [0, 100], 54, { group: 'Motion' }),
        density: num('Density', [0, 100], 42, { group: 'Motion' }),
        width: num('Band Width', [4, 100], 34, { group: 'Motion' }),
        trail: num('Trail', [0, 100], 46, { group: 'Motion' }),
        warp: num('Warp', [0, 100], 20, { group: 'Motion' }),
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
        const speed = (controls.speed as number) / 100
        const density = (controls.density as number) / 100
        const bandWidth = s.ds(12 + (controls.width as number) * 1.4)
        const trail = (controls.trail as number) / 100
        const warp = (controls.warp as number) / 100
        const brightness = (controls.brightness as number) / 100
        const saturation = (controls.saturation as number) / 100
        const primary = hexToRgb(controls.primaryColor as string)
        const secondary = hexToRgb(controls.secondaryColor as string)
        const accent = hexToRgb(controls.accentColor as string)

        const base = scaleRgb(mixRgb(primary, secondary, 0.18), brightness * 0.12)
        const fill = ctx.createLinearGradient(0, 0, width, height)
        fill.addColorStop(0, rgbToCss(base))
        fill.addColorStop(1, rgbToCss(scaleRgb(mixRgb(base, accent, 0.25), 0.9)))
        ctx.fillStyle = fill
        ctx.fillRect(0, 0, width, height)

        ctx.save()
        ctx.globalCompositeOperation = 'lighter'

        if (mode === 'Tunnel') {
            const ringCount = Math.round(5 + density * 8)
            const centerX = s.dx(160 + Math.sin(time * (0.24 + warp * 0.7)) * 20)
            const centerY = s.dy(100 + Math.cos(time * (0.19 + warp * 0.6)) * 14)
            const maxRadius = s.ds(160)

            for (let i = 0; i < ringCount; i++) {
                const phase = fract(time * (0.08 + speed * 0.42) + i / ringCount)
                const radius = s.ds(10) + phase * maxRadius
                const alpha = (1 - phase) * (0.16 + trail * 0.32)
                const color = waveColor(colorMode, phase + i * 0.08, primary, secondary, accent, brightness, saturation)

                ctx.save()
                ctx.lineWidth = bandWidth * (0.28 + (1 - phase) * 0.18)
                ctx.shadowBlur = bandWidth * (0.55 + trail * 0.45)
                ctx.shadowColor = color
                ctx.strokeStyle = color
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
                const color = waveColor(colorMode, basePhase + i * 0.1, primary, secondary, accent, brightness, saturation)
                const haloColor = rgbToCss(withLift(hexToRgb(color.startsWith('#') ? color : controls.accentColor as string), 0.18))

                bandGradient(ctx, position, bandWidth, span, color, 0.18 + trail * 0.42)
                bandGradient(ctx, position - bandWidth * (0.55 + trail * 0.35), bandWidth * 0.75, span, haloColor, 0.07 + trail * 0.12)
            }
        }

        ctx.restore()

        const vignette = ctx.createRadialGradient(width / 2, height / 2, s.ds(30), width / 2, height / 2, s.ds(200))
        vignette.addColorStop(0, 'rgba(0, 0, 0, 0)')
        vignette.addColorStop(1, `rgba(0, 0, 0, ${0.2 + (1 - brightness) * 0.3})`)
        ctx.fillStyle = vignette
        ctx.fillRect(0, 0, width, height)
    },
    {
        author: 'Hypercolor',
        builtinId: 'color_wave',
        category: 'ambient',
        description:
            'Traveling light bands with sweep, counterflow, and tunnel modes across curated palettes.',
        designBasis: BUILTIN_DESIGN_BASIS,
        presets: [
            {
                controls: {
                    accentColor: '#ff9f45',
                    brightness: 90,
                    colorMode: 'Triad Cycle',
                    density: 44,
                    direction: 'Horizontal',
                    mode: 'Sweep',
                    primaryColor: '#80ffea',
                    saturation: 78,
                    secondaryColor: '#e135ff',
                    speed: 56,
                    trail: 48,
                    warp: 18,
                    width: 32,
                },
                description: 'A glossy sweep with warm accents layered into the motion.',
                name: 'Silk Sweep',
            },
            {
                controls: {
                    accentColor: '#f8f1ff',
                    brightness: 74,
                    colorMode: 'Custom Duo',
                    density: 38,
                    direction: 'Vertical',
                    mode: 'Counterflow',
                    primaryColor: '#6d79ff',
                    saturation: 62,
                    secondaryColor: '#ff6ac1',
                    speed: 42,
                    trail: 64,
                    warp: 32,
                    width: 44,
                },
                description: 'Two opposing columns sliding past each other like a nightclub escalator made of light.',
                name: 'Mirror Drift',
            },
            {
                controls: {
                    accentColor: '#ffd86b',
                    brightness: 82,
                    colorMode: 'Spectrum',
                    density: 56,
                    direction: 'Diagonal',
                    mode: 'Sweep',
                    primaryColor: '#ffffff',
                    saturation: 84,
                    secondaryColor: '#80ffea',
                    speed: 68,
                    trail: 40,
                    warp: 26,
                    width: 26,
                },
                description: 'Sharper diagonal rainbow traffic with enough darkness around it to keep the spectrum vivid.',
                name: 'Prism Drift',
            },
            {
                controls: {
                    accentColor: '#80ffea',
                    brightness: 78,
                    colorMode: 'Triad Cycle',
                    density: 52,
                    direction: 'Horizontal',
                    mode: 'Tunnel',
                    primaryColor: '#ff6ac1',
                    saturation: 70,
                    secondaryColor: '#6c7dff',
                    speed: 48,
                    trail: 58,
                    warp: 44,
                    width: 38,
                },
                description: 'Concentric club rings that feel like a portal opening in the center of the desk.',
                name: 'Club Portal',
            },
        ],
    },
)
