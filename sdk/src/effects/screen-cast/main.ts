import { canvas, combo, getScreenZoneData, num, rect } from '@hypercolor/sdk'

import { hslCss } from '../_builtin/common'

interface RectValue {
    x: number
    y: number
    width: number
    height: number
}

const MIN_VIEWPORT_EDGE = 0.02

function clamp(value: number, min: number, max: number): number {
    return Math.max(min, Math.min(max, value))
}

function clampViewport(viewport: RectValue): RectValue {
    const width = clamp(Number.isFinite(viewport.width) ? viewport.width : 1, MIN_VIEWPORT_EDGE, 1)
    const height = clamp(Number.isFinite(viewport.height) ? viewport.height : 1, MIN_VIEWPORT_EDGE, 1)
    const x = clamp(Number.isFinite(viewport.x) ? viewport.x : 0, 0, Math.max(0, 1 - width))
    const y = clamp(Number.isFinite(viewport.y) ? viewport.y : 0, 0, Math.max(0, 1 - height))
    return { x, y, width, height }
}

function fitRect(
    fitMode: string,
    sourceWidth: number,
    sourceHeight: number,
    targetWidth: number,
    targetHeight: number,
): [number, number, number, number] {
    if (fitMode === 'Stretch') return [0, 0, targetWidth, targetHeight]

    const sourceAspect = sourceWidth / Math.max(sourceHeight, 0.0001)
    const targetAspect = targetWidth / Math.max(targetHeight, 0.0001)

    if ((fitMode === 'Contain' && targetAspect > sourceAspect) || (fitMode === 'Cover' && targetAspect < sourceAspect)) {
        const height = targetHeight
        const width = height * sourceAspect
        return [(targetWidth - width) * 0.5, 0, width, height]
    }

    const width = targetWidth
    const height = width / sourceAspect
    return [0, (targetHeight - height) * 0.5, width, height]
}

export default canvas.stateful(
    'Screen Cast',
    {
        viewport: rect('Viewport', { x: 0, y: 0, width: 1, height: 1 }, {
            group: 'Frame',
            preview: 'screen',
        }),
        fit_mode: combo('Fit Mode', ['Contain', 'Cover', 'Stretch'], {
            default: 'Contain',
            group: 'Frame',
        }),
        brightness: num('Brightness', [0, 1], 1, { group: 'Output' }),
    },
    () => {
        const sourceCanvas = document.createElement('canvas')
        sourceCanvas.width = 1
        sourceCanvas.height = 1
        const sourceCtx = sourceCanvas.getContext('2d')

        return (ctx, _time, controls) => {
            if (!sourceCtx) return

            const screen = getScreenZoneData()
            const brightness = clamp(controls.brightness as number, 0, 1)
            const viewport = clampViewport(controls.viewport as RectValue)

            if (sourceCanvas.width !== screen.width || sourceCanvas.height !== screen.height) {
                sourceCanvas.width = screen.width
                sourceCanvas.height = screen.height
            }

            sourceCtx.clearRect(0, 0, screen.width, screen.height)
            for (let y = 0; y < screen.height; y++) {
                for (let x = 0; x < screen.width; x++) {
                    const index = y * screen.width + x
                    const lightness = clamp(screen.lightness[index] * brightness, 0, 1)
                    sourceCtx.fillStyle = hslCss(screen.hue[index], screen.saturation[index] * 100, lightness * 100)
                    sourceCtx.fillRect(x, y, 1, 1)
                }
            }

            const sx = viewport.x * screen.width
            const sy = viewport.y * screen.height
            const sw = viewport.width * screen.width
            const sh = viewport.height * screen.height
            const [dx, dy, dw, dh] = fitRect(controls.fit_mode as string, sw, sh, ctx.canvas.width, ctx.canvas.height)

            ctx.fillStyle = 'rgba(0, 0, 0, 1)'
            ctx.fillRect(0, 0, ctx.canvas.width, ctx.canvas.height)
            ctx.drawImage(sourceCanvas, sx, sy, sw, sh, dx, dy, dw, dh)
        }
    },
    {
        author: 'Hypercolor',
        builtinId: 'screen_cast',
        category: 'utility',
        description: 'Live screen crop with contain, cover, and stretch fit modes.',
        presets: [
            {
                controls: {
                    brightness: 1,
                    fit_mode: 'Contain',
                    viewport: { x: 0, y: 0, width: 1, height: 1 },
                },
                description: 'Full-frame live capture preserved at the capture aspect ratio.',
                name: 'Full Preview',
            },
            {
                controls: {
                    brightness: 1,
                    fit_mode: 'Cover',
                    viewport: { x: 0.18, y: 0.1, width: 0.62, height: 0.64 },
                },
                description: 'Cropped desk-center view that fills the rig for focused UI mirroring.',
                name: 'Desk Focus',
            },
            {
                controls: {
                    brightness: 1,
                    fit_mode: 'Contain',
                    viewport: { x: 0.3, y: 0.3, width: 0.4, height: 0.4 },
                },
                description: 'Zoomed center square for highlighting the middle of the screen.',
                name: 'Center Zoom',
            },
        ],
        screen: true,
    },
)
