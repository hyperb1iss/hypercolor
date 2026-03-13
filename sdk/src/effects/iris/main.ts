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

function decay(value: number, lambda: number, dt: number): number {
    if (!Number.isFinite(lambda) || lambda <= 0) return value
    return value * Math.exp(-lambda * Math.max(dt, 0))
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
    audioTime: 0,
    smoothMouseX: 0,
    smoothMouseY: 0,
    smoothRotation: 0,
    smoothZoom: 1,
    beatAccum: 0,
    anticipation: 0,
    harmonicHueSmooth: 0,
    radialFlow: 0,
    flowVelocity: 0,
    glowEnergy: 0.7,
    coreEnergy: 0.8,
    irisEnergy: 0.85,
    subBassEnergy: 0,
    displacementAngle: 0,
    // Smoothed audio feature envelopes (prevent spasm)
    smoothOnset: 0,
    smoothLevel: 0,
    smoothBass: 0,
    smoothMid: 0,
    smoothTreble: 0,
    smoothMomentum: 0,
    smoothSwell: 0,
    smoothBrightness: 0,
    // Smoothed per-uniform audio boosts
    boostIris: 1,
    boostCore: 1,
    boostFlow: 1,
    boostBand: 1,
    timeWarpSmooth: 1,
}

let lastTime = 0

// ── Controls ────────────────────────────────────────────────────

const COLOR_SCHEMES = ['Aurora', 'Cyberpunk', 'Gold & Blue', 'Harmonic', 'Ice', 'Lava', 'Midnight Flux', 'Neon Flux', 'Phosphor', 'Solar Storm', 'Synesthesia', 'Vaporwave'] as const

const controls = {
    // Style
    colorScheme: combo('Colors', COLOR_SCHEMES, {
        default: 'Harmonic',
        tooltip: 'Color scheme (Harmonic uses Circle of Fifths)',
    }),
    harmonicColor: num('Harmonic Mix', [0, 100], 50, {
        tooltip: 'Blend harmonic colors with base palette',
    }),

    // Animation
    timeSpeed: num('Time Speed', [0, 100], 50, {
        tooltip: 'Control animation speed',
    }),
    rotationSpeed: num('Rotation', [0, 100], 0, {
        tooltip: 'Pattern rotation speed',
    }),
    flowDrive: num('Flow', [0, 100], 50, {
        tooltip: 'Continuous outward flow strength',
    }),

    // Audio
    wanderSpeed: num('Wander', [0, 100], 30, {
        tooltip: 'View wandering with audio',
    }),
    timeSensitivity: num('Time Warp', [0, 100], 50, {
        tooltip: 'Audio influence on speed',
    }),
    bassPull: num('Bass Pull', [0, 100], 60, {
        tooltip: 'Bass influence on movement',
    }),
    treblePull: num('Treble Pull', [0, 100], 60, {
        tooltip: 'Treble influence on movement',
    }),

    // Pattern
    scale: num('Scale', [20, 200], 80, {
        tooltip: 'Zoom level',
    }),
    irisStrength: num('Iris', [0, 100], 65, {
        tooltip: 'Iris/radial pattern strength',
    }),
    corePulse: num('Core Pulse', [0, 100], 60, {
        tooltip: 'Energy in the center column',
    }),
    bandSharpness: num('Bands', [0, 100], 50, {
        tooltip: 'Sharpen or soften band edges',
    }),

    // Color
    glowIntensity: num('Glow', [0, 100], 70, {
        tooltip: 'Center glow intensity',
    }),
    colorAccent: num('Accent', [0, 100], 65, {
        tooltip: 'Boost palette saturation',
    }),
    colorContrast: num('Contrast', [0, 100], 60, {
        tooltip: 'Control overall contrast curve',
    }),

    // Texture
    particleDensity: num('Texture Strength', [0, 100], 60, {
        tooltip: 'Glitch texture intensity',
    }),
    particleSize: num('Texture Scale', [0, 100], 50, {
        tooltip: 'Texture size',
    }),
    particleColorMix: num('Texture Hue', [0, 100], 50, {
        tooltip: 'Texture color mixing',
    }),
}

// ── Effect ──────────────────────────────────────────────────────

