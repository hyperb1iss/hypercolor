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

function springStep(
    position: number,
    velocity: number,
    target: number,
    stiffness: number,
    damping: number,
    dt: number,
    impulse = 0,
): [number, number] {
    const step = Math.max(dt, 0)
    const nextVelocity =
        (velocity + ((target - position) * stiffness + impulse) * step) * Math.exp(-Math.max(damping, 0) * step)
    const nextPosition = position + nextVelocity * step
    return [nextPosition, nextVelocity]
}

function clampMotion(position: number, velocity: number, min: number, max: number): [number, number] {
    if (position < min) return [min, Math.max(0, velocity)]
    if (position > max) return [max, Math.min(0, velocity)]
    return [position, velocity]
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
    anticipation: 0,
    cameraEnergy: 0,
    drift: 0,
    driftVelocity: 0,
    impact: 0,
    motionPulse: 0,
    panVelocityX: 0,
    panVelocityY: 0,
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
    twistVelocity: 0,
    warpPhase: 0,
    warpVelocity: 0,
    zoom: 1,
    zoomVelocity: 0,
}

let lastTime = 0

export default effect(
    'Cymatics',
    shader,
    {
        visualStyle: combo('Style', ['Lattice', 'Particle Field', 'Vortex', 'Resonance'], {
            default: 'Particle Field',
            tooltip: 'Visualization mode — each renders sound as a different spatial phenomenon',
            group: 'Scene',
        }),
        colorScheme: combo('Colors', ['Aurora', 'Cyberpunk', 'Lava', 'Prism', 'Toxic', 'Vaporwave'], {
            default: 'Cyberpunk',
            tooltip: 'Color palette with three-phase cycling for richer hue diversity',
            group: 'Color',
        }),
        colorSpeed: num('Color Speed', [0, 200], 50, {
            normalize: 'none',
            tooltip: 'Color cycling speed',
            group: 'Color',
        }),
        glowIntensity: num('Glow', [10, 200], 100, {
            normalize: 'none',
            tooltip: 'Overall brightness',
            group: 'Color',
        }),
        flow: num('Flow', [-100, 100], 30, {
            normalize: 'none',
            tooltip: 'Travel direction and speed',
            group: 'Motion',
        }),
        sensitivity: num('Sensitivity', [10, 200], 50, {
            normalize: 'none',
            tooltip: 'Audio reactivity',
            group: 'Audio',
        }),
    },
    {
        audio: true,
        description: 'Sound made visible — cinematic audio visualizer with spring-physics camera, frequency-reactive geometry, and motion-led response',

        frame: (ctx, time) => {
            const dt = Math.min(lastTime > 0 ? time - lastTime : 0.016, 0.05)
            lastTime = time

            const audio = ctx.audio
            const flow = clamp(readNumber(ctx.controls.flow, 30) / 100, -1, 1)
            const fluxBass = audio?.spectralFluxBands[0] ?? 0
            const fluxMid = audio?.spectralFluxBands[1] ?? 0

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
            const fluxTarget = audio
                ? Math.max((audio.spectralFlux ?? 0) * 0.62, fluxBass * 0.48, fluxMid * 0.4)
                : fallbackPulse * 0.75
            const anticipationTarget = audio
                ? audio.beatPhase > 0.62
                    ? ((audio.beatPhase - 0.62) / 0.38) * (0.15 + (audio.beatConfidence ?? 0) * 0.85)
                    : 0
                : (0.5 + 0.5 * Math.sin(time * 0.82)) * 0.12

            state.smoothLevel = smoothAsymmetric(state.smoothLevel, levelTarget, 5, 1.7, dt)
            state.smoothBass = smoothAsymmetric(state.smoothBass, bassTarget, 6, 1.9, dt)
            state.smoothMid = smoothAsymmetric(state.smoothMid, midTarget, 7, 2.2, dt)
            state.smoothTreble = smoothAsymmetric(state.smoothTreble, trebleTarget, 8, 2.6, dt)
            state.smoothOnset = smoothAsymmetric(state.smoothOnset, onsetTarget, 8, 2.1, dt)
            state.smoothMomentum = smoothApproach(state.smoothMomentum, momentumTarget, 1.6, dt)
            state.smoothSwell = smoothApproach(state.smoothSwell, swellTarget, 2.2, dt)
            state.anticipation = smoothApproach(state.anticipation, anticipationTarget, 3.4, dt)

            const cameraEnergyTarget = clamp(
                state.smoothBass * 0.55 +
                    state.smoothSwell * 0.38 +
                    state.smoothLevel * 0.22 +
                    Math.abs(state.smoothMomentum) * 0.18 +
                    fluxTarget * 0.12,
                0,
                1,
            )
            state.cameraEnergy = smoothAsymmetric(state.cameraEnergy, cameraEnergyTarget, 4.6, 1.1, dt)

            const impactTarget = clamp(
                state.smoothOnset * 0.95 + fluxTarget * 0.55 + state.smoothBass * 0.22 + state.anticipation * 0.18,
                0,
                1,
            )
            state.impact = smoothAsymmetric(state.impact, impactTarget, 10, 2.6, dt)

            const motionPulseTarget = clamp(
                state.impact * (0.72 + state.cameraEnergy * 0.28) + state.smoothSwell * 0.12,
                0,
                1,
            )
            state.motionPulse = smoothAsymmetric(state.motionPulse, motionPulseTarget, 11, 2.8, dt)

            const wanderRate = 0.20 + Math.abs(flow) * 0.28 + state.cameraEnergy * 0.22 + state.motionPulse * 0.14
            const wanderAmplitude = 0.22 + state.cameraEnergy * 0.38 + state.motionPulse * 0.28
            const wanderTime = time * wanderRate + state.drift * 0.12
            const driftX = smoothNoise(wanderTime * 0.82, 1.3)
            const driftY = smoothNoise(wanderTime * 0.67, 8.7)

            // Bass and treble pull the camera in opposing directions
            const bassPull =
                (state.smoothBass - 0.12) * (0.52 + state.cameraEnergy * 0.32) + state.smoothMomentum * 0.28
            const treblePull =
                (state.smoothTreble - 0.10) * (0.38 + state.cameraEnergy * 0.22) - state.smoothMomentum * 0.14
            const lateralLead = flow * (0.08 + state.motionPulse * 0.22 + state.cameraEnergy * 0.14)
            const verticalLift = state.anticipation * (0.08 + state.cameraEnergy * 0.06) - state.smoothSwell * 0.14

            const targetPanX = clamp(driftX * wanderAmplitude + bassPull + lateralLead, -0.92, 0.92)
            const targetPanY = clamp(driftY * (wanderAmplitude * 0.92) + treblePull - verticalLift, -0.72, 0.72)

            // Lower damping during drops for dramatic overshoot
            const panStiffness = 16 + state.cameraEnergy * 20 + state.motionPulse * 18
            const panDamping = 5.2 - state.cameraEnergy * 1.2 - state.motionPulse * 0.8
            const panKickX =
                flow * state.motionPulse * 5.5 +
                state.smoothMomentum * (1.4 + state.cameraEnergy * 0.8) +
                state.smoothOnset * 2.8 * (driftX > 0 ? 1 : -1)
            const panKickY =
                (state.smoothTreble - state.smoothBass) * (2.0 + state.motionPulse * 1.4) -
                state.anticipation * 2.8 +
                state.smoothOnset * 1.8 * (driftY > 0 ? 1 : -1)

            ;[state.panX, state.panVelocityX] = springStep(
                state.panX,
                state.panVelocityX,
                targetPanX,
                panStiffness,
                panDamping,
                dt,
                panKickX,
            )
            ;[state.panY, state.panVelocityY] = springStep(
                state.panY,
                state.panVelocityY,
                targetPanY,
                panStiffness * 0.9,
                panDamping + 0.15,
                dt,
                panKickY,
            )
            ;[state.panX, state.panVelocityX] = clampMotion(state.panX, state.panVelocityX, -0.95, 0.95)
            ;[state.panY, state.panVelocityY] = clampMotion(state.panY, state.panVelocityY, -0.75, 0.75)

            // Zoom — wider range, drops punch in hard, quiet pulls way out
            const targetZoom = clamp(
                0.88 -
                    state.anticipation * 0.14 +
                    state.cameraEnergy * 0.32 +
                    state.motionPulse * 0.52 +
                    state.smoothBass * 0.16,
                0.68,
                1.72,
            )
            ;[state.zoom, state.zoomVelocity] = springStep(
                state.zoom,
                state.zoomVelocity,
                targetZoom,
                14 + state.cameraEnergy * 14 + state.motionPulse * 18,
                4.8 - state.motionPulse * 1.2,
                dt,
                state.motionPulse * 6.5 - state.anticipation * 3.2 + state.smoothOnset * 4.0,
            )
            ;[state.zoom, state.zoomVelocity] = clampMotion(state.zoom, state.zoomVelocity, 0.65, 1.78)

            // Twist — more aggressive rotation during drops
            const twistVelocityTarget =
                flow * (0.28 + state.cameraEnergy * 0.24) +
                state.smoothMomentum * 0.62 +
                (state.smoothTreble - state.smoothBass) * 0.22 +
                state.motionPulse * 0.72 +
                state.smoothOnset * 0.45
            state.twistVelocity = smoothAsymmetric(state.twistVelocity, twistVelocityTarget, 6.5, 2.2, dt)
            state.twist += state.twistVelocity * dt

            const travelDirection = Math.abs(flow) > 0.01 ? Math.sign(flow) : Math.sign(state.smoothMomentum) || 1
            const driftVelocityTarget =
                travelDirection * (0.62 + state.cameraEnergy * 1.1 + state.smoothSwell * 0.48) +
                flow * 0.52 +
                state.smoothMomentum * 0.62 +
                state.motionPulse * 0.85 * travelDirection +
                state.smoothOnset * 0.6 * travelDirection
            state.driftVelocity = smoothAsymmetric(state.driftVelocity, driftVelocityTarget, 5.8, 1.7, dt)
            state.drift += state.driftVelocity * dt

            const warpVelocityTarget =
                0.32 +
                state.smoothMid * 0.68 +
                state.smoothTreble * 0.32 +
                state.cameraEnergy * 0.48 +
                state.motionPulse * 0.72 +
                state.smoothOnset * 0.35
            state.warpVelocity = smoothAsymmetric(state.warpVelocity, warpVelocityTarget, 6.4, 2.2, dt)
            state.warpPhase += state.warpVelocity * dt

            ctx.setUniform('iAudioLevelSmooth', state.smoothLevel)
            ctx.setUniform('iAudioBassSmooth', state.smoothBass)
            ctx.setUniform('iAudioMidSmooth', state.smoothMid)
            ctx.setUniform('iAudioTrebleSmooth', state.smoothTreble)
            ctx.setUniform('iMotionEnergy', state.cameraEnergy)
            ctx.setUniform('iMotionPulse', state.motionPulse)
            ctx.setUniform('iMotionPan', [state.panX, state.panY])
            ctx.setUniform('iMotionZoom', state.zoom)
            ctx.setUniform('iMotionTwist', state.twist)
            ctx.setUniform('iFlowDrift', state.drift)
            ctx.setUniform('iWarpPhase', state.warpPhase)
        },

        presets: [
            {
                controls: {
                    colorScheme: 'Lava',
                    colorSpeed: 25,
                    flow: -80,
                    glowIntensity: 180,
                    sensitivity: 180,
                    visualStyle: 'Vortex',
                },
                description:
                    'Sub-bass becomes architecture — slow vortex pull in a concrete bunker where the kick drum is a seismic event',
                name: 'Warehouse Ritual',
            },
            {
                controls: {
                    colorScheme: 'Cyberpunk',
                    colorSpeed: 120,
                    flow: 75,
                    glowIntensity: 130,
                    sensitivity: 90,
                    visualStyle: 'Resonance',
                },
                description:
                    'Eight frequency ribbons streaming through neon rain — bass swells roll the low bands while treble shimmer crackles across the top',
                name: 'Neon Meridian',
            },
            {
                controls: {
                    colorScheme: 'Prism',
                    colorSpeed: 15,
                    flow: 10,
                    glowIntensity: 85,
                    sensitivity: 35,
                    visualStyle: 'Particle Field',
                },
                description:
                    'Ambient drone through stained glass — prismatic particles breathe with glacial patience, each harmonic a new color',
                name: 'Glass Cathedral',
            },
            {
                controls: {
                    colorScheme: 'Toxic',
                    colorSpeed: 160,
                    flow: -45,
                    glowIntensity: 150,
                    sensitivity: 145,
                    visualStyle: 'Lattice',
                },
                description:
                    'Industrial lattice pulses under acid-green data streams — percussive hits tear holes in the matrix',
                name: 'Toxic Mainframe',
            },
            {
                controls: {
                    colorScheme: 'Aurora',
                    colorSpeed: 40,
                    flow: 20,
                    glowIntensity: 70,
                    sensitivity: 55,
                    visualStyle: 'Particle Field',
                },
                description:
                    'Jazz club at closing time — warm aurora ripples respond to brushed cymbals and upright bass with velvet restraint',
                name: 'Aurora Lounge',
            },
            {
                controls: {
                    colorScheme: 'Vaporwave',
                    colorSpeed: 65,
                    flow: 40,
                    glowIntensity: 110,
                    sensitivity: 70,
                    visualStyle: 'Resonance',
                },
                description:
                    'Frequency ribbons unfurl like magnetic tape in zero gravity — each band a living seismograph of its octave, bass ripples propagating through pastel shimmer',
                name: 'Chladni Plate',
            },
        ],

        setup: (ctx) => {
            ctx.registerUniform('iAudioLevelSmooth', 0)
            ctx.registerUniform('iAudioBassSmooth', 0)
            ctx.registerUniform('iAudioMidSmooth', 0)
            ctx.registerUniform('iAudioTrebleSmooth', 0)
            ctx.registerUniform('iMotionEnergy', 0)
            ctx.registerUniform('iMotionPulse', 0)
            ctx.registerUniform('iMotionPan', [0, 0])
            ctx.registerUniform('iMotionZoom', 1)
            ctx.registerUniform('iMotionTwist', 0)
            ctx.registerUniform('iFlowDrift', 0)
            ctx.registerUniform('iWarpPhase', 0)
        },
    },
)
