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
    (ctx, _time, controls) => {
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
            ctx.fillStyle = rgbToCss(primary)
            ctx.fillRect(0, 0, splitX, splitY)
            ctx.fillStyle = rgbToCss(secondary)
            ctx.fillRect(splitX, 0, width - splitX, splitY)
            ctx.fillStyle = rgbToCss(secondary)
            ctx.fillRect(0, splitY, splitX, height - splitY)
            ctx.fillStyle = rgbToCss(primary)
            ctx.fillRect(splitX, splitY, width - splitX, height - splitY)
        } else {
            ctx.fillStyle = rgbToCss(secondary)
            ctx.fillRect(0, 0, width, height)
            const cx = width * split
            const cy = height * 0.5
            const glowRadius = s.ds(52 + (controls.scale as number) * 8)
            const body = ctx.createRadialGradient(cx, cy, s.ds(8), cx, cy, glowRadius)
            body.addColorStop(0, rgbToCss(primary, 0.92))
            body.addColorStop(0.45, rgbToCss(mixRgb(primary, accent, 0.2), 0.4))
            body.addColorStop(1, rgbToCss(primary, 0))
            ctx.fillStyle = body
            ctx.fillRect(0, 0, width, height)

            const core = ctx.createRadialGradient(cx, cy, s.ds(4), cx, cy, glowRadius * 0.45)
            core.addColorStop(0, rgbToCss(accent, 0.82))
            core.addColorStop(0.32, rgbToCss(mixRgb(accent, primary, 0.35), 0.3))
            core.addColorStop(1, rgbToCss(accent, 0))
            ctx.fillStyle = core
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
        description: 'Clean fills, simple diagnostic patterns, and a restrained beacon mode for practical room lighting and layout checks.',
        designBasis: BUILTIN_DESIGN_BASIS,
        presets: [
            {
                controls: { brightness: 100, color: '#f5f6fb', pattern: 'Solid', vignette: 0 },
                description: 'Neutral full white for confirming output, dead LEDs, and overall brightness.',
                name: 'Neutral White',
            },
            {
                controls: {
                    brightness: 52,
                    color: '#fff2d6',
                    pattern: 'Solid',
                    vignette: 0,
                },
                description: 'Warm monitor bias light that fills the room without looking clinical or harsh.',
                name: 'Warm Bias',
            },
            {
                controls: {
                    brightness: 16,
                    color: '#ff8f1f',
                    pattern: 'Solid',
                    vignette: 0,
                },
                description: 'Low-light amber for night use when you want visibility without blowing up your pupils.',
                name: 'Night Amber',
            },
            {
                controls: {
                    brightness: 14,
                    color: '#b81818',
                    pattern: 'Solid',
                    vignette: 0,
                },
                description: 'A dim deep-red fill that keeps the room readable without feeling awake.',
                name: 'Darkroom Red',
            },
            {
                controls: {
                    brightness: 22,
                    color: '#4ec7ff',
                    pattern: 'Solid',
                    vignette: 0,
                },
                description: 'Cool low-blue ambience for desks and underglow when you want something calm but present.',
                name: 'Focus Blue',
            },
            {
                controls: {
                    brightness: 100,
                    color: '#ffffff',
                    pattern: 'Vertical Split',
                    position: 50,
                    secondaryColor: '#000000',
                    softness: 0,
                    vignette: 0,
                },
                description: 'Hard left-right split for checking zone placement, mirrored mapping, and strip order.',
                name: 'A/B Split',
            },
            {
                controls: {
                    brightness: 100,
                    color: '#ffffff',
                    pattern: 'Checker',
                    scale: 8,
                    secondaryColor: '#000000',
                    vignette: 0,
                },
                description: 'Black-and-white checkerboard for quickly spotting sampling errors and uneven footprints.',
                name: 'Checkerboard',
            },
            {
                controls: {
                    accentColor: '#fff4cf',
                    brightness: 42,
                    color: '#fff0da',
                    pattern: 'Halo',
                    position: 50,
                    scale: 6,
                    secondaryColor: '#090b10',
                    vignette: 12,
                },
                description: 'A restrained center beacon for judging centering and falloff without turning the whole rig into a gimmick.',
                name: 'Center Beacon',
            },
        ],
    },
)
