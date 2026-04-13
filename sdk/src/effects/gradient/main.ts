import { canvas, color, combo, num, scaleContext, toggle } from '@hypercolor/sdk'

import {
    BUILTIN_DESIGN_BASIS,
    clamp01,
    hexToRgb,
    hslCss,
    mixRgb,
    rgbToCss,
    scaleRgb,
} from '../_builtin/common'

function repeatOffset(value: number, mode: string): number {
    if (mode === 'Repeat') return ((((value + 1) % 2) + 2) % 2) - 1
    if (mode === 'Mirror') {
        const wrapped = (((value + 1) % 4) + 4) % 4
        return (wrapped <= 2 ? wrapped : 4 - wrapped) - 1
    }
    return Math.max(-1, Math.min(1, value))
}

function ease(value: number, mode: string): number {
    const t = clamp01(value)
    if (mode === 'Ease In') return t * t
    if (mode === 'Ease Out') return 1 - (1 - t) * (1 - t)
    if (mode === 'Smooth') return t * t * (3 - 2 * t)
    return t
}

function rgbToHsl(rgb: { r: number; g: number; b: number }): [number, number, number] {
    const r = rgb.r / 255
    const g = rgb.g / 255
    const b = rgb.b / 255
    const max = Math.max(r, g, b)
    const min = Math.min(r, g, b)
    const delta = max - min
    const lightness = (max + min) / 2

    if (delta === 0) return [0, 0, lightness]

    const saturation = lightness > 0.5 ? delta / (2 - max - min) : delta / (max + min)
    let hue = 0
    if (max === r) hue = (g - b) / delta + (g < b ? 6 : 0)
    else if (max === g) hue = (b - r) / delta + 2
    else hue = (r - g) / delta + 4

    return [hue * 60, saturation, lightness]
}

function saturateRgb(rgb: { r: number; g: number; b: number }, saturation: number): string {
    const [hue, sat, lightness] = rgbToHsl(rgb)
    return hslCss(hue, clamp01(sat * saturation) * 100, lightness * 100)
}

