import { canvas, combo, getScreenZoneData, num, toggle } from '@hypercolor/sdk'

import { hslCss, wrapHue } from '../_builtin/common'

function clamp(value: number, min: number, max: number): number {
    return Math.max(min, Math.min(max, value))
}

function smoothApproach(current: number, target: number, lambda: number, dt: number): number {
    if (!Number.isFinite(lambda) || lambda <= 0) return target
    const factor = 1 - Math.exp(-lambda * Math.max(dt, 0))
    return current + (target - current) * factor
}

function smoothHue(current: number, target: number, lambda: number, dt: number): number {
    const diff = ((((target - current) % 360) + 540) % 360) - 180
    return wrapHue(current + diff * (1 - Math.exp(-lambda * Math.max(dt, 0))))
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
        fitMode: combo('Fit Mode', ['Contain', 'Cover', 'Stretch'], { default: 'Contain', group: 'Frame' }),
        style: combo('Style', ['Natural', 'Vivid', 'Pixelated', 'Neon'], { default: 'Natural', group: 'Output' }),
        frameX: num('Frame X', [0, 100], 0, { group: 'Frame' }),
        frameY: num('Frame Y', [0, 100], 0, { group: 'Frame' }),
        frameWidth: num('Frame Width', [5, 100], 100, { group: 'Frame' }),
        frameHeight: num('Frame Height', [5, 100], 100, { group: 'Frame' }),
        brightness: num('Brightness', [0, 100], 92, { group: 'Output' }),
        saturation: num('Saturation', [0, 200], 108, { group: 'Output' }),
        contrast: num('Contrast', [0, 200], 108, { group: 'Output' }),
        smoothness: num('Smoothness', [0, 100], 42, { group: 'Output' }),
        showGrid: toggle('Show Grid', false, { group: 'Output' }),
    },
    () => {
        const sourceCanvas = document.createElement('canvas')
        sourceCanvas.width = 1
        sourceCanvas.height = 1
        const sourceCtx = sourceCanvas.getContext('2d')

        let smoothedHue = new Float32Array(0)
        let smoothedSaturation = new Float32Array(0)
        let smoothedLightness = new Float32Array(0)
        let lastTime = 0

        return (ctx, time, controls) => {
            const screen = getScreenZoneData()
            const sampleCount = screen.width * screen.height
            const dt = Math.min(lastTime > 0 ? time - lastTime : 0.016, 0.05)
            lastTime = time

            if (!sourceCtx) return

            if (sourceCanvas.width !== screen.width || sourceCanvas.height !== screen.height) {
                sourceCanvas.width = screen.width
                sourceCanvas.height = screen.height
            }

            if (smoothedHue.length !== sampleCount) {
                smoothedHue = new Float32Array(sampleCount)
                smoothedSaturation = new Float32Array(sampleCount)
                smoothedLightness = new Float32Array(sampleCount)
                for (let i = 0; i < sampleCount; i++) {
                    smoothedHue[i] = screen.hue[i]
                    smoothedSaturation[i] = screen.saturation[i]
                    smoothedLightness[i] = screen.lightness[i]
                }
            }

            const brightness = (controls.brightness as number) / 100
            const saturationBoost = (controls.saturation as number) / 100
            const contrast = (controls.contrast as number) / 100
            const smoothness = (controls.smoothness as number) / 100
            const style = controls.style as string
            const smoothingLambda = 3 + smoothness * 18

            for (let i = 0; i < sampleCount; i++) {
                smoothedHue[i] = smoothHue(smoothedHue[i], screen.hue[i], smoothingLambda, dt)
                smoothedSaturation[i] = smoothApproach(smoothedSaturation[i], screen.saturation[i], smoothingLambda, dt)
                smoothedLightness[i] = smoothApproach(smoothedLightness[i], screen.lightness[i], smoothingLambda, dt)
            }

            sourceCtx.clearRect(0, 0, screen.width, screen.height)
            for (let y = 0; y < screen.height; y++) {
                for (let x = 0; x < screen.width; x++) {
                    const index = y * screen.width + x
                    const hue = smoothedHue[index]
                    const saturation = clamp(smoothedSaturation[index] * saturationBoost, 0, 1)
                    const lightness = clamp((smoothedLightness[index] - 0.5) * contrast + 0.5, 0, 1) * brightness
                    sourceCtx.fillStyle = hslCss(hue, saturation * 100, lightness * 100)
                    sourceCtx.fillRect(x, y, 1, 1)
                }
            }

            const cropX = clamp((controls.frameX as number) / 100, 0, 1)
            const cropY = clamp((controls.frameY as number) / 100, 0, 1)
            const cropWidth = clamp((controls.frameWidth as number) / 100, 0.05, 1)
            const cropHeight = clamp((controls.frameHeight as number) / 100, 0.05, 1)
            const sx = cropX * screen.width
            const sy = cropY * screen.height
            const sw = cropWidth * screen.width
            const sh = cropHeight * screen.height
            const [dx, dy, dw, dh] = fitRect(controls.fitMode as string, sw, sh, ctx.canvas.width, ctx.canvas.height)

            ctx.fillStyle = style === 'Neon' ? 'rgba(3, 4, 10, 1)' : 'rgba(0, 0, 0, 1)'
            ctx.fillRect(0, 0, ctx.canvas.width, ctx.canvas.height)

            ctx.save()
            ctx.imageSmoothingEnabled = style !== 'Pixelated'
            ctx.drawImage(sourceCanvas, sx, sy, sw, sh, dx, dy, dw, dh)
            ctx.restore()

            if (style === 'Vivid' || style === 'Neon') {
                const overlay = ctx.createLinearGradient(dx, dy, dx + dw, dy + dh)
                overlay.addColorStop(0, 'rgba(255, 255, 255, 0.06)')
                overlay.addColorStop(0.5, 'rgba(255, 255, 255, 0)')
                overlay.addColorStop(1, 'rgba(255, 255, 255, 0.08)')
                ctx.fillStyle = overlay
                ctx.fillRect(dx, dy, dw, dh)
            }

            if (style === 'Neon') {
                ctx.save()
                ctx.globalCompositeOperation = 'lighter'
                const glow = ctx.createRadialGradient(dx + dw * 0.5, dy + dh * 0.5, Math.min(dw, dh) * 0.15, dx + dw * 0.5, dy + dh * 0.5, Math.max(dw, dh) * 0.72)
                glow.addColorStop(0, 'rgba(128, 255, 234, 0.12)')
                glow.addColorStop(1, 'rgba(225, 53, 255, 0)')
                ctx.fillStyle = glow
                ctx.fillRect(dx, dy, dw, dh)
                ctx.restore()
            }

            if ((controls.showGrid as boolean) || style === 'Pixelated') {
                const cols = Math.max(1, Math.floor(sw))
                const rows = Math.max(1, Math.floor(sh))
                ctx.save()
                ctx.strokeStyle = 'rgba(255, 255, 255, 0.12)'
                ctx.lineWidth = 1
                for (let col = 1; col < cols; col++) {
                    const x = dx + (col / cols) * dw
                    ctx.beginPath()
                    ctx.moveTo(x, dy)
                    ctx.lineTo(x, dy + dh)
                    ctx.stroke()
                }
                for (let row = 1; row < rows; row++) {
                    const y = dy + (row / rows) * dh
                    ctx.beginPath()
                    ctx.moveTo(dx, y)
                    ctx.lineTo(dx + dw, y)
                    ctx.stroke()
                }
                ctx.restore()
            }
        }
    },
    {
        author: 'Hypercolor',
        builtinId: 'screen_cast',
        category: 'utility',
        description:
            'Live screen-reactive sampling with crop controls, fit modes, smoothing, and display treatments built for actual LED layouts instead of raw debug output.',
        presets: [
            {
                controls: {
                    brightness: 94,
                    contrast: 104,
                    fitMode: 'Contain',
                    frameHeight: 100,
                    frameWidth: 100,
                    frameX: 0,
                    frameY: 0,
                    saturation: 108,
                    showGrid: false,
                    smoothness: 38,
                    style: 'Natural',
                },
                description: 'Balanced full-frame preview with just enough smoothing to calm screen capture chatter.',
                name: 'Full Preview',
            },
            {
                controls: {
                    brightness: 92,
                    contrast: 112,
                    fitMode: 'Cover',
                    frameHeight: 64,
                    frameWidth: 62,
                    frameX: 18,
                    frameY: 10,
                    saturation: 118,
                    showGrid: false,
                    smoothness: 44,
                    style: 'Vivid',
                },
                description: 'A cropped desk-center view that fills the rig and keeps UI surfaces punchy.',
                name: 'Desk Focus',
            },
            {
                controls: {
                    brightness: 90,
                    contrast: 120,
                    fitMode: 'Stretch',
                    frameHeight: 100,
                    frameWidth: 100,
                    frameX: 0,
                    frameY: 0,
                    saturation: 130,
                    showGrid: true,
                    smoothness: 18,
                    style: 'Pixelated',
                },
                description: 'A crunchy debug-friendly view for reading the sampling grid directly on the hardware.',
                name: 'Pixel Grid',
            },
            {
                controls: {
                    brightness: 86,
                    contrast: 128,
                    fitMode: 'Contain',
                    frameHeight: 84,
                    frameWidth: 84,
                    frameX: 8,
                    frameY: 8,
                    saturation: 142,
                    showGrid: false,
                    smoothness: 52,
                    style: 'Neon',
                },
                description: 'A glam live-capture treatment with boosted color and a soft club-light halo around the sample.',
                name: 'Neon Cast',
            },
        ],
        screen: true,
    },
)
