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
    'Gradient',
    {
        mode: combo('Gradient Mode', ['Linear', 'Radial'], { default: 'Linear', group: 'Gradient' }),
        colorStart: color('Start Color', '#e135ff', { group: 'Colors' }),
        colorMid: color('Mid Color', '#80ffea', { group: 'Colors' }),
        colorEnd: color('End Color', '#0b1020', { group: 'Colors' }),
        angle: num('Angle', [0, 360], 25, { group: 'Gradient' }),
        drift: num('Drift', [0, 100], 28, { group: 'Motion' }),
        midpoint: num('Midpoint', [10, 90], 50, { group: 'Gradient' }),
        softness: num('Softness', [0, 100], 50, { group: 'Gradient' }),
        contrast: num('Contrast', [0, 100], 18, { group: 'Output' }),
        brightness: num('Brightness', [0, 100], 90, { group: 'Output' }),
    },
    (ctx, time, controls) => {
        const s = scaleContext(ctx.canvas, BUILTIN_DESIGN_BASIS)
        const width = s.width
        const height = s.height
        const brightness = (controls.brightness as number) / 100
        const start = scaleRgb(hexToRgb(controls.colorStart as string), brightness)
        const mid = scaleRgb(hexToRgb(controls.colorMid as string), brightness)
        const end = scaleRgb(hexToRgb(controls.colorEnd as string), brightness)
        const midpoint = (controls.midpoint as number) / 100
        const softness = (controls.softness as number) / 100
        const contrast = (controls.contrast as number) / 100
        const drift = (controls.drift as number) / 100

        ctx.clearRect(0, 0, width, height)

        if ((controls.mode as string) === 'Radial') {
            const cx = width * (0.5 + Math.sin(time * (0.3 + drift * 0.4)) * 0.12)
            const cy = height * (0.5 + Math.cos(time * (0.22 + drift * 0.3)) * 0.14)
            const radius = Math.max(width, height) * (0.48 + softness * 0.26)
            const gradient = ctx.createRadialGradient(cx, cy, width * 0.02, cx, cy, radius)
            gradient.addColorStop(0, rgbToCss(start))
            gradient.addColorStop(clamp01(midpoint), rgbToCss(mid))
            gradient.addColorStop(1, rgbToCss(end))
            ctx.fillStyle = gradient
            ctx.fillRect(0, 0, width, height)
        } else {
            const angle = ((controls.angle as number) * Math.PI) / 180
            const driftOffset = Math.sin(time * (0.55 + drift * 0.9)) * width * 0.18
            const dx = Math.cos(angle) * width * 0.6
            const dy = Math.sin(angle) * height * 0.6
            const cx = width / 2 + Math.cos(time * 0.31) * driftOffset
            const cy = height / 2 + Math.sin(time * 0.27) * driftOffset * 0.6
            const gradient = ctx.createLinearGradient(cx - dx, cy - dy, cx + dx, cy + dy)
            gradient.addColorStop(0, rgbToCss(start))
            gradient.addColorStop(clamp01(midpoint - softness * 0.12), rgbToCss(mixRgb(start, mid, 0.65)))
            gradient.addColorStop(clamp01(midpoint), rgbToCss(mid))
            gradient.addColorStop(clamp01(midpoint + softness * 0.12), rgbToCss(mixRgb(mid, end, 0.55)))
            gradient.addColorStop(1, rgbToCss(end))
            ctx.fillStyle = gradient
            ctx.fillRect(0, 0, width, height)
        }

        if (contrast > 0) {
            const shadow = ctx.createLinearGradient(0, 0, width, height)
            shadow.addColorStop(0, `rgba(0, 0, 0, ${contrast * 0.24})`)
            shadow.addColorStop(0.5, 'rgba(0, 0, 0, 0)')
            shadow.addColorStop(1, `rgba(255, 255, 255, ${contrast * 0.08})`)
            ctx.fillStyle = shadow
            ctx.fillRect(0, 0, width, height)
        }
    },
    {
        author: 'Hypercolor',
        builtinId: 'gradient',
        category: 'ambient',
        description: 'Adaptive linear and radial gradients with animated drift, richer mid-stop shaping, and presets tuned for real LED hardware.',
        designBasis: BUILTIN_DESIGN_BASIS,
        presets: [
            {
                controls: {
                    angle: 24,
                    brightness: 92,
                    colorEnd: '#090d18',
                    colorMid: '#80ffea',
                    colorStart: '#e135ff',
                    contrast: 20,
                    drift: 34,
                    midpoint: 44,
                    mode: 'Linear',
                    softness: 58,
                },
                description: 'SilkCircuit candy glass washed diagonally across the whole rig.',
                name: 'Silk Bloom',
            },
            {
                controls: {
                    brightness: 84,
                    colorEnd: '#020812',
                    colorMid: '#5cf2ff',
                    colorStart: '#f6fbff',
                    contrast: 24,
                    drift: 18,
                    midpoint: 38,
                    mode: 'Radial',
                    softness: 70,
                },
                description: 'A bright glacial core diffusing outward into deep polar blue.',
                name: 'Ice Core',
            },
            {
                controls: {
                    angle: 112,
                    brightness: 78,
                    colorEnd: '#14040a',
                    colorMid: '#ff7a1a',
                    colorStart: '#ffd86a',
                    contrast: 28,
                    drift: 22,
                    midpoint: 56,
                    mode: 'Linear',
                    softness: 42,
                },
                description: 'Gold into ember with enough darkness to keep the hot zones vivid instead of chalky.',
                name: 'Solar Veil',
            },
            {
                controls: {
                    brightness: 70,
                    colorEnd: '#03030a',
                    colorMid: '#6c7dff',
                    colorStart: '#ff6ac1',
                    contrast: 30,
                    drift: 46,
                    midpoint: 50,
                    mode: 'Radial',
                    softness: 76,
                },
                description: 'A nightclub orb with soft magenta edges and icy indigo bloom.',
                name: 'Club Nebula',
            },
        ],
    },
)

