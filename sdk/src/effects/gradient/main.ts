import { canvas, color, combo, num, scaleContext, toggle } from '@hypercolor/sdk'

import { BUILTIN_DESIGN_BASIS, clamp01, hexToRgb, hslCss, mixRgb, rgbToCss, scaleRgb } from '../_builtin/common'

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

// ── Oklab interpolation for the Smooth blend mode ───────────────────────────
// Ported from sdk/packages/core/src/palette/runtime.ts (the converters there
// are private, and the public palette API only samples named palettes).

function srgbChannelToLinear(channel: number): number {
    const v = channel / 255
    return v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4
}

function linearChannelToSrgb(value: number): number {
    const v = value <= 0.0031308 ? 12.92 * value : 1.055 * value ** (1 / 2.4) - 0.055
    return Math.round(clamp01(v) * 255)
}

function rgbToOklab(rgb: { r: number; g: number; b: number }): [number, number, number] {
    const lr = srgbChannelToLinear(rgb.r)
    const lg = srgbChannelToLinear(rgb.g)
    const lb = srgbChannelToLinear(rgb.b)

    const l = Math.cbrt(0.4122214708 * lr + 0.5363325363 * lg + 0.0514459929 * lb)
    const m = Math.cbrt(0.2119034982 * lr + 0.6806995451 * lg + 0.1073969566 * lb)
    const s = Math.cbrt(0.0883024619 * lr + 0.2817188376 * lg + 0.6299787005 * lb)

    return [
        0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s,
        1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s,
        0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s,
    ]
}

function oklabToCss(lightnessL: number, a: number, b: number): string {
    const l_ = lightnessL + 0.3963377774 * a + 0.2158037573 * b
    const m_ = lightnessL - 0.1055613458 * a - 0.0638541728 * b
    const s_ = lightnessL - 0.0894841775 * a - 1.291485548 * b

    const l = l_ * l_ * l_
    const m = m_ * m_ * m_
    const s = s_ * s_ * s_

    return rgbToCss({
        b: linearChannelToSrgb(-0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s),
        g: linearChannelToSrgb(-1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s),
        r: linearChannelToSrgb(4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s),
    })
}

const SMOOTH_SEGMENTS = 8

/**
 * Append Oklab-interpolated color stops for one gradient segment, including
 * the starting stop but excluding the final one (the caller closes the ramp).
 * The Saturation control scales Oklab chroma (a/b) so it stays live in Smooth.
 */
function pushOklabStops(
    stops: [number, string][],
    from: { r: number; g: number; b: number },
    to: { r: number; g: number; b: number },
    fromPos: number,
    toPos: number,
    chroma: number,
): void {
    const a = rgbToOklab(from)
    const b = rgbToOklab(to)
    for (let i = 0; i < SMOOTH_SEGMENTS; i++) {
        const t = i / SMOOTH_SEGMENTS
        stops.push([
            fromPos + (toPos - fromPos) * t,
            oklabToCss(
                a[0] + (b[0] - a[0]) * t,
                (a[1] + (b[1] - a[1]) * t) * chroma,
                (a[2] + (b[2] - a[2]) * t) * chroma,
            ),
        ])
    }
}

// Frame-to-frame gradient cache — with Clamp mode and zero scroll speed the
// stops and geometry never change, so the CanvasGradient is built exactly once.
let cachedGradient: CanvasGradient | null = null
let cachedGradientKey = ''
let cachedShadow: CanvasGradient | null = null
let cachedShadowKey = ''