export default canvas(
    'Gradient',
    {
        color_start: color('Color A', '#e135ff', { group: 'Colors' }),
        use_mid_color: toggle('Use Middle Color', true, { group: 'Colors' }),
        color_mid: color('Color B', '#80ffea', { group: 'Colors' }),
        midpoint: num('Middle Position', [0.05, 0.95], 0.5, { group: 'Colors' }),
        color_end: color('Color C', '#0b1020', { group: 'Colors' }),
        interpolation: combo('Color Blend', ['Vivid', 'Smooth', 'Direct'], { default: 'Vivid', group: 'Colors' }),
        saturation: num('Saturation', [0.5, 1.5], 1, { group: 'Colors' }),
        mode: combo('Gradient Type', ['Linear', 'Radial'], { default: 'Linear', group: 'Shape' }),
        angle: num('Angle', [0, 360], 25, { group: 'Shape' }),
        center_x: num('Center X', [0, 1], 0.5, { group: 'Shape' }),
        center_y: num('Center Y', [0, 1], 0.5, { group: 'Shape' }),
        scale: num('Scale', [0.1, 2.5], 1, { group: 'Shape' }),
        easing: combo('Distribution', ['Linear', 'Ease In', 'Ease Out', 'Smooth'], {
            default: 'Linear',
            group: 'Shape',
        }),
        repeat_mode: combo('Repeat', ['Clamp', 'Repeat', 'Mirror'], { default: 'Clamp', group: 'Motion' }),
        offset: num('Offset', [-1, 1], 0, { group: 'Motion' }),
        speed: num('Scroll Speed', [-1, 1], 0.18, { group: 'Motion' }),
        brightness: num('Brightness', [0, 1], 1, { group: 'Output' }),
    },
    (ctx, time, controls) => {
        const s = scaleContext(ctx.canvas, BUILTIN_DESIGN_BASIS)
        const width = s.width
        const height = s.height
        const brightness = controls.brightness as number
        const start = scaleRgb(hexToRgb(controls.color_start as string), brightness)
        const mid = scaleRgb(hexToRgb(controls.color_mid as string), brightness)
        const end = scaleRgb(hexToRgb(controls.color_end as string), brightness)
        const midpoint = controls.midpoint as number
        const useMid = controls.use_mid_color as boolean
        const saturation = controls.saturation as number
        const scale = controls.scale as number
        const easing = controls.easing as string
        const repeatMode = controls.repeat_mode as string
        const animatedOffset = repeatOffset((controls.offset as number) + time * (controls.speed as number), repeatMode)

        ctx.clearRect(0, 0, width, height)

        const startCss = controls.interpolation === 'Direct' ? rgbToCss(start) : saturateRgb(start, saturation)
        const midCss = controls.interpolation === 'Direct' ? rgbToCss(mid) : saturateRgb(mid, saturation)
        const endCss = controls.interpolation === 'Direct' ? rgbToCss(end) : saturateRgb(end, saturation)

        if ((controls.mode as string) === 'Radial') {
            const cx = width * clamp01((controls.center_x as number) + animatedOffset * 0.18)
            const cy = height * clamp01((controls.center_y as number) + Math.sin(time * 0.33) * (controls.speed as number) * 0.08)
            const radius = (Math.max(width, height) * 0.62) / Math.max(scale, 0.1)
            const gradient = ctx.createRadialGradient(cx, cy, width * 0.02, cx, cy, radius)
            gradient.addColorStop(0, startCss)
            if (useMid) {
                gradient.addColorStop(ease(midpoint, easing), midCss)
            }
            gradient.addColorStop(1, endCss)
            ctx.fillStyle = gradient
            ctx.fillRect(0, 0, width, height)
        } else {
            const angle = ((controls.angle as number) * Math.PI) / 180
            const axisOffsetX = Math.cos(angle) * animatedOffset * width * 0.45
            const axisOffsetY = Math.sin(angle) * animatedOffset * height * 0.45
            const dx = (Math.cos(angle) * width * 0.62) / Math.max(scale, 0.1)
            const dy = (Math.sin(angle) * height * 0.62) / Math.max(scale, 0.1)
            const cx = width * (controls.center_x as number) + axisOffsetX
            const cy = height * (controls.center_y as number) + axisOffsetY
            const gradient = ctx.createLinearGradient(cx - dx, cy - dy, cx + dx, cy + dy)
            gradient.addColorStop(0, startCss)
            if (useMid) {
                const easedMid = ease(midpoint, easing)
                gradient.addColorStop(clamp01(easedMid - 0.06), controls.interpolation === 'Direct' ? rgbToCss(mixRgb(start, mid, 0.65)) : saturateRgb(mixRgb(start, mid, 0.65), saturation))
                gradient.addColorStop(easedMid, midCss)
                gradient.addColorStop(clamp01(easedMid + 0.06), controls.interpolation === 'Direct' ? rgbToCss(mixRgb(mid, end, 0.55)) : saturateRgb(mixRgb(mid, end, 0.55), saturation))
            }
            gradient.addColorStop(1, endCss)
            ctx.fillStyle = gradient
            ctx.fillRect(0, 0, width, height)
        }

        if (brightness < 1) {
            const shadow = ctx.createLinearGradient(0, 0, width, height)
            shadow.addColorStop(0, `rgba(0, 0, 0, ${(1 - brightness) * 0.1})`)
            shadow.addColorStop(0.5, 'rgba(0, 0, 0, 0)')
            shadow.addColorStop(1, `rgba(255, 255, 255, ${(1 - brightness) * 0.04})`)
            ctx.fillStyle = shadow
            ctx.fillRect(0, 0, width, height)
        }
    },
    {
        author: 'Hypercolor',
        builtinId: 'gradient',
        category: 'ambient',
        description: 'Adaptive linear and radial gradients with animated drift, shaped mid-stops, and presets tuned for LED hardware.',
        designBasis: BUILTIN_DESIGN_BASIS,
        presets: [
            {
                controls: {
                    angle: 24,
                    brightness: 0.92,
                    color_end: '#090d18',
                    color_mid: '#80ffea',
                    color_start: '#e135ff',
                    midpoint: 0.44,
                    mode: 'Linear',
                    speed: 0.22,
                    use_mid_color: true,
                },
                description: 'Candy glass washed diagonally across the whole rig.',
                name: 'Silk Glass',
            },
            {
                controls: {
                    brightness: 0.84,
                    color_end: '#020812',
                    color_mid: '#5cf2ff',
                    color_start: '#f6fbff',
                    midpoint: 0.38,
                    mode: 'Radial',
                    scale: 0.88,
                    speed: 0.12,
                    use_mid_color: true,
                },
                description: 'A bright glacial core diffusing outward into deep polar blue.',
                name: 'Ice Core',
            },
            {
                controls: {
                    angle: 112,
                    brightness: 0.78,
                    color_end: '#14040a',
                    color_mid: '#ff7a1a',
                    color_start: '#ffd86a',
                    interpolation: 'Smooth',
                    midpoint: 0.56,
                    mode: 'Linear',
                    saturation: 1.08,
                    use_mid_color: true,
                },
                description: 'Gold into ember with enough darkness to keep the hot zones vivid instead of chalky.',
                name: 'Solar Veil',
            },
            {
                controls: {
                    brightness: 0.7,
                    color_end: '#03030a',
                    color_mid: '#6c7dff',
                    color_start: '#ff6ac1',
                    midpoint: 0.5,
                    mode: 'Radial',
                    saturation: 1.12,
                    scale: 0.76,
                    speed: 0.28,
                    use_mid_color: true,
                },
                description: 'A nightclub orb with soft magenta edges and icy indigo bloom.',
                name: 'Club Nebula',
            },
        ],
    },
)
