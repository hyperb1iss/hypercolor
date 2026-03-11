import { combo, effect, num } from '@hypercolor/sdk'
import shader from './fragment.glsl'

function clamp(value: number, min: number, max: number): number {
    return Math.max(min, Math.min(max, value))
}

function smoothApproach(current: number, target: number, lambda: number, dt: number): number {
    if (!Number.isFinite(lambda) || lambda <= 0) return target
    const factor = 1 - Math.exp(-lambda * Math.max(dt, 0))
    return current + (target - current) * factor
}

function smoothAsymmetric(
    current: number,
    target: number,
    attackLambda: number,
    decayLambda: number,
    dt: number,
): number {
    const lambda = target > current ? attackLambda : decayLambda
    return smoothApproach(current, target, lambda, dt)
}

function hashNoise(x: number, seed: number): number {
    const n = Math.sin(x * 127.1 + seed * 311.7) * 43758.5453
    return (n - Math.floor(n)) * 2.0 - 1.0
}

function smoothNoise(x: number, seed: number): number {
    const i = Math.floor(x)
    const f = x - i
    const smooth = f * f * (3.0 - 2.0 * f)
    return hashNoise(i, seed) * (1.0 - smooth) + hashNoise(i + 1, seed) * smooth
}

function readNumber(value: unknown, fallback: number): number {
    return typeof value === 'number' && Number.isFinite(value) ? value : fallback
}

const state = {
    drift: 0,
    motionPulse: 0,
    panX: 0,
    panY: 0,
    smoothBass: 0,
    smoothLevel: 0,
    smoothMid: 0,
    smoothMomentum: 0,
    smoothOnset: 0,
    smoothSwell: 0,
    smoothTreble: 0,
    twist: 0,
    warpPhase: 0,
    zoom: 1,
}

let lastTime = 0

