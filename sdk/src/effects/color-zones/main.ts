import { canvas, color, combo, num, scaleContext } from '@hypercolor/sdk'

import {
    BUILTIN_DESIGN_BASIS,
    hexToRgb,
    mixRgb,
    rgbToCss,
    scaleRgb,
    withLift,
} from '../_builtin/common'

interface ZoneRect {
    x: number
    y: number
    width: number
    height: number
    color: ReturnType<typeof hexToRgb>
}

function gridDimensions(layout: string, zoneCount: number): [number, number] {
    if (layout === 'Rows') return [zoneCount, 1]
    if (layout === 'Columns') return [1, zoneCount]

    if (zoneCount === 2) return [1, 2]
    if (zoneCount === 3) return [1, 3]
    if (zoneCount === 4) return [2, 2]
    if (zoneCount === 5) return [1, 5]
    if (zoneCount === 6) return [2, 3]
    if (zoneCount === 7) return [1, 7]
    if (zoneCount === 8) return [2, 4]
    return [3, 3]
}

function buildRects(
    width: number,
    height: number,
    layout: string,
    zoneCount: number,
    colors: ReturnType<typeof hexToRgb>[],
): ZoneRect[] {
    const [rows, cols] = gridDimensions(layout, zoneCount)
    const rects: ZoneRect[] = []
    const cellWidth = width / cols
    const cellHeight = height / rows

    for (let row = 0; row < rows; row++) {
        for (let col = 0; col < cols; col++) {
            const index = row * cols + col
            if (index >= zoneCount) continue

            rects.push({
                color: colors[index],
                height: cellHeight,
                width: cellWidth,
                x: col * cellWidth,
                y: row * cellHeight,
            })
        }
    }

    return rects
}

function averageColor(colors: ReturnType<typeof hexToRgb>[]): ReturnType<typeof hexToRgb> {
    const count = Math.max(colors.length, 1)
    const sum = colors.reduce(
        (acc, color) => ({
            b: acc.b + color.b,
            g: acc.g + color.g,
            r: acc.r + color.r,
        }),
        { b: 0, g: 0, r: 0 },
    )

    return {
        b: sum.b / count,
        g: sum.g / count,
        r: sum.r / count,
    }
}

