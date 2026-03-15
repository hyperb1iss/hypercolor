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
const STYLE_MODES = ['Glitch', 'Grain', 'Holo', 'Standard'] as const

export default effect(
    'Kaleido Tunnel',
    shader,
    {
        intensity: num('Intensity', [20, 220], 120, {
            group: 'Color',
            step: 1,
            tooltip: 'Overall brightness and energy in the tunnel.',
            uniform: 'iColorIntensity',
        }),
        palette: combo('Palette', PALETTE_MODES, {
            default: 'Rainbow',
            group: 'Color',
            tooltip: 'Core tunnel palette.',
            uniform: 'iColorMode',
        }),
        pulse: num('Pulse', [0, 100], 50, {
            group: 'Geometry',
            step: 1,
            tooltip: 'Breathing and shimmer in the tunnel energy.',
            uniform: 'iPulse',
        }),
        segments: num('Segments', [3, 12], 6, {
            group: 'Geometry',
            step: 1,
            tooltip: 'Number of mirrored kaleidoscope slices.',
            uniform: 'iSegments',
        }),
        speed: num('Speed', [1, 20], 5, {
            group: 'Motion',
            normalize: 'none',
            step: 0.5,
            tooltip: 'Controls tunnel motion speed. Values above 10 push into extra overdrive.',
            uniform: 'iSpeed',
        }),
        style: combo('Style', STYLE_MODES, {
            default: 'Standard',
            group: 'Color',
            tooltip: 'Post-processing treatment.',
            uniform: 'iStyle',
        }),
        twist: num('Twist', [0, 100], 40, {
            group: 'Motion',
            step: 1,
            tooltip: 'Angular twist through the tunnel depth.',
            uniform: 'iTwist',
        }),
        warp: num('Warp', [0, 100], 30, {
            group: 'Geometry',
            step: 1,
            tooltip: 'Psychedelic distortion before the kaleidoscope fold.',
            uniform: 'iWarp',
        }),
    },
    {
        author: 'Hypercolor',
        description:
            'Fall through an infinite kaleidoscope — mirrored symmetry folds around a warping tunnel of endlessly shifting light',
        presets: [
            {
                controls: {
                    intensity: 210,
                    palette: 'Neon',
                    pulse: 85,
                    segments: 12,
                    speed: 14,
                    style: 'Glitch',
                    twist: 95,
                    warp: 100,
                },
                description:
                    'Fractal geometry shatters into infinite recursive symmetry — maximum segments, full warp, neon overload',
                name: 'DMT Breakthrough',
            },
            {
                controls: {
                    intensity: 160,
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
                    intensity: 80,
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
                    intensity: 140,
                    palette: 'Sunset',
                    pulse: 45,
                    segments: 6,
                    speed: 4,
                    style: 'Standard',
                    twist: 60,
                    warp: 35,
                },
                description:
                    'Golden hour refracting through a desert crystal — warm vaporwave tones fold into gentle geometry',
                name: 'Sunset Kaleidoscope',
            },
            {
                controls: {
                    intensity: 190,
                    palette: 'Monochrome',
                    pulse: 15,
                    segments: 3,
                    speed: 8,
                    style: 'Grain',
                    twist: 80,
                    warp: 50,
                },
                description:
                    'Stripped of color, pure geometry remains — clinical black and white spinning at the edge of sanity',
                name: 'Monochrome Asylum',
            },
            {
                controls: {
                    intensity: 55,
                    palette: 'Deep Sea',
                    pulse: 95,
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
                    intensity: 220,
                    palette: 'Electric',
                    pulse: 60,
                    segments: 10,
                    speed: 18,
                    style: 'Glitch',
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
