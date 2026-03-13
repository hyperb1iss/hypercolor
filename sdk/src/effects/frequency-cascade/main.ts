import { combo, effect, num } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect('Frequency Cascade', shader, {
    speed:     num('Speed', [1, 10], 5, { group: 'Audio' }),
    intensity: num('Intensity', [0, 100], 75, { group: 'Audio' }),
    smoothing: num('Smoothing', [0, 100], 50, { group: 'Audio' }),
    barWidth:  num('Bar Width', [0, 100], 58, { group: 'Geometry' }),
    palette:   combo('Palette', ['Aurora', 'Cyberpunk', 'Fire', 'Ice', 'SilkCircuit', 'Sunset'], { default: 'Aurora', group: 'Color' }),
    glow:      num('Glow', [0, 100], 28, { group: 'Geometry' }),
    scene:     combo('Scene', ['Cascade', 'Prism Skyline', 'Pulse Grid', 'Spectrum Tunnel'], { default: 'Cascade', group: 'Scene' }),
}, {
    description: 'Spectrum cascade with scene modes and no-audio fallback motion',
    audio: true,
    presets: [
        {
            name: 'Stadium Anthem',
            description: 'Arena lights ignite on the chorus drop — towering spectral columns rise like crowd hands reaching for the hook',
            controls: {
                speed: 7,
                intensity: 95,
                smoothing: 30,
                barWidth: 75,
                palette: 'Fire',
                glow: 65,
                scene: 'Prism Skyline',
            },
        },
        {
            name: 'Basement Frequencies',
            description: 'Dubstep in a sweat-soaked basement — fat bars throbbing through a tunnel of pure low-end pressure',
            controls: {
                speed: 4,
                intensity: 100,
                smoothing: 70,
                barWidth: 90,
                palette: 'Cyberpunk',
                glow: 85,
                scene: 'Spectrum Tunnel',
            },
        },
        {
            name: 'Crystal Cascade',
            description: 'Classical piano refracted through ice prisms — each note a delicate falling column of frozen light',
            controls: {
                speed: 3,
                intensity: 60,
                smoothing: 80,
                barWidth: 35,
                palette: 'Ice',
                glow: 40,
                scene: 'Cascade',
            },
        },
        {
            name: 'Solar Flare Grid',
            description: 'Mission control monitors during a coronal mass ejection — pulsing data grid screams in solar orange and amber',
            controls: {
                speed: 8,
                intensity: 88,
                smoothing: 20,
                barWidth: 50,
                palette: 'Sunset',
                glow: 55,
                scene: 'Pulse Grid',
            },
        },
        {
            name: 'SilkCircuit Flux',
            description: 'The machine is dreaming — cascading frequency analysis rendered in electric purple and neon cyan, smooth and sentient',
            controls: {
                speed: 5,
                intensity: 78,
                smoothing: 60,
                barWidth: 58,
                palette: 'SilkCircuit',
                glow: 45,
                scene: 'Cascade',
            },
        },
    ],
})
