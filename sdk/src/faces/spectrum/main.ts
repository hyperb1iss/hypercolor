import type { FaceContext } from '@hypercolor/sdk'
import { color, combo, face, lerpColor, num, palette, Smoothed, withAlpha } from '@hypercolor/sdk'
import { drawNebulaField } from '../shared/atmosphere'
import { clamp01, createFaceRoot } from '../shared/dom'

const PEAK_FALL_PER_SEC = 0.35
const PEAK_HOLD_SECS = 0.45
const BAR_DECAY_HALFLIFE = 0.16
const SILENCE_LEVEL = 0.015
const SILENCE_AFTER_SECS = 2
const PITCH_HUES = [330, 0, 30, 55, 80, 120, 160, 190, 215, 250, 275, 300]

interface BandState {
    value: number
    peak: number
    peakHeldAt: number
}

function createBands(count: number): BandState[] {
    return Array.from({ length: count }, () => ({ peak: 0, peakHeldAt: 0, value: 0 }))
}

/** Resample the 24 mel bands onto `count` bars with linear interpolation. */
function sampleMel(mel: Float32Array, count: number, index: number): number {
    if (mel.length === 0) return 0
    const position = (index / Math.max(count - 1, 1)) * (mel.length - 1)
    const low = Math.floor(position)
    const high = Math.min(low + 1, mel.length - 1)
    const t = position - low
    return clamp01((mel[low] ?? 0) * (1 - t) + (mel[high] ?? 0) * t)
}

function chromaHue(chromagram: Float32Array): number {
    let best = 0
    let bestValue = 0
    for (let index = 0; index < chromagram.length; index += 1) {
        const value = chromagram[index] ?? 0
        if (value > bestValue) {
            bestValue = value
            best = index
        }
    }
    return PITCH_HUES[best % PITCH_HUES.length] ?? 280
}

function hsl(hue: number, saturation: number, lightness: number): string {
    return `hsl(${Math.round(hue)}, ${Math.round(saturation * 100)}%, ${Math.round(lightness * 100)}%)`
}

/** Alpha for both color families this face emits — withAlpha is hex-only. */
function fade(color: string, alpha: number): string {
    if (color.startsWith('hsl(')) {
        return `hsla(${color.slice(4, -1)}, ${alpha})`
    }
    return withAlpha(color, alpha)
}

export default face(
    'Spectrum',
    {
        accent: color('Low Color', palette.neonCyan, { group: 'Style' }),
        barCount: num('Bars', [12, 48], 28, { group: 'Layout', step: 1 }),
        colorMode: combo('Color Mode', ['gradient', 'chromagram'], { group: 'Style' }),
        glow: num('Glow', [0, 1], 0.55, { group: 'Style' }),
        peakColor: color('Peak Color', palette.electricYellow, { group: 'Style' }),
        secondaryAccent: color('High Color', palette.electricPurple, { group: 'Style' }),
    },
    {
        audio: true,
        author: 'Hypercolor',
        description: 'Live mel-band spectrum with peak-hold caps. Breathes when the room goes quiet.',
        designBasis: { height: 480, width: 480 },
        presets: [
            {
                controls: {
                    accent: palette.neonCyan,
                    colorMode: 'gradient',
                    peakColor: palette.electricYellow,
                    secondaryAccent: palette.electricPurple,
                },
                description: 'Cyan-to-purple intensity sweep with golden caps.',
                name: 'SilkCircuit',
            },
            {
                controls: {
                    accent: '#1b8c5a',
                    colorMode: 'gradient',
                    glow: 0.8,
                    peakColor: '#d8ffe9',
                    secondaryAccent: '#50fa7b',
                },
                description: 'VU-meter greens with hot white peaks.',
                name: 'Console',
            },
            {
                controls: { colorMode: 'chromagram', glow: 0.7 },
                description: 'Bars take the hue of the dominant pitch class.',
                name: 'Harmonics',
            },
        ],
        variants: {
            wide: (ctx: FaceContext) => buildSpectrum(ctx, true),
        },
    },
    (ctx) => buildSpectrum(ctx, false),
)

