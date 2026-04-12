import { combo, effect, num } from '@hypercolor/sdk'
import shader from './fragment.glsl'

const PALETTE_MODES = [
    'Amethyst',
    'Deep Sea',
    'Electric',
    'Monochrome',
    'Neon',
    'Rainbow',
    'Sunset',
    'Toxic',
    'Vaporwave',
] as const
const STYLE_MODES = ['Glitch', 'Grain', 'Holo', 'Standard', 'Fractal'] as const

export default effect(
    'Breakthrough',
    shader,
    {
        palette: combo('Palette', PALETTE_MODES, {
            default: 'Neon',
            group: 'Color',
            tooltip: 'Core color palette.',
            uniform: 'iColorMode',
        }),
        pulse: num('Pulse', [0, 100], 72, {
            group: 'Geometry',
            step: 1,
            tooltip: 'Radial breathing, outward surge, and shimmer.',
            uniform: 'iPulse',
        }),
        segments: num('Segments', [3, 12], 9, {
            group: 'Geometry',
            step: 1,
            tooltip: 'Number of mirrored kaleidoscope slices.',
            uniform: 'iSegments',
        }),
        speed: num('Speed', [1, 20], 8.5, {
            group: 'Motion',
            normalize: 'none',
            step: 0.5,
            tooltip: 'Outward tunnel flow speed. Above 10 enters overdrive.',
            uniform: 'iSpeed',
        }),
        style: combo('Style', STYLE_MODES, {
            default: 'Fractal',
            group: 'Color',
            tooltip: 'Post-processing treatment.',
            uniform: 'iStyle',
        }),
        twist: num('Twist', [0, 100], 72, {
            group: 'Motion',
            step: 1,
            tooltip: 'Angular spiral through the tunnel depth.',
            uniform: 'iTwist',
        }),
        warp: num('Warp', [0, 100], 64, {
            group: 'Geometry',
            step: 1,
            tooltip: 'Psychedelic distortion before the kaleidoscope fold.',
            uniform: 'iWarp',
        }),
    },
    {
        author: 'Hypercolor',
        description:
            'Shatter through infinite fractal symmetry — mirrored light folds and spirals as reality dissolves into pure color',
        presets: [
            {
                controls: {
                    palette: 'Neon',
                    pulse: 78,
                    segments: 10,
                    speed: 12.5,
                    style: 'Fractal',
                    twist: 90,
                    warp: 82,
                },
                description:
                    'Recursive petals, tunneling mirrors, and neon chrysanthemum geometry pushed to the edge of coherence.',
                name: 'DMT Breakthrough',
            },
            {
                controls: {
                    palette: 'Amethyst',
                    pulse: 30,
                    segments: 8,
                    speed: 2,
                    style: 'Holo',
                    twist: 55,
                    warp: 25,
                },
                description:
                    'Stained glass mandala rotating in slow reverence — deep amethyst hues through a holographic prism',
                name: 'Cathedral of Light',
            },
            {
                controls: {
                    palette: 'Toxic',
                    pulse: 70,
                    segments: 5,
                    speed: 3.5,
                    style: 'Grain',
                    twist: 40,
                    warp: 65,
                },
                description:
                    'Sinking through bioluminescent depths — toxic greens pulse through a grainy deep-sea corridor',
                name: 'Abyssal Descent',
            },
            {
                controls: {
                    palette: 'Sunset',
                    pulse: 58,
                    segments: 7,
                    speed: 4.5,
                    style: 'Fractal',
                    twist: 60,
                    warp: 42,
                },
                description:
                    'Golden hour refracting through a desert crystal — warm amber and deep purple fold into gentle geometry',
                name: 'Sunset Kaleidoscope',
            },
            {
                controls: {
                    palette: 'Monochrome',
                    pulse: 15,
                    segments: 3,
                    speed: 8,
                    style: 'Fractal',
                    twist: 80,
                    warp: 50,
                },
                description:
                    'Stripped of color, pure geometry remains — clinical black and white spinning at the edge of sanity',
                name: 'Monochrome Asylum',
            },
            {
                controls: {
                    palette: 'Deep Sea',
                    pulse: 90,
                    segments: 4,
                    speed: 1.5,
                    style: 'Holo',
                    twist: 20,
                    warp: 80,
                },
                description:
                    'Drift through a bioluminescent jellyfish bloom — holographic membranes pulse and warp in the abyssal current',
                name: 'Jellyfish Cathedral',
            },
            {
                controls: {
                    palette: 'Electric',
                    pulse: 78,
                    segments: 10,
                    speed: 18,
                    style: 'Fractal',
                    twist: 100,
                    warp: 90,
                },
                description:
                    'Lightning strikes a hall of mirrors — ten electric facets shatter and reassemble at impossible velocity',
                name: 'Tesla Coil Museum',
            },
        ],
    },
)