export default canvas(
    'Color Zones',
    {
        zoneCount: combo('Zone Count', ['2', '3', '4', '5', '6', '7', '8', '9'], {
            default: '3',
            group: 'Layout',
        }),
        layout: combo('Layout', ['Columns', 'Rows', 'Grid'], { default: 'Columns', group: 'Layout' }),
        blend: num('Blend Softness', [0, 100], 32, { group: 'Layout' }),
        sheen: num('Sheen', [0, 100], 18, { group: 'Output' }),
        vignette: num('Vignette', [0, 100], 10, { group: 'Output' }),
        brightness: num('Brightness', [0, 100], 90, { group: 'Output' }),
        zone1: color('Zone 1', '#e135ff', { group: 'Zone Colors' }),
        zone2: color('Zone 2', '#80ffea', { group: 'Zone Colors' }),
        zone3: color('Zone 3', '#ff6ac1', { group: 'Zone Colors' }),
        zone4: color('Zone 4', '#50fa7b', { group: 'Zone Colors' }),
        zone5: color('Zone 5', '#f1fa8c', { group: 'Zone Colors' }),
        zone6: color('Zone 6', '#ff6363', { group: 'Zone Colors' }),
        zone7: color('Zone 7', '#4f7dff', { group: 'Zone Colors' }),
        zone8: color('Zone 8', '#ff9f45', { group: 'Zone Colors' }),
        zone9: color('Zone 9', '#6c2bff', { group: 'Zone Colors' }),
    },
    (ctx, _time, controls) => {
        const s = scaleContext(ctx.canvas, BUILTIN_DESIGN_BASIS)
        const width = s.width
        const height = s.height
        const zoneCount = Number.parseInt(controls.zoneCount as string, 10) || 3
        const brightness = (controls.brightness as number) / 100
        const blend = (controls.blend as number) / 100
        const sheen = (controls.sheen as number) / 100
        const vignette = (controls.vignette as number) / 100
        const colors = [
            hexToRgb(controls.zone1 as string),
            hexToRgb(controls.zone2 as string),
            hexToRgb(controls.zone3 as string),
            hexToRgb(controls.zone4 as string),
            hexToRgb(controls.zone5 as string),
            hexToRgb(controls.zone6 as string),
            hexToRgb(controls.zone7 as string),
            hexToRgb(controls.zone8 as string),
            hexToRgb(controls.zone9 as string),
        ]
            .slice(0, zoneCount)
            .map((colorValue) => scaleRgb(colorValue, brightness))

        const rects = buildRects(width, height, controls.layout as string, zoneCount, colors)
        const ambient = scaleRgb(mixRgb(averageColor(colors), { r: 0, g: 0, b: 0 }, 0.72), 0.9)

        ctx.fillStyle = rgbToCss(ambient)
        ctx.fillRect(0, 0, width, height)

        for (const rect of rects) {
            ctx.fillStyle = rgbToCss(rect.color)
            ctx.fillRect(rect.x, rect.y, rect.width, rect.height)
        }

        if (blend > 0) {
            const [rows, cols] = gridDimensions(controls.layout as string, zoneCount)
            const seamWidth = Math.max(8, Math.min(width / cols, height / rows) * blend * 0.6)

            for (let row = 0; row < rows; row++) {
                for (let col = 0; col < cols - 1; col++) {
                    const leftIndex = row * cols + col
                    const rightIndex = leftIndex + 1
                    if (rightIndex >= rects.length) continue

                    const left = rects[leftIndex]
                    const right = rects[rightIndex]
                    const seamX = left.x + left.width
                    const gradient = ctx.createLinearGradient(seamX - seamWidth * 0.5, 0, seamX + seamWidth * 0.5, 0)
                    gradient.addColorStop(0, rgbToCss(left.color))
                    gradient.addColorStop(0.5, rgbToCss(scaleRgb(mixRgb(left.color, right.color, 0.5), 1.02)))
                    gradient.addColorStop(1, rgbToCss(right.color))
                    ctx.fillStyle = gradient
                    ctx.fillRect(seamX - seamWidth * 0.5, left.y, seamWidth, left.height)
                }
            }

            for (let row = 0; row < rows - 1; row++) {
                for (let col = 0; col < cols; col++) {
                    const topIndex = row * cols + col
                    const bottomIndex = topIndex + cols
                    if (bottomIndex >= rects.length) continue

                    const top = rects[topIndex]
                    const bottom = rects[bottomIndex]
                    const seamY = top.y + top.height
                    const gradient = ctx.createLinearGradient(0, seamY - seamWidth * 0.5, 0, seamY + seamWidth * 0.5)
                    gradient.addColorStop(0, rgbToCss(top.color))
                    gradient.addColorStop(0.5, rgbToCss(scaleRgb(mixRgb(top.color, bottom.color, 0.5), 1.02)))
                    gradient.addColorStop(1, rgbToCss(bottom.color))
                    ctx.fillStyle = gradient
                    ctx.fillRect(top.x, seamY - seamWidth * 0.5, top.width, seamWidth)
                }
            }
        }

        if (sheen > 0) {
            const sheenGradient = ctx.createLinearGradient(0, 0, width, height)
            sheenGradient.addColorStop(0, rgbToCss(withLift(ambient, 0.5), 0.12 + sheen * 0.08))
            sheenGradient.addColorStop(0.45, 'rgba(255, 255, 255, 0)')
            sheenGradient.addColorStop(0.7, rgbToCss(withLift(ambient, 0.75), 0.04 + sheen * 0.06))
            sheenGradient.addColorStop(1, 'rgba(255, 255, 255, 0)')
            ctx.fillStyle = sheenGradient
            ctx.fillRect(0, 0, width, height)
        }

        if (vignette > 0) {
            const shadow = ctx.createRadialGradient(width / 2, height / 2, Math.min(width, height) * 0.2, width / 2, height / 2, Math.max(width, height) * 0.72)
            shadow.addColorStop(0, 'rgba(0, 0, 0, 0)')
            shadow.addColorStop(1, `rgba(0, 0, 0, ${0.08 + vignette * 0.38})`)
            ctx.fillStyle = shadow
            ctx.fillRect(0, 0, width, height)
        }
    },
    {
        author: 'Hypercolor',
        builtinId: 'color_zones',
        category: 'ambient',
        description:
            'A multi-zone scene builder with clean geometric boundaries, smooth seam blending, and richer preset palettes for actual room setups.',
        designBasis: BUILTIN_DESIGN_BASIS,
        presets: [
            {
                controls: {
                    blend: 30,
                    brightness: 90,
                    layout: 'Columns',
                    sheen: 22,
                    vignette: 12,
                    zone1: '#e135ff',
                    zone2: '#80ffea',
                    zone3: '#ff6ac1',
                    zoneCount: '3',
                },
                description: 'A simple three-column SilkCircuit spread for keyboards, underglow, and ambient bars.',
                name: 'Silk Stack',
            },
            {
                controls: {
                    blend: 12,
                    brightness: 94,
                    layout: 'Rows',
                    sheen: 10,
                    vignette: 8,
                    zone1: '#ff6363',
                    zone2: '#f1fa8c',
                    zone3: '#50fa7b',
                    zone4: '#80ffea',
                    zoneCount: '4',
                },
                description: 'Top-to-bottom status colors that stay readable when you need zones to communicate, not just look pretty.',
                name: 'Status Ladder',
            },
            {
                controls: {
                    blend: 52,
                    brightness: 82,
                    layout: 'Grid',
                    sheen: 20,
                    vignette: 16,
                    zone1: '#08264b',
                    zone2: '#1358ff',
                    zone3: '#46f1dc',
                    zone4: '#80ffea',
                    zone5: '#03111f',
                    zone6: '#1746ff',
                    zoneCount: '6',
                },
                description: 'A cool blue command deck with soft seams that read beautifully across split layouts.',
                name: 'Ocean Grid',
            },
            {
                controls: {
                    blend: 40,
                    brightness: 88,
                    layout: 'Grid',
                    sheen: 26,
                    vignette: 18,
                    zone1: '#ff6ac1',
                    zone2: '#ff9f45',
                    zone3: '#f1fa8c',
                    zone4: '#80ffea',
                    zone5: '#7d49ff',
                    zone6: '#50fa7b',
                    zone7: '#ff6363',
                    zone8: '#4f7dff',
                    zone9: '#14061f',
                    zoneCount: '9',
                },
                description: 'A candy-glass control wall with enough variation to make full layouts feel intentionally composed.',
                name: 'Candy Control',
            },
        ],
    },
)
