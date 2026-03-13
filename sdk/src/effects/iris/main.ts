/**
 * Iris - Geometric Audio Visualizer
 *
 * Mobius circle inversions, spiral dot patterns, and audio-reactive
 * geometric waves with Circle of Fifths harmonic color mapping.
 */

import { combo, effect, num } from '@hypercolor/sdk'
import shader from './fragment.glsl'

// ── Helpers ─────────────────────────────────────────────────────

function lerp(a: number, b: number, t: number): number {
    return a + (b - a) * t
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
    const factor = 1 - Math.exp(-lambda * Math.max(dt, 0))
    return current + (target - current) * factor
}

/** Normalize a raw 0-100 slider to 0-1 range. */
function pct(value: unknown, fallback = 0.5): number {
    const v = typeof value === 'number' && !Number.isNaN(value) ? value : fallback * 100
    return Math.max(0, Math.min(1, v / 100))
}

/** Normalize a raw slider with custom range to 0-1. */
function norm(value: unknown, min: number, max: number, fallback: number): number {
    const v = typeof value === 'number' && !Number.isNaN(value) ? value : fallback
    return Math.max(0, Math.min(1, (v - min) / Math.max(0.00001, max - min)))
}

// ── State ───────────────────────────────────────────────────────

const state = {
    anticipation: 0,
    audioTime: 0,
    boostBand: 1,
    boostCore: 1,
    boostFlow: 1,
    // Smoothed per-uniform audio boosts
    boostIris: 1,
    coreEnergy: 0.8,
    displacementAngle: 0,
    flowVelocity: 0,
    glowEnergy: 0.7,
    irisEnergy: 0.85,
    radialFlow: 0,
    smoothBass: 0,
    smoothBrightness: 0,
    smoothLevel: 0,
    smoothMid: 0,
    smoothMomentum: 0,
    smoothMouseX: 0,
    smoothMouseY: 0,
    // Smoothed audio feature envelopes (prevent spasm)
    smoothOnset: 0,
    smoothRotation: 0,
    smoothSwell: 0,
    smoothTreble: 0,
    smoothZoom: 1,
    subBassEnergy: 0,
    timeWarpSmooth: 1,
}

let lastTime = 0

// ── Controls ────────────────────────────────────────────────────

const COLOR_SCHEMES = [
    'Aurora',
    'Cyberpunk',
    'Gold & Blue',
    'Harmonic',
    'Ice',
    'Lava',
    'Midnight Flux',
    'Neon Flux',
    'Phosphor',
    'Solar Storm',
    'Synesthesia',
    'Vaporwave',
] as const

const controls = {
    // Color
    colorScheme: combo('Colors', COLOR_SCHEMES, {
        default: 'Harmonic',
        tooltip: 'Color scheme (Harmonic uses Circle of Fifths)',
        group: 'Color',
    }),
    glowIntensity: num('Glow', [0, 100], 70, {
        tooltip: 'Core bloom and radiance',
        group: 'Color',
    }),
    colorAccent: num('Accent', [0, 100], 65, {
        tooltip: 'Saturation and tonal punch',
        group: 'Color',
    }),

    // Motion
    timeSpeed: num('Time Warp', [0, 100], 50, {
        tooltip: 'Base animation speed and temporal warp',
        group: 'Motion',
    }),
    flowDrive: num('Flow', [0, 100], 50, {
        tooltip: 'Forward rush and radial twisting',
        group: 'Motion',
    }),
    rotationSpeed: num('Rotation', [0, 100], 0, {
        tooltip: 'Spin and orbital motion',
        group: 'Motion',
    }),

    // Audio
    beatFlash: num('Beat Flash', [0, 100], 45, {
        tooltip: 'How violently beats flare, pulse, and explode',
        group: 'Audio',
    }),
    wanderSpeed: num('Drift', [0, 100], 35, {
        tooltip: 'Audio-driven camera drift and pull',
        group: 'Audio',
    }),

    // Pattern
    scale: num('Scale', [20, 200], 80, {
        tooltip: 'Macro zoom and spatial density',
        group: 'Pattern',
    }),
    irisStrength: num('Iris', [0, 100], 65, {
        tooltip: 'Ripple count and iris aggression',
        group: 'Pattern',
    }),
    corePulse: num('Core', [0, 100], 60, {
        tooltip: 'Center beam width and vascular energy',
        group: 'Pattern',
    }),
    bandSharpness: num('Geometry', [0, 100], 50, {
        tooltip: 'From soft folds to razor bands',
        group: 'Pattern',
    }),
    particleDensity: num('Texture', [0, 100], 55, {
        tooltip: 'Particle fabric and glitch texture strength',
        group: 'Pattern',
    }),
}