function buildSpectrum(ctx: FaceContext, wide: boolean) {
    createFaceRoot(ctx, 'hc-spectrum')
    let bands: BandState[] = []
    const levelGlide = new Smoothed(0, 0.25)
    const idleBlend = new Smoothed(0, 0.6)
    let lastTime = Number.NaN
    let loudAt = 0

    const advanceBands = (mel: Float32Array, count: number, time: number, dt: number): BandState[] => {
        if (bands.length !== count) bands = createBands(count)
        const decay = 1 - 0.5 ** (dt / BAR_DECAY_HALFLIFE)
        for (let index = 0; index < count; index += 1) {
            const band = bands[index]
            if (!band) continue
            const target = sampleMel(mel, count, index)
            // Rise instantly, fall on a half-life: transients land hard,
            // release stays liquid at face frame rates.
            band.value = target > band.value ? target : band.value + (target - band.value) * decay
            if (band.value >= band.peak) {
                band.peak = band.value
                band.peakHeldAt = time
            } else if (time - band.peakHeldAt > PEAK_HOLD_SECS) {
                band.peak = Math.max(band.value, band.peak - PEAK_FALL_PER_SEC * dt)
            }
        }
        return bands
    }

    const barColor = (
        value: number,
        index: number,
        count: number,
        controls: Record<string, unknown>,
        chromagram: Float32Array,
    ): string => {
        if (controls.colorMode === 'chromagram') {
            const hue = chromaHue(chromagram)
            const spread = (index / Math.max(count - 1, 1) - 0.5) * 40
            return hsl(hue + spread, 0.85, 0.5 + value * 0.22)
        }
        return lerpColor(controls.accent as string, controls.secondaryAccent as string, value)
    }

    return (
        time: number,
        controls: Record<string, unknown>,
        _sensors: import('@hypercolor/sdk').SensorAccessor,
        audio: import('@hypercolor/sdk').AudioAccessor,
    ) => {
        const dt = Number.isNaN(lastTime) ? 1 / 30 : Math.max(time - lastTime, 0)
        lastTime = time
        const audioData = audio.data()
        const level = levelGlide.update(clamp01(audioData.level), dt)
        if (audioData.level > SILENCE_LEVEL) loudAt = time
        const idle = idleBlend.update(time - loudAt > SILENCE_AFTER_SECS ? 1 : 0, dt)
        const count = Math.max(12, Math.round(controls.barCount as number))
        const state = advanceBands(audioData.melBandsNormalized, count, time, dt)
        const glow = clamp01(controls.glow as number)
        const peakColor = controls.peakColor as string

        const c = ctx.ctx
        c.clearRect(0, 0, ctx.width, ctx.height)
        drawNebulaField(
            c,
            ctx.width,
            ctx.height,
            time,
            controls.accent as string,
            controls.secondaryAccent as string,
            0.4 + level * 0.7,
        )

        // Idle breath: a slow wave rolls across the bars so silence still moves.
        const idleValue = (index: number): number =>
            0.08 + 0.07 * (1 + Math.sin(time * 1.1 + (index / count) * Math.PI * 3)) * 0.5

        if (wide) {
            const margin = ctx.width * 0.02
            const baseline = ctx.height * 0.92
            const maxBarHeight = ctx.height * 0.8
            const slot = (ctx.width - margin * 2) / count
            const barWidth = slot * 0.62

            for (let index = 0; index < count; index += 1) {
                const band = state[index]
                if (!band) continue
                const display = band.value * (1 - idle) + idleValue(index) * idle
                const barHeight = Math.max(2, display * maxBarHeight)
                const x = margin + index * slot + (slot - barWidth) / 2
                const fill = barColor(display, index, count, controls, audioData.chromagram)
                c.save()
                if (glow > 0 && idle < 0.5) {
                    c.shadowColor = fill
                    c.shadowBlur = 14 * glow * display
                }
                c.fillStyle = idle > 0.5 ? fade(fill, 0.55) : fill
                c.fillRect(x, baseline - barHeight, barWidth, barHeight)
                c.restore()

                const peakDisplay = band.peak * (1 - idle)
                if (peakDisplay > 0.02) {
                    const peakY = baseline - peakDisplay * maxBarHeight
                    c.fillStyle = withAlpha(peakColor, 0.9)
                    c.fillRect(x, peakY - 2, barWidth, 2)
                }
            }

            // Beat-reactive baseline.
            const pulse = clamp01(audioData.beatPulse) * (1 - idle)
            c.fillStyle = withAlpha(peakColor, 0.25 + 0.55 * pulse)
            c.fillRect(margin, baseline + 2, ctx.width - margin * 2, 2 + pulse * 2)
            return
        }

        // Round: radial spectrum around a breathing center ring.
        const cx = ctx.width / 2
        const cy = ctx.height / 2
        const innerRadius = Math.min(ctx.width, ctx.height) * (0.17 + 0.02 * Math.sin(time * 1.3) * idle)
        const maxLength = Math.min(ctx.width, ctx.height) * 0.46 - innerRadius

        for (let index = 0; index < count; index += 1) {
            const band = state[index]
            if (!band) continue
            const display = band.value * (1 - idle) + idleValue(index) * idle
            const angle = (index / count) * Math.PI * 2 - Math.PI / 2
            const length = Math.max(2, display * maxLength)
            const x0 = cx + Math.cos(angle) * innerRadius
            const y0 = cy + Math.sin(angle) * innerRadius
            const x1 = cx + Math.cos(angle) * (innerRadius + length)
            const y1 = cy + Math.sin(angle) * (innerRadius + length)
            const fill = barColor(display, index, count, controls, audioData.chromagram)

            c.save()
            if (glow > 0 && idle < 0.5) {
                c.shadowColor = fill
                c.shadowBlur = 12 * glow * display
            }
            c.strokeStyle = idle > 0.5 ? fade(fill, 0.55) : fill
            c.lineWidth = Math.max(2, ((Math.PI * 2 * innerRadius) / count) * 0.5)
            c.lineCap = 'round'
            c.beginPath()
            c.moveTo(x0, y0)
            c.lineTo(x1, y1)
            c.stroke()
            c.restore()

            const peakDisplay = band.peak * (1 - idle)
            if (peakDisplay > 0.02) {
                const peakRadius = innerRadius + peakDisplay * maxLength
                const px = cx + Math.cos(angle) * peakRadius
                const py = cy + Math.sin(angle) * peakRadius
                c.fillStyle = withAlpha(peakColor, 0.85)
                c.beginPath()
                c.arc(px, py, 1.6, 0, Math.PI * 2)
                c.fill()
            }
        }

        // Center ring: level-driven when loud, slow breath when idle.
        const ringStrength = level * (1 - idle) + (0.25 + 0.12 * Math.sin(time * 1.3)) * idle
        const ringColor = barColor(ringStrength, 0, 1, controls, audioData.chromagram)
        c.save()
        c.strokeStyle = fade(ringColor, 0.5 + 0.4 * ringStrength)
        c.lineWidth = 2 + ringStrength * 4
        if (glow > 0) {
            c.shadowColor = ringColor
            c.shadowBlur = 18 * glow * ringStrength
        }
        c.beginPath()
        c.arc(cx, cy, innerRadius - 8, 0, Math.PI * 2)
        c.stroke()
        c.restore()
    }
}
