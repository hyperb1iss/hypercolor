import { canvas, combo, getScreenZoneData, hslToRgb, num } from '@hypercolor/sdk'

import { clamp, clamp01, hslCss, wrapHue } from '../_builtin/common'

interface EdgeStop {
    hue: number
    saturation: number
    lightness: number
}

function zoneAt(screen: ReturnType<typeof getScreenZoneData>, x: number, y: number, intensity: number): EdgeStop {
    const index = y * screen.width + x
    return {
        hue: screen.hue[index] ?? 0,
        saturation: clamp01(screen.saturation[index] ?? 0),
        lightness: clamp01((screen.lightness[index] ?? 0) * intensity),
    }
}

const HUE_WEIGHT_FLOOR = 0.03

/// Average the outer band zones inward from an edge so the projected color
/// reflects the band rather than a single zone row. The circular hue mean
/// is chroma-weighted: black or gray zones carry no meaningful hue and must
/// not drag the band toward their stored (arbitrary) hue value.
function bandAverage(stops: EdgeStop[]): EdgeStop {
    if (stops.length === 0) return { hue: 0, lightness: 0, saturation: 0 }
    let x = 0
    let y = 0
    let hueWeight = 0
    let saturation = 0
    let lightness = 0
    for (const stop of stops) {
        const weight = stop.saturation * stop.lightness
        if (weight > HUE_WEIGHT_FLOOR) {
            const radians = (stop.hue * Math.PI) / 180
            x += Math.cos(radians) * weight
            y += Math.sin(radians) * weight
            hueWeight += weight
        }
        saturation += stop.saturation
        lightness += stop.lightness
    }
    if (hueWeight <= 0) {
        return { hue: 0, lightness: lightness / stops.length, saturation: 0 }
    }
    const hue = ((Math.atan2(y, x) * 180) / Math.PI + 360) % 360
    return {
        hue,
        saturation: saturation / stops.length,
        lightness: lightness / stops.length,
    }
}

function mixStops(a: EdgeStop, b: EdgeStop): EdgeStop {
    const ax = Math.cos((a.hue * Math.PI) / 180) * a.saturation
    const ay = Math.sin((a.hue * Math.PI) / 180) * a.saturation
    const bx = Math.cos((b.hue * Math.PI) / 180) * b.saturation
    const by = Math.sin((b.hue * Math.PI) / 180) * b.saturation
    const mx = (ax + bx) / 2
    const my = (ay + by) / 2
    return {
        hue: ((Math.atan2(my, mx) * 180) / Math.PI + 360) % 360,
        saturation: Math.min(1, Math.hypot(mx, my)),
        lightness: (a.lightness + b.lightness) / 2,
    }
}

/// Canvas gradients interpolate in sRGB, which collapses saturated
/// transitions (blue to red) through muddy gray midpoints on LED strips.
/// Inserting an HSL-mixed midpoint per stop pair keeps the path saturated.
function edgeGradient(
    ctx: CanvasRenderingContext2D,
    stops: EdgeStop[],
    x0: number,
    y0: number,
    x1: number,
    y1: number,
): CanvasGradient {
    const gradient = ctx.createLinearGradient(x0, y0, x1, y1)
    const last = Math.max(stops.length - 1, 1)
    stops.forEach((stop, index) => {
        gradient.addColorStop(index / last, hslCss(stop.hue, stop.saturation * 100, stop.lightness * 100))
        if (index < stops.length - 1) {
            const mid = mixStops(stop, stops[index + 1])
            gradient.addColorStop((index + 0.5) / last, hslCss(mid.hue, mid.saturation * 100, mid.lightness * 100))
        }
    })
    return gradient
}

