import { canvas, color, combo, num } from '@hypercolor/sdk'

import { BUILTIN_DESIGN_BASIS, hexToRgb } from '../_builtin/common'

interface LinRgb {
    r: number
    g: number
    b: number
}

// sRGB byte -> linear-light [0, 1]. Precomputed once for all 256 values.
const SRGB_BYTE_TO_LINEAR = (() => {
    const table = new Float32Array(256)
    for (let i = 0; i < 256; i++) {
        const n = i / 255
        table[i] = n <= 0.04045 ? n / 12.92 : ((n + 0.055) / 1.055) ** 2.4
    }
    return table
})()

// linear-light [0, 1] -> sRGB byte. Indexed by round(linear * 1024).
const LINEAR_TO_SRGB_BYTE = (() => {
    const table = new Uint8Array(1025)
    for (let i = 0; i <= 1024; i++) {
        const l = i / 1024
        const s = l <= 0.0031308 ? l * 12.92 : 1.055 * l ** (1 / 2.4) - 0.055
        table[i] = Math.max(0, Math.min(255, Math.round(s * 255)))
    }
    return table
})()

function linearToByte(linear: number): number {
    if (linear <= 0) return 0
    if (linear >= 1) return 255
    return LINEAR_TO_SRGB_BYTE[Math.round(linear * 1024)]
}

