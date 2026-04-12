import { canvas, color, combo, num, scaleContext } from '@hypercolor/sdk'

import {
    BUILTIN_DESIGN_BASIS,
    clamp01,
    hexToRgb,
    mixRgb,
    rgbToCss,
    scaleRgb,
} from '../_builtin/common'

export default canvas(
    'Solid Color',
    {
        pattern: combo('Pattern', ['Solid', 'Vertical Split', 'Horizontal Split', 'Checker', 'Quadrants', 'Halo'], {
            default: 'Solid',
            group: 'Pattern',
        }),
        color: color('Primary Color', '#ffffff', { group: 'Colors' }),
        secondaryColor: color('Secondary Color', '#10121a', { group: 'Colors' }),
        accentColor: color('Accent Color', '#80ffea', { group: 'Colors' }),
        position: num('Split Position', [0, 100], 50, { group: 'Pattern' }),
        softness: num('Blend Softness', [0, 100], 0, { group: 'Pattern' }),
        scale: num('Pattern Scale', [2, 16], 6, { group: 'Pattern' }),
        vignette: num('Vignette', [0, 100], 0, { group: 'Output' }),
        brightness: num('Brightness', [0, 100], 100, { group: 'Output' }),
    },
    (ctx, time, controls) => {
        const s = scaleContext(ctx.canvas, BUILTIN_DESIGN_BASIS)
        const width = s.width
        const height = s.height
        const primary = scaleRgb(hexToRgb(controls.color as string), (controls.brightness as number) / 100)
        const secondary = scaleRgb(hexToRgb(controls.secondaryColor as string), (controls.brightness as number) / 100)
        const accent = scaleRgb(hexToRgb(controls.accentColor as string), (controls.brightness as number) / 100)
        const pattern = controls.pattern as string
        const split = (controls.position as number) / 100
        const softness = (controls.softness as number) / 100
        const cells = Math.max(2, Math.round(controls.scale as number))

        ctx.clearRect(0, 0, width, height)

        if (pattern === 'Solid') {
            ctx.fillStyle = rgbToCss(primary)
            ctx.fillRect(0, 0, width, height)
        } else if (pattern === 'Vertical Split' || pattern === 'Horizontal Split') {
            const gradient =
                pattern === 'Vertical Split'
                    ? ctx.createLinearGradient(0, 0, width, 0)
                    : ctx.createLinearGradient(0, 0, 0, height)
            const edge = split
            const feather = softness * 0.2
            gradient.addColorStop(Math.max(0, edge - feather), rgbToCss(primary))
            gradient.addColorStop(Math.min(1, edge + feather), rgbToCss(secondary))
            ctx.fillStyle = gradient
            ctx.fillRect(0, 0, width, height)
        } else if (pattern === 'Checker') {
            const cellWidth = width / cells
            const cellHeight = height / Math.max(2, Math.round(cells * (height / Math.max(width, 1))))
            for (let y = 0; y < height; y += cellHeight) {
                for (let x = 0; x < width; x += cellWidth) {
                    const usePrimary = (Math.floor(x / cellWidth) + Math.floor(y / cellHeight)) % 2 === 0
                    ctx.fillStyle = rgbToCss(usePrimary ? primary : secondary)
                    ctx.fillRect(x, y, cellWidth + 1, cellHeight + 1)
                }
            }
        } else if (pattern === 'Quadrants') {
            const splitX = width * split
            const splitY = height * split
            const topRight = mixRgb(primary, accent, 0.5)
            const bottomLeft = mixRgb(secondary, accent, 0.25)
            ctx.fillStyle = rgbToCss(primary)
            ctx.fillRect(0, 0, splitX, splitY)
            ctx.fillStyle = rgbToCss(topRight)
            ctx.fillRect(splitX, 0, width - splitX, splitY)
            ctx.fillStyle = rgbToCss(bottomLeft)
            ctx.fillRect(0, splitY, splitX, height - splitY)
            ctx.fillStyle = rgbToCss(secondary)
            ctx.fillRect(splitX, splitY, width - splitX, height - splitY)
        } else {
            ctx.fillStyle = rgbToCss(primary)
            ctx.fillRect(0, 0, width, height)
            const cx = s.dx(160 + Math.sin(time * 0.42) * 18)
            const cy = s.dy(100 + Math.cos(time * 0.33) * 12)
            const halo = ctx.createRadialGradient(cx, cy, s.ds(8), cx, cy, s.ds(112))
            halo.addColorStop(0, rgbToCss(accent, 0.95))
            halo.addColorStop(0.38, rgbToCss(mixRgb(accent, primary, 0.45), 0.45))
            halo.addColorStop(1, rgbToCss(primary, 0))
            ctx.fillStyle = halo
            ctx.fillRect(0, 0, width, height)
        }

        const vignette = (controls.vignette as number) / 100
        if (vignette > 0) {
            const overlay = ctx.createRadialGradient(width / 2, height / 2, width * 0.15, width / 2, height / 2, width * 0.78)
            overlay.addColorStop(0, 'rgba(0, 0, 0, 0)')
            overlay.addColorStop(1, `rgba(0, 0, 0, ${clamp01(vignette * 0.8)})`)
            ctx.fillStyle = overlay
            ctx.fillRect(0, 0, width, height)
        }
    },
    {
        author: 'Hypercolor',
        builtinId: 'solid_color',
        category: 'ambient',
        description: 'Solid fills and clean diagnostic splits with richer accent lighting, vignette shaping, and scene-ready presets.',
        designBasis: BUILTIN_DESIGN_BASIS,
        presets: [
            {
                controls: { brightness: 100, color: '#f6f7fb', pattern: 'Solid', vignette: 0 },
                description: 'Clean neutral fill for hardware checks and understated ambient light.',
                name: 'Studio White',
            },
            {
                controls: {
                    accentColor: '#80ffea',
                    brightness: 92,
                    color: '#09111e',
                    pattern: 'Vertical Split',
                    position: 52,
                    secondaryColor: '#e135ff',
                    softness: 8,
                    vignette: 18,
                },
                description: 'A sharp cyan-to-magenta divider that makes zone placement instantly obvious.',
                name: 'Silk Divide',
            },
            {
                controls: {
                    accentColor: '#ffffff',
                    brightness: 88,
                    color: '#130b1f',
                    pattern: 'Checker',
                    scale: 8,
                    secondaryColor: '#ff6ac1',
                    vignette: 10,
                },
                description: 'Checkerboard contrast with enough drama to spot sampling mistakes at a glance.',
                name: 'Rose Checker',
            },
            {
                controls: {
                    accentColor: '#f1fa8c',
                    brightness: 80,
                    color: '#05070c',
                    pattern: 'Halo',
                    secondaryColor: '#05070c',
                    vignette: 24,
                },
                description: 'A dark field with a soft central beacon for quickly judging centering and falloff.',
                name: 'Operator Halo',
            },
        ],
    },
)