export default canvas(
    'Gradient',
    {
        color_start: color('Color A', '#e135ff', { group: 'Colors' }),
        use_mid_color: toggle('Use Middle Color', true, { group: 'Colors' }),
        color_mid: color('Color B', '#1adfc9', { group: 'Colors' }),
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
        // normalize:'none' — the magic `speed` normalization assumes a 1-10 domain;
        // on this [-1, 1] range it NaNs negatives and clamps positives to 0.2.
        speed: num('Scroll Speed', [-1, 1], 0.18, { group: 'Motion', normalize: 'none' }),
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
        const speed = controls.speed as number
        const interpolation = controls.interpolation as string
        const animatedOffset = repeatOffset((controls.offset as number) + time * speed, repeatMode)

        ctx.clearRect(0, 0, width, height)

        const easedMid = ease(midpoint, easing)
        const isRadial = (controls.mode as string) === 'Radial'

        // Assemble the color-stop list for the active blend mode.
        const stops: [number, string][] = []
        if (interpolation === 'Smooth') {
            // Perceptual ramp — Oklab-interpolated stops between the user colors.
            if (useMid) {
                pushOklabStops(stops, start, mid, 0, easedMid, saturation)
                pushOklabStops(stops, mid, end, easedMid, 1, saturation)
            } else {
                pushOklabStops(stops, start, end, 0, 1, saturation)
            }
            const endLab = rgbToOklab(end)
            stops.push([1, oklabToCss(endLab[0], endLab[1] * saturation, endLab[2] * saturation)])
        } else {
            const direct = interpolation === 'Direct'
            const startCss = direct ? rgbToCss(start) : saturateRgb(start, saturation)
            const midCss = direct ? rgbToCss(mid) : saturateRgb(mid, saturation)
            const endCss = direct ? rgbToCss(end) : saturateRgb(end, saturation)
            stops.push([0, startCss])
            if (useMid && isRadial) {
                stops.push([easedMid, midCss])
            } else if (useMid) {
                const preMix = mixRgb(start, mid, 0.65)
                const postMix = mixRgb(mid, end, 0.55)
                stops.push([clamp01(easedMid - 0.06), direct ? rgbToCss(preMix) : saturateRgb(preMix, saturation)])
                stops.push([easedMid, midCss])
                stops.push([clamp01(easedMid + 0.06), direct ? rgbToCss(postMix) : saturateRgb(postMix, saturation)])
            }
            stops.push([1, endCss])
        }

        let key: string
        let create: () => CanvasGradient
        if (isRadial) {
            const cx = width * clamp01((controls.center_x as number) + animatedOffset * 0.18)
            const cy = height * clamp01((controls.center_y as number) + Math.sin(time * 0.33) * speed * 0.08)
            const radius = (Math.max(width, height) * 0.62) / Math.max(scale, 0.1)
            key = `radial|${cx}|${cy}|${radius}`
            create = () => ctx.createRadialGradient(cx, cy, width * 0.02, cx, cy, radius)
        } else {
            const angle = ((controls.angle as number) * Math.PI) / 180
            const axisOffsetX = Math.cos(angle) * animatedOffset * width * 0.45
            const axisOffsetY = Math.sin(angle) * animatedOffset * height * 0.45
            const dx = (Math.cos(angle) * width * 0.62) / Math.max(scale, 0.1)
            const dy = (Math.sin(angle) * height * 0.62) / Math.max(scale, 0.1)
            const cx = width * (controls.center_x as number) + axisOffsetX
            const cy = height * (controls.center_y as number) + axisOffsetY
            key = `linear|${cx - dx}|${cy - dy}|${cx + dx}|${cy + dy}`
            create = () => ctx.createLinearGradient(cx - dx, cy - dy, cx + dx, cy + dy)
        }
        for (const [pos, css] of stops) key += `|${pos}:${css}`

        if (!cachedGradient || key !== cachedGradientKey) {
            const gradient = create()
            for (const [pos, css] of stops) gradient.addColorStop(pos, css)
            cachedGradient = gradient
            cachedGradientKey = key
        }
        ctx.fillStyle = cachedGradient
        ctx.fillRect(0, 0, width, height)

        if (brightness < 1) {
            const shadowKey = `${width}|${height}|${brightness}`
            if (!cachedShadow || shadowKey !== cachedShadowKey) {
                const shadow = ctx.createLinearGradient(0, 0, width, height)
                shadow.addColorStop(0, `rgba(0, 0, 0, ${(1 - brightness) * 0.1})`)
                shadow.addColorStop(0.5, 'rgba(0, 0, 0, 0)')
                shadow.addColorStop(1, `rgba(255, 255, 255, ${(1 - brightness) * 0.04})`)
                cachedShadow = shadow
                cachedShadowKey = shadowKey
            }
            ctx.fillStyle = cachedShadow
            ctx.fillRect(0, 0, width, height)
        }
    },
    {
        author: 'Hypercolor',
        builtinId: 'gradient',
        category: 'ambient',
        description:
            'Adaptive linear and radial gradients with animated drift, shaped mid-stops, and presets tuned for LED hardware.',
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
