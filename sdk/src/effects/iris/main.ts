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
    'Abyss Bloom',
    'Circuit Jade',
    'Orchid Signal',
    'Ruby Current',
] as const

const controls = {
    // Color
    colorScheme: combo('Colors', COLOR_SCHEMES, {
        default: 'Harmonic',
        tooltip: 'Color scheme (Harmonic uses Circle of Fifths)',
        group: 'Color',
    }),
    glowIntensity: num('Halo', [0, 100], 28, {
        tooltip: 'Outer halo brightness and spread',
        group: 'Color',
    }),
    colorAccent: num('Color Split', [0, 100], 88, {
        tooltip: 'Separates hues and keeps highlights colored instead of white',
        group: 'Color',
    }),

    // Motion
    timeSpeed: num('Time Warp', [0, 100], 52, {
        tooltip: 'Base animation speed and temporal warp',
        group: 'Motion',
    }),
    flowDrive: num('Flow', [0, 100], 58, {
        tooltip: 'Forward rush and radial twisting',
        group: 'Motion',
    }),
    rotationSpeed: num('Rotation', [0, 100], 14, {
        tooltip: 'Spin and orbital motion',
        group: 'Motion',
    }),

    // Audio
    beatFlash: num('Beat Flash', [0, 100], 28, {
        tooltip: 'How violently beats flare, pulse, and explode',
        group: 'Audio',
    }),
    wanderSpeed: num('Drift', [0, 100], 24, {
        tooltip: 'Audio-driven camera drift and pull',
        group: 'Audio',
    }),

    // Pattern
    scale: num('Zoom', [20, 200], 74, {
        tooltip: 'Zoom the tunnel in and out',
        group: 'Pattern',
    }),
    irisStrength: num('Ripple Density', [0, 100], 62, {
        tooltip: 'How many concentric iris bands appear',
        group: 'Pattern',
    }),
    corePulse: num('Center Beam', [0, 100], 46, {
        tooltip: 'Thickness and pulse of the bright center spine',
        group: 'Pattern',
    }),
    bandSharpness: num('Band Sharpness', [0, 100], 58, {
        tooltip: 'Soft folds at low values, crisp contour bands at high values',
        group: 'Pattern',
    }),
    particleDensity: num('Particle Fabric', [0, 100], 44, {
        tooltip: 'Amount of particle grain woven through the geometry',
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
            bandSharpness: lerp(0.18, 4.2, bandFactor ** 0.78),
            beatFlash: beatFlashFactor,
            colorAccent: lerp(0.25, 2.5, accentFactor ** 0.9),
            corePulse: lerp(0.15, 4.6, coreFactor ** 0.92),
            flowDrive: lerp(0.0, 4.0, flowFactor ** 0.85),
            glowIntensity: lerp(0.0, 1.15, glowFactor ** 1.25),
            irisStrength: lerp(0.18, 4.8, irisFactor ** 0.9),
            rotationSpeed: lerp(0.0, 3.6, rotationFactor ** 1.1),
            scale: lerp(1.0, 9.4, scaleFactor ** 0.9),
            texture: lerp(0.0, 1.2, textureFactor ** 0.85),
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
        const colorAudioAccent = 0.72 + lvl * 0.12 + Math.abs(audio.chordMood) * 0.1 + state.smoothBrightness * 0.08
        const textureAudioDensity = 0.45 + onset * 0.55 * bf + fluxTreble * 0.35 + state.smoothBrightness * 0.12

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
        const targetGlow = 0.28 + lvl * 0.28 + onset * 0.3 * bf + state.smoothBrightness * 0.18
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
                bandSharpness: 28,
                beatFlash: 12,
                colorAccent: 78,
                colorScheme: 'Lava',
                corePulse: 68,
                flowDrive: 36,
                glowIntensity: 34,
                irisStrength: 46,
                particleDensity: 18,
                rotationSpeed: 4,
                scale: 126,
                timeSpeed: 28,
                wanderSpeed: 12,
            },
            description: 'Broad ember folds and a slow cathedral spine for low-end ritual tracks.',
            name: 'Cathedral Ember',
        },
        {
            controls: {
                bandSharpness: 62,
                beatFlash: 24,
                colorAccent: 94,
                colorScheme: 'Harmonic',
                corePulse: 44,
                flowDrive: 42,
                glowIntensity: 24,
                irisStrength: 64,
                particleDensity: 34,
                rotationSpeed: 18,
                scale: 78,
                timeSpeed: 46,
                wanderSpeed: 28,
            },
            description:
                'The musical mode preset: stained-glass fifths with clear color separation and restrained bloom.',
            name: 'Fifths In Glass',
        },
        {
            controls: {
                bandSharpness: 74,
                beatFlash: 48,
                colorAccent: 86,
                colorScheme: 'Orchid Signal',
                corePulse: 34,
                flowDrive: 58,
                glowIntensity: 38,
                irisStrength: 70,
                particleDensity: 78,
                rotationSpeed: 36,
                scale: 68,
                timeSpeed: 62,
                wanderSpeed: 42,
            },
            description: 'Orchid, fuchsia, and ice-cyan bands for glossy synths and faster treble detail.',
            name: 'Orchid Relay',
        },
        {
            controls: {
                bandSharpness: 20,
                beatFlash: 4,
                colorAccent: 58,
                colorScheme: 'Phosphor',
                corePulse: 28,
                flowDrive: 22,
                glowIntensity: 48,
                irisStrength: 38,
                particleDensity: 24,
                rotationSpeed: 2,
                scale: 154,
                timeSpeed: 18,
                wanderSpeed: 18,
            },
            description: 'Soft phosphor scanlines and breathing rings for the calmest side of Iris.',
            name: 'Phosphor Dream',
        },
        {
            controls: {
                bandSharpness: 48,
                beatFlash: 18,
                colorAccent: 84,
                colorScheme: 'Abyss Bloom',
                corePulse: 36,
                flowDrive: 64,
                glowIntensity: 30,
                irisStrength: 56,
                particleDensity: 42,
                rotationSpeed: 12,
                scale: 96,
                timeSpeed: 38,
                wanderSpeed: 34,
            },
            description: 'Deep indigo water, electric azure edges, and a soft jade drift through the tunnel.',
            name: 'Pelagic Bloom',
        },
        {
            controls: {
                bandSharpness: 82,
                beatFlash: 88,
                colorAccent: 90,
                colorScheme: 'Solar Storm',
                corePulse: 72,
                flowDrive: 86,
                glowIntensity: 44,
                irisStrength: 86,
                particleDensity: 74,
                rotationSpeed: 58,
                scale: 52,
                timeSpeed: 82,
                wanderSpeed: 62,
            },
            description: 'The festival preset: brass heat, blue counter-light, and hard pulses on every drop.',
            name: 'Solar Choir',
        },
        {
            controls: {
                bandSharpness: 18,
                beatFlash: 0,
                colorAccent: 66,
                colorScheme: 'Ice',
                corePulse: 22,
                flowDrive: 14,
                glowIntensity: 56,
                irisStrength: 34,
                particleDensity: 10,
                rotationSpeed: 0,
                scale: 168,
                timeSpeed: 16,
                wanderSpeed: 10,
            },
            description: 'Glacial, sparse, and nearly still, with a cold halo and minimal rhythmic intrusion.',
            name: 'Glacier Hymnal',
        },
        {
            controls: {
                bandSharpness: 86,
                beatFlash: 32,
                colorAccent: 86,
                colorScheme: 'Circuit Jade',
                corePulse: 32,
                flowDrive: 52,
                glowIntensity: 22,
                irisStrength: 66,
                particleDensity: 64,
                rotationSpeed: 24,
                scale: 72,
                timeSpeed: 58,
                wanderSpeed: 26,
            },
            description: 'Sharper contour bands and emerald-cyan circuitry for precise, technical rhythms.',
            name: 'Jade Lattice',
        },
        {
            controls: {
                bandSharpness: 68,
                beatFlash: 36,
                colorAccent: 92,
                colorScheme: 'Ruby Current',
                corePulse: 52,
                flowDrive: 44,
                glowIntensity: 26,
                irisStrength: 58,
                particleDensity: 30,
                rotationSpeed: 20,
                scale: 84,
                timeSpeed: 54,
                wanderSpeed: 18,
            },
            description: 'Ruby pressure with cobalt relief: warm, dramatic, and still clearly split into color bands.',
            name: 'Ruby Meridian',
        },
        {
            controls: {
                bandSharpness: 92,
                beatFlash: 62,
                colorAccent: 82,
                colorScheme: 'Neon Flux',
                corePulse: 38,
                flowDrive: 68,
                glowIntensity: 18,
                irisStrength: 94,
                particleDensity: 96,
                rotationSpeed: 88,
                scale: 28,
                timeSpeed: 96,
                wanderSpeed: 78,
            },
            description: 'The maximal preset: fast spin, dense particle fabric, and razor neon edges.',
            name: 'Collider Bloom',
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
