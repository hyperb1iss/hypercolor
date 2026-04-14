import { canvas, color, combo, num, scaleContext } from '@hypercolor/sdk'

import { BUILTIN_DESIGN_BASIS, easeInOutSine, hexToRgb, mixRgb, rgbToCss, scaleRgb } from '../_builtin/common'

export default canvas(
    'Breathing',
    {
        mode: combo('Mode', ['Single', 'Dual', 'Aurora'], { default: 'Single', group: 'Pattern' }),
        color: color('Primary Color', '#ff8a33', { group: 'Colors' }),
        secondaryColor: color('Secondary Color', '#80ffea', { group: 'Colors' }),
        speed: num('Speed', [2, 60], 15, { group: 'Motion' }),
        minBrightness: num('Minimum Brightness', [0, 100], 10, { group: 'Output' }),
        maxBrightness: num('Maximum Brightness', [0, 100], 100, { group: 'Output' }),
        glow: num('Glow', [0, 100], 35, { group: 'Output' }),
        drift: num('Drift', [0, 100], 20, { group: 'Motion' }),
    },
    (ctx, time, controls) => {
        const s = scaleContext(ctx.canvas, BUILTIN_DESIGN_BASIS)
        const width = s.width
        const height = s.height
        const primary = hexToRgb(controls.color as string)
        const secondary = hexToRgb(controls.secondaryColor as string)
        const minBrightness = (controls.minBrightness as number) / 100
        const maxBrightness = (controls.maxBrightness as number) / 100
        const glow = (controls.glow as number) / 100
        const drift = (controls.drift as number) / 100
        const pulse = easeInOutSine(0.5 + 0.5 * Math.sin(time * ((controls.speed as number) / 60) * Math.PI * 2))
        const brightness = minBrightness + (maxBrightness - minBrightness) * pulse
        const mode = controls.mode as string

        ctx.clearRect(0, 0, width, height)
        ctx.fillStyle = rgbToCss(scaleRgb(mixRgb(primary, secondary, 0.08), brightness * 0.12))
        ctx.fillRect(0, 0, width, height)

        if (mode === 'Dual') {
            const gradient = ctx.createLinearGradient(0, 0, width, height)
            gradient.addColorStop(0, rgbToCss(scaleRgb(primary, brightness)))
            gradient.addColorStop(1, rgbToCss(scaleRgb(secondary, brightness * 0.92)))
            ctx.fillStyle = gradient
            ctx.fillRect(0, 0, width, height)
        } else {
            const cx = s.dx(160 + Math.sin(time * (0.28 + drift * 0.8)) * 26)
            const cy = s.dy(100 + Math.cos(time * (0.22 + drift * 0.65)) * 18)
            const outer = s.ds(112 + glow * 64 + pulse * 26)
            const gradient = ctx.createRadialGradient(cx, cy, s.ds(8), cx, cy, outer)
            gradient.addColorStop(0, rgbToCss(scaleRgb(primary, brightness)))
            gradient.addColorStop(
                0.48,
                rgbToCss(scaleRgb(mode === 'Aurora' ? mixRgb(primary, secondary, 0.5) : primary, brightness * 0.55)),
            )
            gradient.addColorStop(1, rgbToCss(scaleRgb(mode === 'Aurora' ? secondary : primary, brightness * 0.08), 0))
            ctx.fillStyle = gradient
            ctx.fillRect(0, 0, width, height)

            if (mode === 'Aurora') {
                const ribbon = ctx.createLinearGradient(0, height * 0.18, width, height * 0.82)
                ribbon.addColorStop(0, rgbToCss(scaleRgb(secondary, brightness * 0.12), 0))
                ribbon.addColorStop(0.5, rgbToCss(scaleRgb(secondary, brightness * 0.55), 0.55))
                ribbon.addColorStop(1, rgbToCss(scaleRgb(primary, brightness * 0.18), 0))
                ctx.fillStyle = ribbon
                ctx.fillRect(0, 0, width, height)
            }
        }
    },
    {
        author: 'Hypercolor',
        builtinId: 'breathing',
        category: 'ambient',
        description:
            'Soft pulse lighting with shaped glow, dual-color crossfades, and an aurora mode for layered ambient scenes.',
        designBasis: BUILTIN_DESIGN_BASIS,
        presets: [
            {
                controls: {
                    color: '#ff6d2a',
                    drift: 14,
                    glow: 28,
                    maxBrightness: 78,
                    minBrightness: 5,
                    mode: 'Single',
                    speed: 8,
                },
                description: 'A warm ember inhale and exhale that stays cozy instead of glaring.',
                name: 'Warm Ember',
            },
            {
                controls: {
                    color: '#2d4dff',
                    drift: 22,
                    glow: 24,
                    maxBrightness: 68,
                    minBrightness: 8,
                    mode: 'Single',
                    speed: 6,
                },
                description: 'Slow tidal blue breathing with plenty of darkness left in the room.',
                name: 'Ocean Calm',
            },
            {
                controls: {
                    color: '#ff4444',
                    drift: 8,
                    glow: 18,
                    maxBrightness: 100,
                    minBrightness: 18,
                    mode: 'Dual',
                    secondaryColor: '#fff0f0',
                    speed: 34,
                },
                description: 'Sharper dual-tone pulsing for warnings, timers, and “look here now” moments.',
                name: 'Alert Pulse',
            },
            {
                controls: {
                    color: '#e135ff',
                    drift: 30,
                    glow: 58,
                    maxBrightness: 84,
                    minBrightness: 10,
                    mode: 'Aurora',
                    secondaryColor: '#80ffea',
                    speed: 11,
                },
                description: 'A drifting neon bloom with layered color and a slow crossfade.',
                name: 'Silk Bloom',
            },
        ],
    },
)