export default canvas.stateful(
    'Ambilight',
    {
        mode: combo('Mode', ['Wash', 'Ring'], {
            default: 'Wash',
            group: 'Projection',
        }),
        edge_band: num('Edge Band', [0.05, 0.5], 0.2, {
            group: 'Ring',
            tooltip: 'Ring mode only: how deep into the screen each edge band samples.',
        }),
        ring_depth: num('Ring Depth', [0.1, 0.5], 0.25, {
            group: 'Ring',
            tooltip: 'Ring mode only: how far the edge bands reach toward the center.',
        }),
        intensity: num('Intensity', [0, 1.5], 1, { group: 'Output' }),
        center_dim: num('Center Dim', [0, 1], 0.85, {
            group: 'Ring',
            tooltip: 'Ring mode only: how dark the interior stays inside the ring.',
        }),
    },
    () => {
        const washCanvas = document.createElement('canvas')
        washCanvas.width = 1
        washCanvas.height = 1
        const washCtx = washCanvas.getContext('2d')
        let washImage: ImageData | null = null

        return (ctx, _time, controls) => {
            const screen = getScreenZoneData()
            const width = ctx.canvas.width
            const height = ctx.canvas.height
            const intensity = clamp(controls.intensity as number, 0, 1.5)
            const edgeBand = clamp(controls.edge_band as number, 0.05, 0.5)

            ctx.fillStyle = 'rgba(0, 0, 0, 1)'
            ctx.fillRect(0, 0, width, height)

            if (screen.width < 1 || screen.height < 1) return

            if ((controls.mode as string) === 'Ring') {
                // Sample band depth in zones from each edge.
                const bandCols = Math.max(1, Math.round(screen.width * edgeBand))
                const bandRows = Math.max(1, Math.round(screen.height * edgeBand))

                const topStops: EdgeStop[] = []
                const bottomStops: EdgeStop[] = []
                for (let x = 0; x < screen.width; x++) {
                    const top: EdgeStop[] = []
                    const bottom: EdgeStop[] = []
                    for (let d = 0; d < bandRows; d++) {
                        top.push(zoneAt(screen, x, d, intensity))
                        bottom.push(zoneAt(screen, x, screen.height - 1 - d, intensity))
                    }
                    topStops.push(bandAverage(top))
                    bottomStops.push(bandAverage(bottom))
                }

                const leftStops: EdgeStop[] = []
                const rightStops: EdgeStop[] = []
                for (let y = 0; y < screen.height; y++) {
                    const left: EdgeStop[] = []
                    const right: EdgeStop[] = []
                    for (let d = 0; d < bandCols; d++) {
                        left.push(zoneAt(screen, d, y, intensity))
                        right.push(zoneAt(screen, screen.width - 1 - d, y, intensity))
                    }
                    leftStops.push(bandAverage(left))
                    rightStops.push(bandAverage(right))
                }

                renderRing(ctx, width, height, controls, topStops, bottomStops, leftStops, rightStops)
                return
            }

            // Wash: fill the zone grid at native resolution into a reused
            // pixel buffer (one putImageData beats hundreds of 1px fillRects)
            // and let smooth upscaling produce the soft full-canvas wall wash.
            if (!washCtx) return
            if (washCanvas.width !== screen.width || washCanvas.height !== screen.height || !washImage) {
                washCanvas.width = screen.width
                washCanvas.height = screen.height
                washImage = washCtx.createImageData(screen.width, screen.height)
            }
            const pixels = washImage.data
            for (let y = 0; y < screen.height; y++) {
                for (let x = 0; x < screen.width; x++) {
                    const stop = zoneAt(screen, x, y, intensity)
                    const [r, g, b] = hslToRgb(wrapHue(stop.hue), stop.saturation, stop.lightness)
                    const offset = (y * screen.width + x) * 4
                    pixels[offset] = Math.round(r * 255)
                    pixels[offset + 1] = Math.round(g * 255)
                    pixels[offset + 2] = Math.round(b * 255)
                    pixels[offset + 3] = 255
                }
            }
            washCtx.putImageData(washImage, 0, 0)
            ctx.imageSmoothingEnabled = true
            ctx.drawImage(washCanvas, 0, 0, screen.width, screen.height, 0, 0, width, height)
        }
    },
    {
        author: 'Hypercolor',
        builtinId: 'ambilight',
        category: 'ambient',
        description:
            'Projects screen edge colors outward — the classic glow-behind-the-monitor look for LEDs around a desk or wall.',
        presets: [
            {
                controls: {
                    center_dim: 0.85,
                    edge_band: 0.2,
                    intensity: 1,
                    mode: 'Wash',
                    ring_depth: 0.25,
                },
                description: 'Soft full-surface wash of the whole screen for wall-washer rigs.',
                name: 'Wall Wash',
            },
            {
                controls: {
                    center_dim: 0.9,
                    edge_band: 0.15,
                    intensity: 1.1,
                    mode: 'Ring',
                    ring_depth: 0.3,
                },
                description: 'Bright edge ring with a dark center for strips that surround the display.',
                name: 'Edge Ring',
            },
            {
                controls: {
                    center_dim: 0.5,
                    edge_band: 0.35,
                    intensity: 0.9,
                    mode: 'Ring',
                    ring_depth: 0.5,
                },
                description: 'Deep, mellow ring that bleeds toward the center — cinema bias lighting.',
                name: 'Cinema Bias',
            },
        ],
        screen: true,
    },
)

