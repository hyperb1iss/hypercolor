import { canvas, combo, num, scaleContext } from '@hypercolor/sdk'

import { BUILTIN_DESIGN_BASIS, clamp01, hslCss } from '../_builtin/common'

export default canvas(
    'Rainbow',
    {
        direction: combo('Direction', ['Horizontal', 'Vertical', 'Diagonal', 'Tunnel'], {
            default: 'Horizontal',
            group: 'Motion',
        }),
        speed: num('Speed', [-100, 100], 48, { group: 'Motion' }),
        density: num('Band Density', [20, 220], 100, { group: 'Shape' }),
        saturation: num('Saturation', [0, 100], 100, { group: 'Colors' }),
        brightness: num('Brightness', [0, 100], 75, { group: 'Output' }),
        shimmer: num('Shimmer', [0, 100], 20, { group: 'Motion' }),
        blackLevel: num('Black Level', [0, 100], 8, { group: 'Output' }),
    },
    (ctx, time, controls) => {
        const s = scaleContext(ctx.canvas, BUILTIN_DESIGN_BASIS)
        const width = s.width
        const height = s.height
        const speed = controls.speed as number
        const density = (controls.density as number) / 100
        const saturation = controls.saturation as number
        const brightness = controls.brightness as number
        const shimmer = (controls.shimmer as number) / 100
        const blackLevel = clamp01((controls.blackLevel as number) / 100)
        const stripCount = Math.max(28, Math.round(48 + density * 56))

        ctx.fillStyle = `rgba(0, 0, 0, ${0.15 + blackLevel * 0.7})`
        ctx.fillRect(0, 0, width, height)

        if ((controls.direction as string) === 'Tunnel') {
            const centerX = width / 2
            const centerY = height / 2
            for (let ring = stripCount; ring >= 1; ring--) {
                const t = ring / stripCount
                const radius = Math.min(width, height) * 0.5 * t
                const hue = time * speed * 1.8 + t * 360 * density + Math.sin(time * 2 + t * 8) * shimmer * 45
                ctx.strokeStyle = hslCss(hue, saturation, brightness * (0.48 + (1 - t) * 0.42), 0.95)
                ctx.lineWidth = Math.max(2, (Math.min(width, height) / stripCount) * 0.9)
                ctx.beginPath()
                ctx.arc(centerX, centerY, radius, 0, Math.PI * 2)
                ctx.stroke()
            }
            return
        }

        ctx.save()
        if ((controls.direction as string) === 'Diagonal') {
            ctx.translate(width / 2, height / 2)
            ctx.rotate(-Math.PI / 4)
            ctx.translate(-width / 2, -height / 2)
        }

        const isVertical = (controls.direction as string) === 'Vertical'
        const major = isVertical ? height : width
        const minor = isVertical ? width : height
        const stripSize = major / stripCount

        for (let index = -2; index < stripCount + 2; index++) {
            const t = index / stripCount
            const hue =
                time * speed * 2.2 +
                t * 360 * density +
                Math.sin(time * 1.4 + index * 0.23) * shimmer * 28 +
                Math.cos(time * 0.37 + index * 0.11) * shimmer * 12
            const lightness = brightness * (0.68 + Math.sin(time * 0.75 + index * 0.18) * shimmer * 0.08)
            ctx.fillStyle = hslCss(hue, saturation, lightness)
            if (isVertical) {
                ctx.fillRect(0, index * stripSize, minor, stripSize + 2)
            } else {
                ctx.fillRect(index * stripSize, 0, stripSize + 2, minor)
            }
        }

        ctx.restore()
    },
    {
        author: 'Hypercolor',
        builtinId: 'rainbow',
        category: 'ambient',
        description: 'Rainbow sweeps with dense bands, a tunnel mode, a controllable black floor, and shimmer that stays vivid on LEDs.',
        designBasis: BUILTIN_DESIGN_BASIS,
        presets: [
            {
                controls: {
                    blackLevel: 12,
                    brightness: 72,
                    density: 90,
                    direction: 'Horizontal',
                    saturation: 100,
                    shimmer: 14,
                    speed: 40,
                },
                description: 'The clean everyday sweep: bright enough to sing, dark enough to stay tasteful.',
                name: 'Prism Sweep',
            },
            {
                controls: {
                    blackLevel: 28,
                    brightness: 82,
                    density: 130,
                    direction: 'Diagonal',
                    saturation: 96,
                    shimmer: 35,
                    speed: 58,
                },
                description: 'High-energy diagonal stripes with a little chrome sparkle in the transitions.',
                name: 'Laser Rain',
            },
            {
                controls: {
                    blackLevel: 34,
                    brightness: 68,
                    density: 120,
                    direction: 'Tunnel',
                    saturation: 100,
                    shimmer: 24,
                    speed: 32,
                },
                description: 'Concentric rainbow rings spiraling toward the center like a toy wormhole.',
                name: 'Portal Loop',
            },
        ],
    },
)