export default effect('Iris', shader, controls, {
    description:
        'Geometric audio visualizer with Mobius inversions, harmonic color mapping, and spectral flux beat detection',
    audio: true,

    presets: [
        {
            name: 'Hypnagogic Temple',
            description: 'Drone metal in a candlelit crypt — massive bass displacement warps concentric rings while lava glow bleeds from the core',
            controls: {
                colorScheme: 'Lava',
                harmonicColor: 20,
                timeSpeed: 25,
                rotationSpeed: 8,
                flowDrive: 80,
                wanderSpeed: 15,
                timeSensitivity: 70,

                bassPull: 95,
                treblePull: 20,
                scale: 120,
                irisStrength: 85,
                corePulse: 90,
                bandSharpness: 35,
                glowIntensity: 90,
                colorAccent: 80,
                colorContrast: 75,
                particleDensity: 40,
                particleSize: 70,
                particleColorMix: 25,
            },
        },
        {
            name: 'Chromatic Fugue',
            description: 'Every note has a color — Circle of Fifths mapping transforms a string quartet into spinning harmonic stained glass',
            controls: {
                colorScheme: 'Harmonic',
                harmonicColor: 95,
                timeSpeed: 45,
                rotationSpeed: 30,
                flowDrive: 40,
                wanderSpeed: 50,
                timeSensitivity: 60,

                bassPull: 45,
                treblePull: 80,
                scale: 70,
                irisStrength: 70,
                corePulse: 50,
                bandSharpness: 65,
                glowIntensity: 60,
                colorAccent: 90,
                colorContrast: 55,
                particleDensity: 50,
                particleSize: 40,
                particleColorMix: 85,
            },
        },
        {
            name: 'Midnight Mainframe',
            description: 'IDM at 3am in a server room — cyberpunk iris geometry spasms with glitch texture as fractured beats rearrange the grid',
            controls: {
                colorScheme: 'Cyberpunk',
                harmonicColor: 30,
                timeSpeed: 75,
                rotationSpeed: 55,
                flowDrive: 65,
                wanderSpeed: 70,
                timeSensitivity: 85,

                bassPull: 50,
                treblePull: 75,
                scale: 55,
                irisStrength: 80,
                corePulse: 70,
                bandSharpness: 85,
                glowIntensity: 55,
                colorAccent: 75,
                colorContrast: 80,
                particleDensity: 90,
                particleSize: 35,
                particleColorMix: 60,
            },
        },
        {
            name: 'Phosphor Meditation',
            description: 'Ambient pads dissolve into bioluminescent geometry — soft green phosphor rings expand like breath, zero flash, pure presence',
            controls: {
                colorScheme: 'Phosphor',
                harmonicColor: 40,
                timeSpeed: 20,
                rotationSpeed: 5,
                flowDrive: 30,
                wanderSpeed: 20,
                timeSensitivity: 25,

                bassPull: 30,
                treblePull: 40,
                scale: 150,
                irisStrength: 45,
                corePulse: 35,
                bandSharpness: 25,
                glowIntensity: 85,
                colorAccent: 50,
                colorContrast: 40,
                particleDensity: 30,
                particleSize: 65,
                particleColorMix: 35,
            },
        },
        {
            name: 'Solar Storm Apex',
            description: 'Stadium EDM climax — iris geometry detonates on every drop, golden plasma jets compete with ice-blue shockwaves at maximum warp',
            controls: {
                colorScheme: 'Solar Storm',
                harmonicColor: 60,
                timeSpeed: 85,
                rotationSpeed: 70,
                flowDrive: 90,
                wanderSpeed: 80,
                timeSensitivity: 95,

                bassPull: 85,
                treblePull: 70,
                scale: 45,
                irisStrength: 95,
                corePulse: 85,
                bandSharpness: 75,
                glowIntensity: 95,
                colorAccent: 95,
                colorContrast: 85,
                particleDensity: 80,
                particleSize: 55,
                particleColorMix: 70,
            },
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

    frame: (ctx, time) => {
        const dt = Math.min(lastTime > 0 ? time - lastTime : 0.016, 0.05)
        lastTime = time

        const audio = ctx.audio
        if (!audio) return

        const raw = ctx.controls

        // ── Normalize controls (raw 0-100 → internal ranges) ────────
        const scaleFactor = norm(raw.scale, 40, 200, 160)
        const wanderFactor = pct(raw.wanderSpeed, 0.3)
        const timeFactor = pct(raw.timeSensitivity, 0.5)
        const bassFactor = pct(raw.bassPull, 0.6)
        const trebleFactor = pct(raw.treblePull, 0.6)
        const glowFactor = pct(raw.glowIntensity, 0.7)
        const rotationFactor = pct(raw.rotationSpeed, 0)
        const irisFactor = pct(raw.irisStrength, 0.65)
        const coreFactor = pct(raw.corePulse, 0.6)
        const flowFactor = pct(raw.flowDrive, 0.5)
        const accentFactor = pct(raw.colorAccent, 0.65)
        const contrastFactor = pct(raw.colorContrast, 0.6)
        const bandFactor = pct(raw.bandSharpness, 0.5)
        const particleDensityFactor = pct(raw.particleDensity, 0.6)
        const particleSizeFactor = pct(raw.particleSize, 0.5)
        const particleColorFactor = pct(raw.particleColorMix, 0.5)
        const timeSpeedFactor = pct(raw.timeSpeed, 0.5)
        const beatFlashFactor = pct(raw.beatFlash, 0.2)
        const harmonicFactor = pct(raw.harmonicColor, 0.5)

        // Map normalized factors to shader-domain values
        const c = {
            scale: lerp(2.0, 5.0, scaleFactor ** 0.7),
            wanderSpeed: lerp(0.15, 2.2, (0.08 + wanderFactor * 0.92) ** 0.9),
            timeSensitivity: lerp(0.35, 2.8, timeFactor ** 0.9),
            bassPull: lerp(0.0, 2.4, bassFactor ** 1.1),
            treblePull: lerp(0.0, 2.0, trebleFactor ** 1.05),
            glowIntensity: lerp(0.12, 1.2, glowFactor),
            rotationSpeed: lerp(0.0, 2.4, rotationFactor ** 1.2),
            irisStrength: lerp(0.3, 3.2, irisFactor ** 0.85),
            corePulse: lerp(0.2, 2.8, coreFactor ** 0.95),
            flowDrive: lerp(0.2, 2.5, flowFactor ** 0.9),
            colorAccent: lerp(0.6, 1.6, accentFactor ** 0.9),
            colorContrast: lerp(0.7, 2.0, contrastFactor ** 0.8),
            bandSharpness: lerp(0.5, 2.0, bandFactor ** 0.8),
            particleDensity: lerp(0.05, 3.0, particleDensityFactor ** 0.8),
            particleSize: lerp(0.2, 2.0, particleSizeFactor ** 0.8),
            particleColorMix: lerp(0.05, 1.2, particleColorFactor ** 0.9),
            timeSpeed: lerp(0.3, 2.5, timeSpeedFactor ** 0.8),
            harmonicColor: harmonicFactor,
            beatFlash: beatFlashFactor,
        }

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
        const anticipation = audio.beatPhase > 0.7
            ? ((audio.beatPhase - 0.7) / 0.3) * audio.beatConfidence
            : 0
        state.anticipation = smoothApproach(state.anticipation, Math.max(0, anticipation), 3, dt)

        // ── Harmonic hue smoothing (with wraparound) ────────────────
        let hueDiff = audio.harmonicHue - state.harmonicHueSmooth
        if (hueDiff > 180) hueDiff -= 360
        if (hueDiff < -180) hueDiff += 360
        state.harmonicHueSmooth += hueDiff * 0.06

        // ── Spectral flux bands (smoothed) ──────────────────────────
        const fluxBass = audio.spectralFluxBands[0]
        const fluxMid = audio.spectralFluxBands[1]
        const fluxTreble = audio.spectralFluxBands[2]

        // ── Audio boosts — smoothed separately so elements breathe ──
        // Each visual element responds to different bands at different rates
        const bf = c.beatFlash
        const targetBoostIris = 0.92 + mid * 0.4 + onset * 0.25 * bf + fluxMid * 0.2
        const targetBoostCore = 0.88 + bass * 0.45 + onset * 0.2 * bf + swell * 0.2
        const targetBoostFlow = 0.75 + mom * 0.35 + lvl * 0.25 + swell * 0.25
        const targetBoostBand = 0.85 + onset * 0.3 * bf + audio.roughness * 0.15

        state.boostIris = smoothAsymmetric(state.boostIris, targetBoostIris, 5, 2, dt)
        state.boostCore = smoothAsymmetric(state.boostCore, targetBoostCore, 4, 1.5, dt)
        state.boostFlow = smoothAsymmetric(state.boostFlow, targetBoostFlow, 3, 1.2, dt)
        state.boostBand = smoothAsymmetric(state.boostBand, targetBoostBand, 6, 2, dt)

        const flowBeatMod = state.boostFlow * (0.85 + onset * 0.35 * bf)
        const colorAudioAccent = 0.92 + lvl * 0.2 + Math.abs(audio.chordMood) * 0.15
        const colorAudioContrast = 0.92 + mom * 0.15 + state.smoothBrightness * 0.1
        const particleAudioDensity = 0.85 + onset * 0.25 * bf + fluxTreble * 0.2
        const particleAudioSize = 0.92 + lvl * 0.15 + audio.spread * 0.1
        const particleAudioColor = 0.85 + treb * 0.2 + state.smoothBrightness * 0.15

        // ── Time warp — smooth, momentum-driven with gentle beat swell ─
        const targetTimeWarp = 0.6 + lvl * 0.5 + mom * 0.3 + onset * 0.25 + swell * 0.2
        state.timeWarpSmooth = smoothAsymmetric(state.timeWarpSmooth, targetTimeWarp, 4, 1.5, dt)
        const timeWarp = (0.8 + c.timeSensitivity) * c.timeSpeed
        state.audioTime += dt * timeWarp * state.timeWarpSmooth

        // ── Radial flow ("flying through") ──────────────────────────
        // Momentum and swell drive sustained flow, bass gives surge
        const baseFlowSpeed = c.flowDrive * 0.5
        const flowTarget = baseFlowSpeed * (1.0 + bass * 0.4 + mom * 0.5 + swell * 0.4 + onset * 0.5)

        state.flowVelocity = smoothAsymmetric(
            state.flowVelocity, flowTarget,
            6, 1.8, dt,
        )
        state.radialFlow += state.flowVelocity * dt

        // ── Beat accumulation for rotation ──────────────────────────
        state.beatAccum += onset * (0.5 + c.timeSensitivity * 0.06)
        state.beatAccum = Math.max(0, decay(state.beatAccum, 1.8, dt))

        // ── Continuous rotation — momentum-driven, not beat-jerked ──
        const spinAudio = mom * 0.35 + lvl * 0.1
        const rotationSpeed = c.rotationSpeed * (0.4 + spinAudio)
        state.smoothRotation += rotationSpeed * dt

        // ── Zoom — gentle swell, not beat explosion ─────────────────
        const anticipationZoom = 1.0 - state.anticipation * 0.1
        const zoomSwell = onset * 0.3 + swell * 0.12 + lvl * 0.08
        const targetZoom = anticipationZoom + zoomSwell

        state.smoothZoom = smoothAsymmetric(
            state.smoothZoom, targetZoom,
            6, 2.5, dt,
        )

        // ── Energy envelopes — different rates per band ─────────────
        // Glow: brightness-driven, slow and warm (onset scaled by beat flash)
        const targetGlow = 0.7 + lvl * 0.35 + onset * 0.25 * bf + state.smoothBrightness * 0.25
        // Core: bass-driven, slow heave
        const targetCore = 0.8 + bass * 0.45 + swell * 0.2 + fluxBass * 0.2
        // Iris: mid-driven, medium pace
        const targetIris = 0.85 + mid * 0.35 + fluxMid * 0.25

        state.glowEnergy = smoothAsymmetric(state.glowEnergy, targetGlow, 5, 1.8, dt)
        state.coreEnergy = smoothAsymmetric(state.coreEnergy, targetCore, 4, 1.5, dt)
        state.irisEnergy = smoothAsymmetric(state.irisEnergy, targetIris, 5, 2, dt)

        // ── Sub-bass displacement — slow, tidal ─────────────────────
        const subBassTarget = bass * 0.5 + fluxBass * 0.5 + onset * 0.35
        state.subBassEnergy = smoothAsymmetric(
            state.subBassEnergy, subBassTarget,
            5, 1.2, dt,
        )
        state.displacementAngle += dt * 1.8 + mom * dt * 2.0

        // ── Wander system — lazy drift, not twitchy ─────────────────
        const wanderRate = 0.18 + c.wanderSpeed * 0.25 + mom * 0.03
        const wanderAmplitude = 0.2 + c.wanderSpeed * 0.45
        const wanderTime = state.audioTime * wanderRate
        const pathX = smoothNoise(wanderTime, 0) * wanderAmplitude
        const pathY = smoothNoise(wanderTime, 123.45) * wanderAmplitude

        // Audio pulls — smoothed band energies, not raw impulses
        const bassBlend = bass * 0.5 + fluxBass * 0.2
        const trebleBlend = treb * 0.5 + fluxTreble * 0.2

        const wanderNormalized = Math.min(1, c.wanderSpeed / 2.2)
        const audioWanderScale = 0.3 + wanderNormalized * 0.7

        let targetX = pathX + bassBlend * c.bassPull * audioWanderScale
        let targetY = pathY + trebleBlend * c.treblePull * audioWanderScale

        // Pull back toward center as wander decreases
        const focusStrength = 0.25 + (1 - wanderNormalized) * 0.4
        targetX = lerp(targetX, 0, focusStrength)
        targetY = lerp(targetY, 0, focusStrength)

        // Clamp to safe range
        const clampRange = 0.6 + wanderNormalized * 0.35
        const clampedX = Math.max(-clampRange, Math.min(clampRange, targetX))
        const clampedY = Math.max(-clampRange, Math.min(clampRange, targetY))

        // Smooth wander — no onset acceleration, just steady drift
        const wanderResponse = 2.0 + c.wanderSpeed * 0.8
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
        ctx.setUniform('iColorContrast', c.colorContrast * colorAudioContrast)
        ctx.setUniform('iBandSharpness', c.bandSharpness * state.boostBand)
        ctx.setUniform('iParticleDensity', c.particleDensity * particleAudioDensity)
        ctx.setUniform('iParticleSize', c.particleSize * particleAudioSize)
        ctx.setUniform('iParticleColorMix', c.particleColorMix * particleAudioColor)
        ctx.setUniform('iHarmonicColor', c.harmonicColor)
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
})