export default effect(
    'Audio Pulse',
    shader,
    {
        colorScheme: combo('Colors', ['Aurora', 'Cyberpunk', 'Lava', 'Prism', 'Toxic', 'Vaporwave'], {
            default: 'Cyberpunk',
            tooltip: 'Color scheme',
        }),
        colorSpeed: num('Color Speed', [0, 200], 50, {
            normalize: 'none',
            tooltip: 'Color cycling speed',
        }),
        flow: num('Flow', [-100, 100], 30, {
            normalize: 'none',
            tooltip: 'Travel direction and speed',
        }),
        glowIntensity: num('Glow', [10, 200], 100, {
            normalize: 'none',
            tooltip: 'Overall brightness',
        }),
        sensitivity: num('Sensitivity', [10, 200], 50, {
            normalize: 'none',
            tooltip: 'Audio reactivity',
        }),
        visualStyle: combo('Style', ['Grid', 'Pulse Field', 'Vortex', 'Waveform'], {
            default: 'Pulse Field',
            tooltip: 'Visualization style',
        }),
    },
    {
        audio: true,
        description: 'Cinematic audio visualizer with motion-led audio response instead of beat flashes',

        frame: (ctx, time) => {
            const dt = Math.min(lastTime > 0 ? time - lastTime : 0.016, 0.05)
            lastTime = time

            const audio = ctx.audio
            const flow = clamp(readNumber(ctx.controls.flow, 30) / 100, -1, 1)

            const fallbackLevel = 0.06 + (0.5 + 0.5 * Math.sin(time * 0.33)) * 0.05
            const fallbackPulse = Math.max(0, Math.sin(time * 1.15)) ** 6 * 0.16

            const levelTarget = audio ? Math.max(audio.levelShort, audio.level * 0.85) : fallbackLevel
            const bassTarget = audio ? Math.max(audio.bassEnv, audio.bass * 0.9) : fallbackLevel * 0.9
            const midTarget = audio ? Math.max(audio.midEnv, audio.mid * 0.85) : fallbackLevel * 0.72
            const trebleTarget = audio ? Math.max(audio.trebleEnv, audio.treble * 0.8) : fallbackLevel * 0.58
            const onsetTarget = audio
                ? Math.max(audio.onsetPulse * 0.72, audio.beatPulse * 0.48, audio.swell * 0.22)
                : fallbackPulse
            const momentumTarget = audio?.momentum ?? Math.sin(time * 0.18) * 0.12
            const swellTarget = audio?.swell ?? fallbackLevel * 0.75

            state.smoothLevel = smoothAsymmetric(state.smoothLevel, levelTarget, 5, 1.7, dt)
            state.smoothBass = smoothAsymmetric(state.smoothBass, bassTarget, 6, 1.9, dt)
            state.smoothMid = smoothAsymmetric(state.smoothMid, midTarget, 7, 2.2, dt)
            state.smoothTreble = smoothAsymmetric(state.smoothTreble, trebleTarget, 8, 2.6, dt)
            state.smoothOnset = smoothAsymmetric(state.smoothOnset, onsetTarget, 8, 2.1, dt)
            state.smoothMomentum = smoothApproach(state.smoothMomentum, momentumTarget, 1.6, dt)
            state.smoothSwell = smoothApproach(state.smoothSwell, swellTarget, 2.2, dt)

            const motionPulseTarget = clamp(
                state.smoothOnset * 0.8 + state.smoothSwell * 0.25 + state.smoothLevel * 0.1,
                0,
                1,
            )
            state.motionPulse = smoothAsymmetric(state.motionPulse, motionPulseTarget, 6, 1.5, dt)

            const wanderRate = 0.12 + Math.abs(flow) * 0.18 + state.smoothLevel * 0.09
            const wanderTime = time * wanderRate
            const driftX = smoothNoise(wanderTime * 0.82, 1.3)
            const driftY = smoothNoise(wanderTime * 0.67, 8.7)

            const bassPull = (state.smoothBass - 0.2) * 0.22 + state.smoothMomentum * 0.09
            const treblePull = (state.smoothTreble - 0.16) * 0.16 - state.smoothMomentum * 0.05
            const panAmplitude = 0.08 + state.smoothLevel * 0.08 + state.smoothSwell * 0.04

            const targetPanX = clamp(driftX * panAmplitude + bassPull + flow * state.motionPulse * 0.04, -0.32, 0.32)
            const targetPanY = clamp(
                driftY * (panAmplitude * 0.85) + treblePull - state.smoothSwell * 0.05,
                -0.26,
                0.26,
            )

            state.panX = smoothApproach(state.panX, targetPanX, 2.4, dt)
            state.panY = smoothApproach(state.panY, targetPanY, 2.1, dt)

            const targetZoom = clamp(
                1.0 + state.smoothBass * 0.08 + state.smoothSwell * 0.06 + state.motionPulse * 0.04,
                0.94,
                1.18,
            )
            state.zoom = smoothAsymmetric(state.zoom, targetZoom, 4.8, 2.2, dt)

            const twistVelocity =
                flow * (0.14 + state.smoothLevel * 0.08) +
                state.smoothMomentum * 0.18 +
                (state.smoothTreble - state.smoothBass) * 0.05 +
                state.motionPulse * 0.08
            state.twist += twistVelocity * dt

            const driftVelocity =
                flow * (0.42 + state.smoothLevel * 0.36 + state.smoothSwell * 0.22) + state.smoothMomentum * 0.18
            state.drift += driftVelocity * dt

            const warpVelocity = 0.24 + state.smoothMid * 0.36 + state.smoothTreble * 0.14 + state.motionPulse * 0.18
            state.warpPhase += warpVelocity * dt

            ctx.setUniform('iAudioLevelSmooth', state.smoothLevel)
            ctx.setUniform('iAudioBassSmooth', state.smoothBass)
            ctx.setUniform('iAudioMidSmooth', state.smoothMid)
            ctx.setUniform('iAudioTrebleSmooth', state.smoothTreble)
            ctx.setUniform('iMotionPulse', state.motionPulse)
            ctx.setUniform('iMotionPan', [state.panX, state.panY])
            ctx.setUniform('iMotionZoom', state.zoom)
            ctx.setUniform('iMotionTwist', state.twist)
            ctx.setUniform('iFlowDrift', state.drift)
            ctx.setUniform('iWarpPhase', state.warpPhase)
        },

        setup: (ctx) => {
            ctx.registerUniform('iAudioLevelSmooth', 0)
            ctx.registerUniform('iAudioBassSmooth', 0)
            ctx.registerUniform('iAudioMidSmooth', 0)
            ctx.registerUniform('iAudioTrebleSmooth', 0)
            ctx.registerUniform('iMotionPulse', 0)
            ctx.registerUniform('iMotionPan', [0, 0])
            ctx.registerUniform('iMotionZoom', 1)
            ctx.registerUniform('iMotionTwist', 0)
            ctx.registerUniform('iFlowDrift', 0)
            ctx.registerUniform('iWarpPhase', 0)
        },
    },
)
