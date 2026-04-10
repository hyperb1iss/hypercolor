import { combo, effect, num } from '@hypercolor/sdk'
import shader from './fragment.glsl'

function clamp(value: number, min: number, max: number): number {
    return Math.max(min, Math.min(max, value))
}

function mix(a: number, b: number, t: number): number {
    return a + (b - a) * t
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
    return smoothApproach(current, target, target > current ? attackLambda : decayLambda, dt)
}

function readNumber(value: unknown, fallback: number): number {
    return typeof value === 'number' && Number.isFinite(value) ? value : fallback
}

const state = {
    beatBloom: 0,
    floor: 0.12,
    presence: 0,
    smoothBass: 0,
    smoothLevel: 0,
    smoothMid: 0,
    smoothSwell: 0,
    smoothTreble: 0,
}

let lastTime = 0

function resetState(): void {
    state.beatBloom = 0
    state.floor = 0.12
    state.presence = 0
    state.smoothBass = 0
    state.smoothLevel = 0
    state.smoothMid = 0
    state.smoothSwell = 0
    state.smoothTreble = 0
    lastTime = 0
}

export default effect(
    'Frequency Cascade',
    shader,
    {
        scene: combo('Scene', ['Cascade', 'Mirror', 'Horizon', 'Tunnel'], {
            default: 'Cascade',
            group: 'Scene',
        }),
        palette: combo('Palette', ['Aurora', 'Cyberpunk', 'Fire', 'Ice', 'SilkCircuit', 'Sunset'], {
            default: 'Aurora',
            group: 'Color',
        }),
        speed: num('Speed', [1, 10], 5, { group: 'Audio' }),
        intensity: num('Intensity', [0, 100], 80, { group: 'Audio' }),
        smoothing: num('Smoothing', [0, 100], 60, { group: 'Audio' }),
        barWidth: num('Bar Width', [0, 100], 55, { group: 'Geometry' }),
        glow: num('Glow', [0, 100], 45, { group: 'Geometry' }),
    },
    {
        audio: true,
        description:
            'Feed it sound and watch the spectrum breathe — frequency bands rise in poised columns of light across a stable, luminous field',
        setup: (ctx) => {
            resetState()
            ctx.registerUniform('iCascadeLevel', 0)
            ctx.registerUniform('iCascadeBass', 0)
            ctx.registerUniform('iCascadeMid', 0)
            ctx.registerUniform('iCascadeTreble', 0)
            ctx.registerUniform('iCascadeSwell', 0)
            ctx.registerUniform('iCascadePresence', 0)
            ctx.registerUniform('iCascadeBeatBloom', 0)
            ctx.registerUniform('iCascadeFloor', 0.12)
        },
        frame: (ctx, time) => {
            const dt = Math.min(lastTime > 0 ? time - lastTime : 0.016, 0.05)
            lastTime = time

            const audio = ctx.audio
            const smoothing = clamp(readNumber(ctx.controls.smoothing, 60) / 100, 0, 1)
            const intensity = clamp(readNumber(ctx.controls.intensity, 80) / 100, 0, 1)
            const speed = clamp(readNumber(ctx.controls.speed, 5) / 10, 0.1, 1)

            const fallbackLevel = 0.05 + (0.5 + 0.5 * Math.sin(time * (0.24 + speed * 0.24))) * 0.04
            const fallbackBass = fallbackLevel * (0.88 + 0.1 * Math.sin(time * 0.62))
            const fallbackMid = fallbackLevel * (0.72 + 0.08 * Math.cos(time * 0.47))
            const fallbackTreble = fallbackLevel * (0.54 + 0.08 * Math.sin(time * 0.94))
            const fallbackBeat = Math.max(0, Math.sin(time * (0.92 + speed * 0.66))) ** 6 * 0.14

            const beatConfidence = audio?.beatConfidence ?? 0
            const levelTarget = audio ? Math.max(audio.levelShort, audio.level * 0.88) : fallbackLevel
            const bassTarget = audio ? Math.max(audio.bassEnv, audio.bass * 0.9) : fallbackBass
            const midTarget = audio ? Math.max(audio.midEnv, audio.mid * 0.86) : fallbackMid
            const trebleTarget = audio ? Math.max(audio.trebleEnv, audio.treble * 0.82) : fallbackTreble
            const swellTarget = audio ? Math.max(audio.swell, levelTarget * 0.5) : fallbackLevel * 0.72
            const beatTarget = audio
                ? Math.max(
                      audio.onsetPulse * 0.52,
                      audio.beatPulse * (0.22 + beatConfidence * 0.3),
                      audio.spectralFlux * 0.18,
                  )
                : fallbackBeat

            state.smoothLevel = smoothAsymmetric(
                state.smoothLevel,
                levelTarget,
                mix(9.5, 4.0, smoothing),
                mix(3.0, 1.15, smoothing),
                dt,
            )
            state.smoothBass = smoothAsymmetric(
                state.smoothBass,
                bassTarget,
                mix(8.5, 3.6, smoothing),
                mix(2.7, 1.0, smoothing),
                dt,
            )
            state.smoothMid = smoothAsymmetric(
                state.smoothMid,
                midTarget,
                mix(9.2, 3.9, smoothing),
                mix(2.9, 1.08, smoothing),
                dt,
            )
            state.smoothTreble = smoothAsymmetric(
                state.smoothTreble,
                trebleTarget,
                mix(10.5, 4.3, smoothing),
                mix(3.2, 1.18, smoothing),
                dt,
            )
            state.smoothSwell = smoothApproach(state.smoothSwell, swellTarget, mix(4.0, 1.55, smoothing), dt)

            const presenceTarget = clamp(
                state.smoothBass * 0.36 +
                    state.smoothMid * 0.32 +
                    state.smoothTreble * 0.18 +
                    state.smoothLevel * 0.24 +
                    state.smoothSwell * 0.16,
                0,
                1.2,
            )
            state.presence = smoothAsymmetric(
                state.presence,
                presenceTarget,
                mix(6.3, 2.8, smoothing),
                mix(2.5, 0.95, smoothing),
                dt,
            )

            const beatBloomTarget = clamp(beatTarget * (0.42 + beatConfidence * 0.58) + state.smoothSwell * 0.12, 0, 1)
            state.beatBloom = smoothAsymmetric(
                state.beatBloom,
                beatBloomTarget,
                mix(15.0, 6.4, smoothing),
                mix(4.6, 1.8, smoothing),
                dt,
            )

            const floorTarget = clamp(0.08 + state.presence * mix(0.1, 0.18, intensity), 0.08, 0.28)
            state.floor = smoothApproach(state.floor, floorTarget, mix(3.4, 1.35, smoothing), dt)

            ctx.setUniform('iCascadeLevel', state.smoothLevel)
            ctx.setUniform('iCascadeBass', state.smoothBass)
            ctx.setUniform('iCascadeMid', state.smoothMid)
            ctx.setUniform('iCascadeTreble', state.smoothTreble)
            ctx.setUniform('iCascadeSwell', state.smoothSwell)
            ctx.setUniform('iCascadePresence', state.presence)
            ctx.setUniform('iCascadeBeatBloom', state.beatBloom)
            ctx.setUniform('iCascadeFloor', state.floor)
        },
        presets: [
            {
                controls: {
                    barWidth: 70,
                    glow: 60,
                    intensity: 92,
                    palette: 'Fire',
                    scene: 'Horizon',
                    smoothing: 45,
                    speed: 7,
                },
                description:
                    'Arena lights ignite on the chorus drop — towering spectral columns rise from a blazing horizon like crowd hands reaching for the hook',
                name: 'Stadium Anthem',
            },
            {
                controls: {
                    barWidth: 85,
                    glow: 75,
                    intensity: 95,
                    palette: 'Cyberpunk',
                    scene: 'Tunnel',
                    smoothing: 60,
                    speed: 4,
                },
                description:
                    'Dubstep in a sweat-soaked basement — fat bars throb through a tunnel of pure low-end pressure',
                name: 'Basement Frequencies',
            },
            {
                controls: {
                    barWidth: 30,
                    glow: 40,
                    intensity: 58,
                    palette: 'Ice',
                    scene: 'Cascade',
                    smoothing: 80,
                    speed: 3,
                },
                description:
                    'Classical piano refracted through ice prisms — each note a delicate falling column of frozen light',
                name: 'Crystal Cascade',
            },
            {
                controls: {
                    barWidth: 50,
                    glow: 55,
                    intensity: 85,
                    palette: 'Sunset',
                    scene: 'Horizon',
                    smoothing: 35,
                    speed: 7,
                },
                description:
                    'Mission control monitors during a coronal mass ejection — bars surge from a burning horizon in solar orange and amber',
                name: 'Solar Flare Grid',
            },
            {
                controls: {
                    barWidth: 55,
                    glow: 50,
                    intensity: 78,
                    palette: 'SilkCircuit',
                    scene: 'Cascade',
                    smoothing: 65,
                    speed: 5,
                },
                description:
                    'The machine is dreaming — cascading frequency analysis rendered in electric purple and neon cyan, smooth and sentient',
                name: 'SilkCircuit Flux',
            },
            {
                controls: {
                    barWidth: 25,
                    glow: 85,
                    intensity: 65,
                    palette: 'Aurora',
                    scene: 'Tunnel',
                    smoothing: 88,
                    speed: 2,
                },
                description:
                    'Whale song reverberates through a borealis-lit ice cave — thin spectral lines shimmer inside a tunnel of frozen green',
                name: 'Whale Song Cavern',
            },
            {
                controls: {
                    barWidth: 95,
                    glow: 35,
                    intensity: 100,
                    palette: 'Cyberpunk',
                    scene: 'Mirror',
                    smoothing: 25,
                    speed: 10,
                },
                description:
                    'A Tokyo arcade cabinet overloads on drum-and-bass — razor-wide columns slam the mirror line in magenta and cyan at 174 BPM',
                name: 'Akihabara Overload',
            },
            {
                controls: {
                    barWidth: 42,
                    glow: 70,
                    intensity: 55,
                    palette: 'Ice',
                    scene: 'Horizon',
                    smoothing: 80,
                    speed: 3,
                },
                description:
                    'A glass city skyline refracts ambient piano into pale blue towers — each note lifts a column of cold crystal light from still water',
                name: 'Glass City Lullaby',
            },
        ],
    },
)