/// Paint four mitered edge bands: each edge band is a gradient built from
/// that edge's zone colors, clipped to its trapezoid so corners meet
/// cleanly. The center fades toward black by `center_dim`.
function renderRing(
    ctx: CanvasRenderingContext2D,
    width: number,
    height: number,
    controls: Record<string, unknown>,
    topStops: EdgeStop[],
    bottomStops: EdgeStop[],
    leftStops: EdgeStop[],
    rightStops: EdgeStop[],
): void {
    const ringDepth = clamp(controls.ring_depth as number, 0.1, 0.5)
    const centerDim = clamp01(controls.center_dim as number)
    const bandWidth = width * ringDepth
    const bandHeight = height * ringDepth

    // Top trapezoid
    ctx.save()
    ctx.beginPath()
    ctx.moveTo(0, 0)
    ctx.lineTo(width, 0)
    ctx.lineTo(width - bandWidth, bandHeight)
    ctx.lineTo(bandWidth, bandHeight)
    ctx.closePath()
    ctx.clip()
    ctx.fillStyle = edgeGradient(ctx, topStops, 0, 0, width, 0)
    ctx.fillRect(0, 0, width, bandHeight)
    ctx.restore()

    // Bottom trapezoid
    ctx.save()
    ctx.beginPath()
    ctx.moveTo(0, height)
    ctx.lineTo(width, height)
    ctx.lineTo(width - bandWidth, height - bandHeight)
    ctx.lineTo(bandWidth, height - bandHeight)
    ctx.closePath()
    ctx.clip()
    ctx.fillStyle = edgeGradient(ctx, bottomStops, 0, 0, width, 0)
    ctx.fillRect(0, height - bandHeight, width, bandHeight)
    ctx.restore()

    // Left trapezoid
    ctx.save()
    ctx.beginPath()
    ctx.moveTo(0, 0)
    ctx.lineTo(bandWidth, bandHeight)
    ctx.lineTo(bandWidth, height - bandHeight)
    ctx.lineTo(0, height)
    ctx.closePath()
    ctx.clip()
    ctx.fillStyle = edgeGradient(ctx, leftStops, 0, 0, 0, height)
    ctx.fillRect(0, 0, bandWidth, height)
    ctx.restore()

    // Right trapezoid
    ctx.save()
    ctx.beginPath()
    ctx.moveTo(width, 0)
    ctx.lineTo(width - bandWidth, bandHeight)
    ctx.lineTo(width - bandWidth, height - bandHeight)
    ctx.lineTo(width, height)
    ctx.closePath()
    ctx.clip()
    ctx.fillStyle = edgeGradient(ctx, rightStops, 0, 0, 0, height)
    ctx.fillRect(width - bandWidth, 0, bandWidth, height)
    ctx.restore()

    // The trapezoids cover the whole canvas at deep ring depths, so the
    // interior must be re-blacked before the veil — alpha-dimmed band
    // color reads as a visible haze on LEDs, never as off.
    const innerWidth = Math.max(0, width - 2 * bandWidth)
    const innerHeight = Math.max(0, height - 2 * bandHeight)
    if (innerWidth > 0 && innerHeight > 0) {
        ctx.fillStyle = `rgba(0, 0, 0, ${centerDim})`
        ctx.fillRect(bandWidth, bandHeight, innerWidth, innerHeight)
    }

    // Center fade: a radial veil that dims the interior, letting the ring
    // bleed inward as center_dim drops.
    const veil = ctx.createRadialGradient(
        width / 2,
        height / 2,
        Math.min(width, height) * 0.1,
        width / 2,
        height / 2,
        Math.max(width, height) * 0.65,
    )
    veil.addColorStop(0, `rgba(0, 0, 0, ${centerDim})`)
    veil.addColorStop(1, 'rgba(0, 0, 0, 0)')
    ctx.fillStyle = veil
    ctx.fillRect(0, 0, width, height)
}
