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
        flow: num('Flow', [0, 100], 45, {
            group: 'Motion',
            step: 1,
            tooltip: 'Outward tunnel intensity. At 0 the concentric shells and radial waves drop out so the kaleidoscope fold reads on its own.',
            uniform: 'iFlow',
        }),
        pulse: num('Pulse', [0, 100], 55, {
            group: 'Geometry',
            step: 1,
            tooltip: 'Radial breathing, outward surge, and shimmer.',
            uniform: 'iPulse',
        }),
        segments: num('Segments', [3, 12], 8, {
            group: 'Geometry',
            step: 1,
            tooltip: 'Number of mirrored kaleidoscope slices.',
            uniform: 'iSegments',
        }),
        speed: num('Speed', [1, 20], 5, {
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
        twist: num('Twist', [0, 100], 55, {
            group: 'Motion',
            step: 1,
            tooltip: 'Angular spiral through the tunnel depth.',
            uniform: 'iTwist',
        }),
        warp: num('Warp', [0, 100], 50, {
            group: 'Geometry',
            step: 1,
            tooltip: 'Psychedelic distortion before the kaleidoscope fold.',
            uniform: 'iWarp',
        }),
    },
    {
        author: 'Hypercolor',
        description:
            'Shatter through infinite fractal symmetry. Mirrored light folds and spirals as reality dissolves into pure color.',
        presets: [
            {
                controls: {
                    flow: 72,
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
                    flow: 15,
                    palette: 'Amethyst',
                    pulse: 30,
                    segments: 8,
                    speed: 2,
                    style: 'Holo',
                    twist: 55,
                    warp: 25,
                },
                description:
                    'Stained glass mandala rotating in slow reverence. Deep amethyst hues through a holographic prism.',
                name: 'Cathedral of Light',
            },
            {
                controls: {
                    flow: 55,
                    palette: 'Toxic',
                    pulse: 70,
                    segments: 5,
                    speed: 3.5,
                    style: 'Grain',
                    twist: 40,
                    warp: 65,
                },
                description:
                    'Sinking through bioluminescent depths. Toxic greens pulse through a grainy deep-sea corridor.',
                name: 'Abyssal Descent',
            },
            {
                controls: {
                    flow: 28,
                    palette: 'Sunset',
                    pulse: 58,
                    segments: 7,
                    speed: 4.5,
                    style: 'Fractal',
                    twist: 60,
                    warp: 42,
                },
                description:
                    'Golden hour refracting through a desert crystal. Warm amber and deep purple fold into gentle geometry.',
                name: 'Sunset Kaleidoscope',
            },
            {
                controls: {
                    flow: 18,
                    palette: 'Monochrome',
                    pulse: 15,
                    segments: 3,
                    speed: 8,
                    style: 'Fractal',
                    twist: 80,
                    warp: 50,
                },
                description:
                    'Stripped of color, pure geometry remains. Clinical black and white spinning at the edge of sanity.',
                name: 'Monochrome Asylum',
            },
            {
                controls: {
                    flow: 42,
                    palette: 'Deep Sea',
                    pulse: 90,
                    segments: 4,
                    speed: 1.5,
                    style: 'Holo',
                    twist: 20,
                    warp: 80,
                },
                description:
                    'Drift through a bioluminescent jellyfish bloom. Holographic membranes pulse and warp in the abyssal current.',
                name: 'Jellyfish Cathedral',
            },
            {
                controls: {
                    flow: 85,
                    palette: 'Electric',
                    pulse: 78,
                    segments: 10,
                    speed: 18,
                    style: 'Fractal',
                    twist: 100,
                    warp: 90,
                },
                description:
                    'Lightning strikes a hall of mirrors. Ten electric facets shatter and reassemble at impossible velocity.',
                name: 'Mirror Strike',
            },
        ],
    },
)