function hexToLinearRgb(hex: string): LinRgb {
    const rgb = hexToRgb(hex)
    return {
        b: SRGB_BYTE_TO_LINEAR[Math.round(rgb.b) | 0],
        g: SRGB_BYTE_TO_LINEAR[Math.round(rgb.g) | 0],
        r: SRGB_BYTE_TO_LINEAR[Math.round(rgb.r) | 0],
    }
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

// Blend-controlled smoothstep: blend=0 is a hard step at 0.5, blend=1 spans
// the full [0, 1] range. Matches the native Rust ColorZonesRenderer.
function smoothstepBlend(t: number, blend: number): number {
    if (blend <= 1e-6) return t < 0.5 ? 0 : 1
    const half = blend * 0.5
    const lower = 0.5 - half
    const upper = 0.5 + half
    if (t <= lower) return 0
    if (t >= upper) return 1
    const n = (t - lower) / (upper - lower)
    return n * n * (3 - 2 * n)
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
        const width = ctx.canvas.width
        const height = ctx.canvas.height
        if (width <= 0 || height <= 0) return

        const zoneCount = Math.max(1, Math.min(9, Number.parseInt(controls.zoneCount as string, 10) || 3))
        const brightness = Math.max(0, Math.min(1, (controls.brightness as number) / 100))
        const blend = Math.max(0, Math.min(1, (controls.blend as number) / 100))
        const sheenAmount = Math.max(0, Math.min(1, (controls.sheen as number) / 100))
        const vignetteAmount = Math.max(0, Math.min(1, (controls.vignette as number) / 100))
        const layout = controls.layout as string

        const [rows, cols] = gridDimensions(layout, zoneCount)

        const zones: LinRgb[] = [
            hexToLinearRgb(controls.zone1 as string),
            hexToLinearRgb(controls.zone2 as string),
            hexToLinearRgb(controls.zone3 as string),
            hexToLinearRgb(controls.zone4 as string),
            hexToLinearRgb(controls.zone5 as string),
            hexToLinearRgb(controls.zone6 as string),
            hexToLinearRgb(controls.zone7 as string),
            hexToLinearRgb(controls.zone8 as string),
            hexToLinearRgb(controls.zone9 as string),
        ]

        // Apply brightness in linear-light space so "dim" looks right on LEDs
        // after the display/driver applies gamma.
        const scaled: LinRgb[] = zones.map((z) => ({
            b: z.b * brightness,
            g: z.g * brightness,
            r: z.r * brightness,
        }))

        const zoneAt = (row: number, col: number): LinRgb => {
            const clampedCol = Math.max(0, Math.min(cols - 1, col))
            const clampedRow = Math.max(0, Math.min(rows - 1, row))
            const idx = Math.min(clampedRow * cols + clampedCol, zoneCount - 1)
            return scaled[idx]
        }

        const maxBaseCol = Math.max(0, cols - 2)
        const maxBaseRow = Math.max(0, rows - 2)

        const image = ctx.createImageData(width, height)
        const data = image.data

        // Sheen: soft diagonal silk band across the upper-left → lower-right axis.
        // Peak at ~0.35 along the diagonal, gentle falloff.
        const sheenPeak = 0.35
        const sheenFalloff = 3.0
        const sheenGain = sheenAmount * 0.18

        // Vignette: quadratic darkening from center to corners.
        // At amount=1.0, corners drop to ~45% brightness. At amount=0.1, ~94%.
        const vignetteGain = vignetteAmount * 0.55

        for (let y = 0; y < height; y++) {
            const ny = (y + 0.5) / height
            const gy = ny * rows
            const baseRow = Math.max(0, Math.min(maxBaseRow, Math.floor(gy - 0.5)))
            const centerTop = baseRow + 0.5
            const fy = Math.max(0, Math.min(1, gy - centerTop))
            const sy = smoothstepBlend(fy, blend)
            const bottomRow = baseRow + 1

            const dy = ny - 0.5

            for (let x = 0; x < width; x++) {
                const nx = (x + 0.5) / width
                const gx = nx * cols
                const baseCol = Math.max(0, Math.min(maxBaseCol, Math.floor(gx - 0.5)))
                const centerLeft = baseCol + 0.5
                const fx = Math.max(0, Math.min(1, gx - centerLeft))
                const sx = smoothstepBlend(fx, blend)
                const rightCol = baseCol + 1

                const c00 = zoneAt(baseRow, baseCol)
                const c10 = zoneAt(baseRow, rightCol)
                const c01 = zoneAt(bottomRow, baseCol)
                const c11 = zoneAt(bottomRow, rightCol)

                // Bilinear interpolation in linear-light RGB.
                const topR = c00.r + (c10.r - c00.r) * sx
                const topG = c00.g + (c10.g - c00.g) * sx
                const topB = c00.b + (c10.b - c00.b) * sx
                const botR = c01.r + (c11.r - c01.r) * sx
                const botG = c01.g + (c11.g - c01.g) * sx
                const botB = c01.b + (c11.b - c01.b) * sx
                let r = topR + (botR - topR) * sy
                let g = topG + (botG - topG) * sy
                let b = topB + (botB - topB) * sy

                if (vignetteGain > 0) {
                    const dx = nx - 0.5
                    // Normalize radial distance so corners reach 1.0 (2 * 0.5^2 = 0.5).
                    const distSq = Math.min(1, (dx * dx + dy * dy) * 2)
                    const shade = 1 - distSq * vignetteGain
                    r *= shade
                    g *= shade
                    b *= shade
                }

                if (sheenGain > 0) {
                    const diag = (nx + ny) * 0.5
                    const band = Math.max(0, 1 - Math.abs(diag - sheenPeak) * sheenFalloff)
                    const bump = band * band * sheenGain
                    r += bump
                    g += bump
                    b += bump
                }

                const i = (y * width + x) * 4
                data[i] = linearToByte(r)
                data[i + 1] = linearToByte(g)
                data[i + 2] = linearToByte(b)
                data[i + 3] = 255
            }
        }

        ctx.putImageData(image, 0, 0)
    },
    {
        author: 'Hypercolor',
        builtinId: 'color_zones',
        category: 'ambient',
        description:
            'A multi-zone scene builder with clean geometric boundaries, smooth seam blending, and curated palettes for room setups.',
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
                description: 'A three-column neon spread for keyboards, underglow, and ambient bars.',
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
                description:
                    'Top-to-bottom status colors that stay readable when zones need to communicate at a glance.',
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
                description: 'A candy-glass control wall with enough variation to carry a full nine-zone layout.',
                name: 'Candy Control',
            },
        ],
    },
)