// ── Effect ──────────────────────────────────────────────────────

export default effect('Iris', shader, controls, {
    audio: true,
    description:
        'Sacred geometry dances to the beat — Mobius inversions warp harmonic shapes as spectral flux drives color, form, and motion',

    frame: (ctx, time) => {
        const dt = Math.min(lastTime > 0 ? time - lastTime : 0.016, 0.05)
        lastTime = time

        const audio = ctx.audio
        if (!audio) return

        const raw = ctx.controls

        // ── Normalize controls (raw 0-100 → internal ranges) ────────
        const scaleFactor = norm(raw.scale, 20, 200, 80)
        const glowFactor = pct(raw.glowIntensity, 0.7)
        const accentFactor = pct(raw.colorAccent, 0.65)
        const timeSpeedFactor = pct(raw.timeSpeed, 0.5)
        const rotationFactor = pct(raw.rotationSpeed, 0)
        const flowFactor = pct(raw.flowDrive, 0.5)
        const beatFlashFactor = pct(raw.beatFlash, 0.45)
        const wanderFactor = pct(raw.wanderSpeed, 0.35)
        const irisFactor = pct(raw.irisStrength, 0.65)
        const coreFactor = pct(raw.corePulse, 0.6)
        const bandFactor = pct(raw.bandSharpness, 0.5)
        const textureFactor = pct(raw.particleDensity, 0.55)

        // Map normalized factors to shader-domain values
        const c = {
            bandSharpness: lerp(0.25, 3.4, bandFactor ** 0.85),
            beatFlash: beatFlashFactor,
            colorAccent: lerp(0.45, 2.15, accentFactor ** 0.8),
            corePulse: lerp(0.15, 4.2, coreFactor ** 0.9),
            flowDrive: lerp(0.0, 4.0, flowFactor ** 0.85),
            glowIntensity: lerp(0.04, 1.35, glowFactor ** 0.9),
            irisStrength: lerp(0.2, 5.4, irisFactor ** 0.85),
            rotationSpeed: lerp(0.0, 3.6, rotationFactor ** 1.1),
            scale: lerp(1.25, 8.5, scaleFactor ** 0.85),
            texture: lerp(0.0, 1.0, textureFactor ** 0.9),
            timeSpeed: lerp(0.18, 3.4, timeSpeedFactor ** 0.95),
            wanderSpeed: lerp(0.0, 3.2, wanderFactor ** 0.85),
        }
        const beatFlashGain = lerp(0.0, 1.65, c.beatFlash ** 0.75)

        // ── Smooth audio features first (prevent spasm) ─────────────
        // Everything flows through smoothed envelopes — no raw impulses
        const rawOnset = Math.max(audio.onsetPulse, audio.beatPulse * 0.6)
        state.smoothOnset = smoothAsymmetric(state.smoothOnset, rawOnset, 8, 2.5, dt)
        state.smoothLevel = smoothAsymmetric(state.smoothLevel, audio.levelShort, 5, 1.8, dt)
        state.smoothBass = smoothAsymmetric(state.smoothBass, audio.bassEnv, 6, 1.5, dt)
        state.smoothMid = smoothAsymmetric(state.smoothMid, audio.midEnv, 7, 2, dt)
        state.smoothTreble = smoothAsymmetric(state.smoothTreble, audio.trebleEnv, 8, 2.5, dt)
        state.smoothMomentum = smoothApproach(state.smoothMomentum, audio.momentum, 1.5, dt)
        state.smoothSwell = smoothApproach(state.smoothSwell, audio.swell, 2, dt)
        state.smoothBrightness = smoothApproach(state.smoothBrightness, audio.brightness, 2, dt)

        const onset = state.smoothOnset
        const lvl = state.smoothLevel
        const bass = state.smoothBass
        const mid = state.smoothMid
        const treb = state.smoothTreble
        const mom = state.smoothMomentum
        const swell = state.smoothSwell

        // ── Anticipation ─────────────────────────────────────────────
        const anticipation = audio.beatPhase > 0.7 ? ((audio.beatPhase - 0.7) / 0.3) * audio.beatConfidence : 0
        state.anticipation = smoothApproach(state.anticipation, Math.max(0, anticipation), 3, dt)

        // ── Spectral flux bands (smoothed) ──────────────────────────
        const fluxBass = audio.spectralFluxBands[0]
        const fluxMid = audio.spectralFluxBands[1]
        const fluxTreble = audio.spectralFluxBands[2]

        // ── Audio boosts — smoothed separately so elements breathe ──
        // Each visual element responds to different bands at different rates
        const bf = beatFlashGain
        const targetBoostIris = 0.78 + mid * 0.7 + fluxMid * 0.4 + onset * 0.6 * bf
        const targetBoostCore = 0.7 + bass * 0.85 + swell * 0.35 + fluxBass * 0.3 + onset * 0.45 * bf
        const targetBoostFlow = 0.55 + mom * 0.9 + lvl * 0.45 + swell * 0.5 + onset * 0.35 * bf
        const targetBoostBand = 0.65 + onset * 0.95 * bf + audio.roughness * 0.3 + fluxTreble * 0.2

        state.boostIris = smoothAsymmetric(state.boostIris, targetBoostIris, 6, 2.2, dt)
        state.boostCore = smoothAsymmetric(state.boostCore, targetBoostCore, 5, 1.7, dt)
        state.boostFlow = smoothAsymmetric(state.boostFlow, targetBoostFlow, 4, 1.3, dt)
        state.boostBand = smoothAsymmetric(state.boostBand, targetBoostBand, 7, 2.3, dt)

        const flowBeatMod = state.boostFlow * (0.6 + onset * 0.8 * bf)
        const colorAudioAccent = 0.85 + lvl * 0.25 + Math.abs(audio.chordMood) * 0.25 + state.smoothBrightness * 0.2
        const textureAudioDensity = 0.65 + onset * 0.9 * bf + fluxTreble * 0.55 + state.smoothBrightness * 0.25

        // ── Time warp — smooth, momentum-driven with gentle beat swell ─
        const targetTimeWarp = 0.55 + lvl * 0.6 + mom * 0.5 + onset * 0.55 * bf + swell * 0.35
        state.timeWarpSmooth = smoothAsymmetric(state.timeWarpSmooth, targetTimeWarp, 5, 1.7, dt)
        const timeWarp = c.timeSpeed * (0.65 + mom * 0.25 + onset * 0.35 * bf + state.smoothBrightness * 0.15)
        state.audioTime += dt * timeWarp * state.timeWarpSmooth

        // ── Radial flow ("flying through") ──────────────────────────
        // Momentum and swell drive sustained flow, bass gives surge
        const baseFlowSpeed = c.flowDrive * 0.5
        const flowTarget = baseFlowSpeed * (0.65 + bass * 0.55 + mom * 0.8 + swell * 0.55 + onset * 0.65 * bf)

        state.flowVelocity = smoothAsymmetric(state.flowVelocity, flowTarget, 6, 1.8, dt)
        state.radialFlow += state.flowVelocity * dt

        // ── Continuous rotation — momentum-driven, not beat-jerked ──
        const spinAudio = mom * 0.55 + lvl * 0.15 + onset * 0.25 * bf
        const rotationSpeed = c.rotationSpeed * (0.4 + spinAudio)
        state.smoothRotation += rotationSpeed * dt

        // ── Zoom — gentle swell, not beat explosion ─────────────────
        const anticipationZoom = 1.0 - state.anticipation * (0.04 + bf * 0.04)
        const zoomSwell = onset * 0.18 + onset * 0.32 * bf + swell * 0.18 + lvl * 0.12
        const targetZoom = anticipationZoom + zoomSwell

        state.smoothZoom = smoothAsymmetric(state.smoothZoom, targetZoom, 6, 2.5, dt)

        // ── Energy envelopes — different rates per band ─────────────
        // Glow: brightness-driven, slow and warm (onset scaled by beat flash)
        const targetGlow = 0.55 + lvl * 0.45 + onset * 0.55 * bf + state.smoothBrightness * 0.35
        // Core: bass-driven, slow heave
        const targetCore = 0.65 + bass * 0.75 + swell * 0.3 + fluxBass * 0.3 + onset * 0.25 * bf
        // Iris: mid-driven, medium pace
        const targetIris = 0.7 + mid * 0.6 + fluxMid * 0.4 + onset * 0.25 * bf

        state.glowEnergy = smoothAsymmetric(state.glowEnergy, targetGlow, 5, 1.8, dt)
        state.coreEnergy = smoothAsymmetric(state.coreEnergy, targetCore, 4, 1.5, dt)
        state.irisEnergy = smoothAsymmetric(state.irisEnergy, targetIris, 5, 2, dt)

        // ── Sub-bass displacement — slow, tidal ─────────────────────
        const subBassTarget = bass * 0.65 + fluxBass * 0.55 + onset * 0.25 * bf
        state.subBassEnergy = smoothAsymmetric(state.subBassEnergy, subBassTarget, 5, 1.2, dt)
        state.displacementAngle += dt * 1.8 + mom * dt * 2.0

        // ── Wander system — lazy drift, not twitchy ─────────────────
        const wanderRate = 0.08 + c.wanderSpeed * 0.48 + mom * 0.08
        const wanderAmplitude = 0.04 + c.wanderSpeed * 0.72
        const wanderTime = state.audioTime * wanderRate
        const pathX = smoothNoise(wanderTime, 0) * wanderAmplitude
        const pathY = smoothNoise(wanderTime, 123.45) * wanderAmplitude

        // Audio pulls — smoothed band energies, not raw impulses
        const bassBlend = bass * 0.5 + fluxBass * 0.2
        const trebleBlend = treb * 0.5 + fluxTreble * 0.2

        const wanderNormalized = Math.min(1, c.wanderSpeed / 3.2)
        const audioWanderScale = 0.18 + wanderNormalized * 1.05

        let targetX = pathX + bassBlend * (0.15 + c.wanderSpeed * 0.82) * audioWanderScale
        let targetY = pathY + trebleBlend * (0.12 + c.wanderSpeed * 0.75) * audioWanderScale

        // Pull back toward center as wander decreases
        const focusStrength = 0.62 - wanderNormalized * 0.42
        targetX = lerp(targetX, 0, focusStrength)
        targetY = lerp(targetY, 0, focusStrength)

        // Clamp to safe range
        const clampRange = 0.2 + wanderNormalized * 0.95
        const clampedX = Math.max(-clampRange, Math.min(clampRange, targetX))
        const clampedY = Math.max(-clampRange, Math.min(clampRange, targetY))

        // Smooth wander — no onset acceleration, just steady drift
        const wanderResponse = 1.4 + c.wanderSpeed * 1.1
        state.smoothMouseX = smoothApproach(state.smoothMouseX, clampedX, wanderResponse, dt)
        state.smoothMouseY = smoothApproach(state.smoothMouseY, clampedY, wanderResponse, dt)

        // ── Push uniforms ───────────────────────────────────────────

        // Control uniforms (with smoothed audio boosts)
        ctx.setUniform('iScale', c.scale)
        ctx.setUniform('iGlowIntensity', c.glowIntensity)
        ctx.setUniform('iIrisStrength', c.irisStrength * state.boostIris)
        ctx.setUniform('iCorePulse', c.corePulse * state.boostCore)
        ctx.setUniform('iFlowDrive', c.flowDrive * flowBeatMod)
        ctx.setUniform('iColorAccent', c.colorAccent * colorAudioAccent)
        ctx.setUniform('iBandSharpness', c.bandSharpness * state.boostBand)
        ctx.setUniform('iParticleDensity', Math.min(1.25, c.texture * textureAudioDensity))
        ctx.setUniform('iBeatFlashOnset', onset * bf)

        // State uniforms
        ctx.setUniform('iAudioTime', state.audioTime)
        ctx.setUniform('iBeatRotation', state.smoothRotation)
        ctx.setUniform('iBeatZoom', state.smoothZoom)
        ctx.setUniform('iSmoothMouse', [state.smoothMouseX, state.smoothMouseY])
        ctx.setUniform('iRadialFlow', state.radialFlow)
        ctx.setUniform('iFlowVelocity', state.flowVelocity)
        ctx.setUniform('iGlowEnergy', state.glowEnergy)
        ctx.setUniform('iCoreEnergy', state.coreEnergy)
        ctx.setUniform('iIrisEnergy', state.irisEnergy)
        ctx.setUniform('iBeatAnticipation', state.anticipation)

        // Sub-bass displacement vector
        const displaceStrength = state.subBassEnergy * 0.012
        const displaceX = Math.cos(state.displacementAngle) * displaceStrength
        const displaceY = Math.sin(state.displacementAngle) * displaceStrength
        ctx.setUniform('iSubBassDisplace', [displaceX, displaceY])
    },

    presets: [
        {
            controls: {
                bandSharpness: 35,
                beatFlash: 18,
                colorAccent: 80,
                colorScheme: 'Lava',
                corePulse: 90,
                flowDrive: 80,
                glowIntensity: 90,
                irisStrength: 85,
                particleDensity: 40,
                rotationSpeed: 8,

                scale: 120,
                timeSpeed: 25,
                wanderSpeed: 18,
            },
            description:
                'Drone metal in a candlelit crypt — massive bass displacement warps concentric rings while lava glow bleeds from the core',
            name: 'Hypnagogic Temple',
        },
        {
            controls: {
                bandSharpness: 65,
                beatFlash: 42,
                colorAccent: 90,
                colorScheme: 'Harmonic',
                corePulse: 50,
                flowDrive: 40,
                glowIntensity: 60,
                irisStrength: 70,
                particleDensity: 50,
                rotationSpeed: 30,

                scale: 70,
                timeSpeed: 45,
                wanderSpeed: 52,
            },
            description:
                'Every note has a color — Circle of Fifths mapping transforms a string quartet into spinning harmonic stained glass',
            name: 'Chromatic Fugue',
        },
        {
            controls: {
                bandSharpness: 85,
                beatFlash: 72,
                colorAccent: 75,
                colorScheme: 'Cyberpunk',
                corePulse: 70,
                flowDrive: 65,
                glowIntensity: 55,
                irisStrength: 80,
                particleDensity: 90,
                rotationSpeed: 55,

                scale: 55,
                timeSpeed: 75,
                wanderSpeed: 74,
            },
            description:
                'IDM at 3am in a server room — cyberpunk iris geometry spasms with glitch texture as fractured beats rearrange the grid',
            name: 'Midnight Mainframe',
        },
        {
            controls: {
                bandSharpness: 25,
                beatFlash: 6,
                colorAccent: 50,
                colorScheme: 'Phosphor',
                corePulse: 35,
                flowDrive: 30,
                glowIntensity: 85,
                irisStrength: 45,
                particleDensity: 30,
                rotationSpeed: 5,

                scale: 150,
                timeSpeed: 20,
                wanderSpeed: 22,
            },
            description:
                'Ambient pads dissolve into bioluminescent geometry — soft green phosphor rings expand like breath, zero flash, pure presence',
            name: 'Phosphor Meditation',
        },
        {
            controls: {
                bandSharpness: 75,
                beatFlash: 95,
                colorAccent: 95,
                colorScheme: 'Solar Storm',
                corePulse: 85,
                flowDrive: 90,
                glowIntensity: 95,
                irisStrength: 95,
                particleDensity: 80,
                rotationSpeed: 70,

                scale: 45,
                timeSpeed: 85,
                wanderSpeed: 82,
            },
            description:
                'Stadium EDM climax — iris geometry detonates on every drop, golden plasma jets compete with ice-blue shockwaves at maximum warp',
            name: 'Solar Storm Apex',
        },
        {
            controls: {
                bandSharpness: 15,
                beatFlash: 0,
                colorAccent: 40,
                colorScheme: 'Ice',
                corePulse: 20,
                flowDrive: 15,
                glowIntensity: 100,
                irisStrength: 30,
                particleDensity: 10,
                rotationSpeed: 0,

                scale: 200,
                timeSpeed: 10,
                wanderSpeed: 8,
            },
            description:
                'Frozen cathedral at the edge of the world — a single ice mandala breathes in glacial silence, all glow, no violence',
            name: 'Permafrost Halo',
        },
        {
            controls: {
                bandSharpness: 100,
                beatFlash: 60,
                colorAccent: 70,
                colorScheme: 'Neon Flux',
                corePulse: 45,
                flowDrive: 55,
                glowIntensity: 40,
                irisStrength: 100,
                particleDensity: 100,
                rotationSpeed: 100,

                scale: 20,
                timeSpeed: 100,
                wanderSpeed: 100,
            },
            description:
                'Feed a drum machine into a particle accelerator — neon iris blades spin at impossible speed while glitch fabric tears reality apart',
            name: 'Centrifuge Protocol',
        },
    ],

    setup: (ctx) => {
        ctx.registerUniform('iAudioTime', 0)
        ctx.registerUniform('iBeatRotation', 0)
        ctx.registerUniform('iBeatZoom', 1)
        ctx.registerUniform('iSmoothMouse', [0, 0])
        ctx.registerUniform('iRadialFlow', 0)
        ctx.registerUniform('iFlowVelocity', 0)
        ctx.registerUniform('iGlowEnergy', 0.7)
        ctx.registerUniform('iCoreEnergy', 0.8)
        ctx.registerUniform('iIrisEnergy', 0.85)
        ctx.registerUniform('iSubBassDisplace', [0, 0])
        ctx.registerUniform('iBeatAnticipation', 0)
        ctx.registerUniform('iBeatFlashOnset', 0)
    },
})
