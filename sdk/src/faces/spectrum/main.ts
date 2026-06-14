import type { FaceContext } from '@hypercolor/sdk'
import { color, combo, face, num, palette, Smoothed, withAlpha } from '@hypercolor/sdk'
import { drawNebulaField } from '../shared/atmosphere'
import { clamp01, createFaceRoot } from '../shared/dom'

const FAST_DECAY_HALFLIFE = 0.14
const GHOST_DECAY_HALFLIFE = 0.85
const SILENCE_LEVEL = 0.015
const SILENCE_AFTER_SECS = 2
const CURVE_POINTS = 48
const SPARK_BANDS = 12
const PITCH_HUES = [330, 0, 30, 55, 80, 120, 160, 190, 215, 250, 275, 300]

interface SpectrumState {
    /** Fast-attack, liquid-release silhouette. */
    live: number[]
    /** Slow ghost silhouette trailing behind the live one. */
    ghost: number[]
}

function createState(): SpectrumState {
    return {
        ghost: new Array<number>(CURVE_POINTS).fill(0),
        live: new Array<number>(CURVE_POINTS).fill(0),
    }
}

/** Resample the 24 mel bands onto the curve with linear interpolation. */
function sampleMel(mel: Float32Array, index: number): number {
    if (mel.length === 0) return 0
    const position = (index / Math.max(CURVE_POINTS - 1, 1)) * (mel.length - 1)
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

function hsl(hue: number, saturation: number, lightness: number, alpha = 1): string {
    return `hsla(${Math.round(hue)}, ${Math.round(saturation * 100)}%, ${Math.round(lightness * 100)}%, ${alpha})`
}

export default face(
    'Spectrum',
    {
        accent: color('Low Color', palette.neonCyan, { group: 'Style' }),
        barCount: num('Detail', [12, 48], 32, { group: 'Layout', step: 1 }),
        colorMode: combo('Color Mode', ['gradient', 'chromagram'], { group: 'Style' }),
        glow: num('Glow', [0, 1], 0.6, { group: 'Style' }),
        peakColor: color('Ridge Color', '#ffffff', { group: 'Style' }),
        secondaryAccent: color('High Color', palette.electricPurple, { group: 'Style' }),
    },
    {
        audio: true,
        author: 'Hypercolor',
        description:
            'The music as terrain: a liquid spectral silhouette with a glowing ridge, ghost afterimage, and beat-driven bloom.',
        designBasis: { height: 480, width: 480 },
        presets: [
            {
                controls: {
                    accent: palette.neonCyan,
                    colorMode: 'gradient',
                    peakColor: '#ffffff',
                    secondaryAccent: palette.electricPurple,
                },
                description: 'Cyan-to-violet terrain with a white-hot ridge.',
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
                description: 'Emerald landscape with a pale mint ridge.',
                name: 'Console',
            },
            {
                controls: { colorMode: 'chromagram', glow: 0.75 },
                description: 'The whole terrain tints with the dominant pitch.',
                name: 'Harmonics',
            },
            {
                controls: {
                    accent: '#ff5e7a',
                    colorMode: 'gradient',
                    glow: 0.9,
                    peakColor: '#ffd9a8',
                    secondaryAccent: '#ffb347',
                },
                description: 'Sunset ridge for warm sets.',
                name: 'Ember Ridge',
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
    const state = createState()
    const levelGlide = new Smoothed(0, 0.25)
    const idleBlend = new Smoothed(0, 0.6)
    const bloom = new Smoothed(0, 0.18)
    let lastTime = Number.NaN
    let loudAt = 0

    const advance = (mel: Float32Array, dt: number): void => {
        const fastDecay = 1 - 0.5 ** (dt / FAST_DECAY_HALFLIFE)
        const ghostDecay = 1 - 0.5 ** (dt / GHOST_DECAY_HALFLIFE)
        for (let index = 0; index < CURVE_POINTS; index += 1) {
            const target = sampleMel(mel, index)
            const live = state.live[index] ?? 0
            // Instant attack, liquid release.
            state.live[index] = target > live ? target : live + (target - live) * fastDecay
            const ghost = state.ghost[index] ?? 0
            const liveNow = state.live[index] ?? 0
            state.ghost[index] =
                liveNow > ghost ? ghost + (liveNow - ghost) * ghostDecay * 2 : ghost + (liveNow - ghost) * ghostDecay
        }
    }

    /** Smooth closed silhouette path through the value points. */
    const traceSilhouette = (
        c: CanvasRenderingContext2D,
        values: number[],
        points: number,
        left: number,
        right: number,
        baseY: number,
        amplitude: number,
        idle: number,
        time: number,
    ): void => {
        const span = right - left
        const point = (index: number): [number, number] => {
            const fraction = index / (points - 1)
            const sample = Math.round(fraction * (CURVE_POINTS - 1))
            const breath = 0.06 + 0.05 * (1 + Math.sin(time * 1.1 + fraction * Math.PI * 3)) * 0.5
            const value = (values[sample] ?? 0) * (1 - idle) + breath * idle
            return [left + fraction * span, baseY - value * amplitude]
        }
        c.moveTo(left, baseY)
        let [prevX, prevY] = point(0)
        c.lineTo(prevX, prevY)
        for (let index = 1; index < points; index += 1) {
            const [x, y] = point(index)
            const midX = (prevX + x) / 2
            const midY = (prevY + y) / 2
            c.quadraticCurveTo(prevX, prevY, midX, midY)
            prevX = x
            prevY = y
        }
        c.lineTo(prevX, prevY)
        c.lineTo(right, baseY)
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
        const detail = Math.max(12, Math.min(CURVE_POINTS, Math.round(controls.barCount as number)))
        const level = levelGlide.update(clamp01(audioData.level), dt)
        if (audioData.level > SILENCE_LEVEL) loudAt = time
        const idle = idleBlend.update(time - loudAt > SILENCE_AFTER_SECS ? 1 : 0, dt)
        const pulse = bloom.update(clamp01(audioData.beatPulse), dt)
        advance(audioData.melBandsNormalized, dt)

        const glow = clamp01(controls.glow as number)
        const chromaMode = controls.colorMode === 'chromagram'
        const hue = chromaHue(audioData.chromagram)
        const lowColor = chromaMode ? hsl(hue - 24, 0.8, 0.42) : (controls.accent as string)
        const highColor = chromaMode ? hsl(hue + 24, 0.85, 0.6) : (controls.secondaryAccent as string)
        const ridgeColor = controls.peakColor as string

        const c = ctx.ctx
        const W = ctx.width
        const H = ctx.height
        c.clearRect(0, 0, W, H)
        drawNebulaField(c, W, H, time, lowColor, highColor, 0.45 + level * 0.8 + pulse * 0.4)

        const safe = ctx.display.safeArea
        const left = wide ? W * 0.025 : safe.x
        const right = wide ? W * 0.975 : safe.x + safe.width
        const baseY = wide ? H * 0.86 : H * 0.72
        const amplitude = wide ? H * 0.72 : safe.height * 0.52
        const alive = 1 - idle * 0.45

        // Ghost silhouette: the recent past as a faint afterimage.
        c.beginPath()
        traceSilhouette(c, state.ghost, detail, left, right, baseY, amplitude * 1.02, idle, time)
        c.closePath()
        c.fillStyle = withAlpha(highColor, 0.1 * alive)
        c.fill()

        // Live silhouette with a frequency-graded fill.
        c.beginPath()
        traceSilhouette(c, state.live, detail, left, right, baseY, amplitude, idle, time)
        c.closePath()
        const fill = c.createLinearGradient(0, baseY - amplitude, 0, baseY)
        fill.addColorStop(0, withAlpha(highColor, (0.5 + pulse * 0.3) * alive))
        fill.addColorStop(0.65, withAlpha(lowColor, 0.3 * alive))
        fill.addColorStop(1, withAlpha(lowColor, 0.04))
        c.fillStyle = fill
        c.fill()

        // Glowing ridge line along the live silhouette.
        c.save()
        c.beginPath()
        traceSilhouette(c, state.live, detail, left, right, baseY, amplitude, idle, time)
        if (glow > 0) {
            c.shadowColor = ridgeColor
            c.shadowBlur = (10 + pulse * 14) * glow
        }
        c.strokeStyle = withAlpha(ridgeColor, (0.55 + pulse * 0.35) * alive)
        c.lineWidth = 1.6
        c.stroke()
        c.restore()

        // Mirrored reflection below the baseline.
        c.save()
        c.translate(0, baseY * 2)
        c.scale(1, -1)
        c.beginPath()
        traceSilhouette(c, state.live, detail, left, right, baseY, amplitude * 0.32, idle, time)
        c.closePath()
        const mirror = c.createLinearGradient(0, baseY - amplitude * 0.32, 0, baseY)
        mirror.addColorStop(0, withAlpha(lowColor, 0.12 * alive))
        mirror.addColorStop(1, withAlpha(lowColor, 0))
        c.fillStyle = mirror
        c.fill()
        c.restore()

        // Spectral sparks lifting off the strongest bands.
        for (let index = 0; index < SPARK_BANDS; index += 1) {
            const sampleIndex = Math.floor((index / SPARK_BANDS) * CURVE_POINTS)
            const value = (state.live[sampleIndex] ?? 0) * (1 - idle)
            if (value < 0.3) continue
            const fraction = sampleIndex / (CURVE_POINTS - 1)
            const sparkPhase = (time * (0.5 + value * 0.9) + index * 0.37) % 1
            const x = left + fraction * (right - left)
            const y = baseY - value * amplitude - sparkPhase * amplitude * 0.3
            const fade = (1 - sparkPhase) * value
            c.fillStyle = withAlpha(ridgeColor, fade * 0.5)
            c.beginPath()
            c.arc(x, y, 1.4, 0, Math.PI * 2)
            c.fill()
        }

        // Baseline hairline that blooms on the beat.
        c.strokeStyle = withAlpha(ridgeColor, (0.18 + pulse * 0.5) * alive)
        c.lineWidth = 1 + pulse * 1.6
        c.beginPath()
        c.moveTo(left, baseY)
        c.lineTo(right, baseY)
        c.stroke()
    }
}
